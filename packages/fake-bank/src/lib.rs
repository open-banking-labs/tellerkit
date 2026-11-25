//! 本地假后台：一个真的 TCP 服务端，用来验证"路由 + 编解码 + 传输"整条链路。
//!
//! 为什么不用 mock 传输做端到端：mock 验证不了分帧、连接生命周期、
//! 响应解析这些**只有真 socket 才会出错**的地方（Windows 上的 RST 就是这么抓到的）。
//!
//! 协议与 `TcpTransport` 对齐：`[u32 大端长度][载荷]`，请求与响应同构。
//! 载荷是 JSON 时按 `{"op":..,"payload":..}` 处理；否则当作二进制下载请求。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tellerkit_core::perm::PermSnapshot;

/// 假后台的可调行为。
#[derive(Debug, Clone)]
pub struct FakeBankConfig {
    /// 菜单版本
    pub menu_version: u64,
    /// 下载返回的字节（模拟签名包）
    pub pack_bytes: Vec<u8>,
    /// 交易拒绝原因；Some 时所有提交都被拒绝
    pub reject_reason: Option<String>,
    /// 收到多少次请求（按 op 累计）
    pub stats: Arc<Mutex<Vec<String>>>,
    /// 是否按幂等键去重（真服务端必须这么做，见 `changes/001`）
    pub idempotent: bool,
    /// 幂等表：key → 流水号
    pub accepted: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    /// 实际"入账"次数。用来证明重复提交没有重复入账。
    pub charged: Arc<Mutex<u32>>,
}

impl Default for FakeBankConfig {
    fn default() -> Self {
        Self {
            menu_version: 7,
            pack_bytes: b"FAKE-MTREE-PACK".to_vec(),
            reject_reason: None,
            stats: Arc::new(Mutex::new(Vec::new())),
            idempotent: true,
            accepted: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            charged: Arc::new(Mutex::new(0)),
        }
    }
}

/// 运行中的假后台。Drop 时停掉监听线程。
pub struct FakeBank {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    config: FakeBankConfig,
}

impl FakeBank {
    /// 绑定到 127.0.0.1 的随机端口并开始服务。
    pub fn start(config: FakeBankConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let addr = listener.local_addr().expect("取本地地址失败");
        listener.set_nonblocking(true).expect("设置非阻塞失败");

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let cfg = config.clone();

        let handle = thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // 短连接：一个连接处理一个请求
                        let _ = handle_connection(stream, &cfg);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            addr,
            stop,
            handle: Some(handle),
            config,
        }
    }

    pub fn host(&self) -> String {
        "127.0.0.1".to_string()
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn config(&self) -> &FakeBankConfig {
        &self.config
    }

    /// 收到过的 op 序列，断言"请求确实到了后台"。
    pub fn ops(&self) -> Vec<String> {
        self.config
            .stats
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }
}

impl Drop for FakeBank {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, config: &FakeBankConfig) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let n = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body)?;

    let response: Vec<u8> = if body.first() == Some(&b'{') {
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let op = request
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        if let Ok(mut stats) = config.stats.lock() {
            stats.push(op.clone());
        }
        let payload = serve_op(&op, &request, config);
        serde_json::to_vec(&json!({ "payload": payload })).unwrap_or_else(|_| b"{}".to_vec())
    } else {
        // 非 JSON：二进制下载请求
        if let Ok(mut stats) = config.stats.lock() {
            stats.push("download".to_string());
        }
        config.pack_bytes.clone()
    };

    let len = u32::try_from(response.len()).unwrap_or(0);
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&response)?;
    stream.flush()
}

fn serve_op(op: &str, request: &Value, config: &FakeBankConfig) -> Value {
    match op {
        "probe_menu" => json!({
            "latest": config.menu_version,
            "shell_compat_min": 1,
            "shell_compat_max": 1
        }),
        "login" => {
            let role = request
                .pointer("/payload/role_id")
                .and_then(|v| v.as_str())
                .unwrap_or("teller");
            let perm_version = request
                .pointer("/payload/perm_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            let snapshot = PermSnapshot {
                user_id: format!("U-{role}"),
                teller_no: "T0231".to_string(),
                outlet_code: "01023".to_string(),
                perm_version,
                menu_codes: vec!["account".into(), "account.deposit".into()],
                action_codes: vec!["account.deposit.print".into()],
                shortcuts: Default::default(),
                role_label: role.to_string(),
            };
            serde_json::to_value(snapshot).unwrap_or(Value::Null)
        }
        "submit_tx" => match &config.reject_reason {
            Some(reason) => json!({
                "error": { "code": "TX_REJECTED", "message": reason, "retryable": false }
            }),
            None => {
                let key = request
                    .pointer("/payload/idempotency_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                if config.idempotent {
                    if let Ok(mut map) = config.accepted.lock() {
                        if let Some(existing) = map.get(key) {
                            // 同键重复提交：返回同一个流水号，**不再入账**
                            return json!({ "reference": existing });
                        }
                        let reference = format!("FAKE-{key}");
                        map.insert(key.to_string(), reference.clone());
                        if let Ok(mut n) = config.charged.lock() {
                            *n += 1;
                        }
                        return json!({ "reference": reference });
                    }
                }

                // 未声明幂等：每次都入账（用来证明客户端为什么不允许离线挂起）
                if let Ok(mut n) = config.charged.lock() {
                    *n += 1;
                }
                json!({ "reference": format!("FAKE-{key}-{}", *config.charged.lock().unwrap()) })
            }
        },
        _ => json!({ "code": "UNKNOWN_OP", "message": op }),
    }
}
