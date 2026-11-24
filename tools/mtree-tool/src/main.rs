//! `.mtree` 菜单包构建工具（模拟服务端构建环节）。
//!
//! 真实环境里这一步在服务端 CI 里执行，私钥不离开服务端；客户端只拿到成品
//! 文件和内置公钥。本工具存在的意义是让 P0 阶段能自己造包、自己验签、自己
//! 演示"改一个字节就失效"。
//!
//! 用法：
//!   mtree-tool keygen
//!   mtree-tool pubkey
//!   mtree-tool build  <tree.json> <out.mtree> [--key <hex>] [--version N]
//!   mtree-tool verify <file.mtree> [--pubkey <hex>]
//!   mtree-tool inspect <file.mtree>
//!   mtree-tool dump   <file.mtree>

use ed25519_dalek::{SigningKey, VerifyingKey};
use std::process::ExitCode;
use tellerkit_core::model::MenuTree;
use tellerkit_core::pack::{self, MenuPack};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    let result = match cmd {
        "keygen" => keygen(),
        "pubkey" => {
            println!("{}", tellerkit_core::mock::demo_pubkey_hex());
            println!("（演示公钥，来自公开的 DEMO_SEED，生产环境必须替换）");
            Ok(())
        }
        "demo" => demo(rest),
        "fixtures" => fixtures(rest),
        "pkg-build" => pkg_build(rest),
        "pkg-verify" => pkg_verify(rest),
        "build" => build(rest),
        "verify" => verify(rest),
        "inspect" => inspect(rest),
        "dump" => dump(rest),
        other => {
            eprintln!("未知命令: {other}\n");
            help();
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误: {e}");
            ExitCode::FAILURE
        }
    }
}

fn help() {
    eprintln!(
        "用法:\n  mtree-tool keygen\n  mtree-tool pubkey\n  \
         mtree-tool demo [out.json]\n  \
         mtree-tool build <tree.json> <out.mtree> [--key <hex>] [--version N]\n  \
         mtree-tool verify <file.mtree> [--pubkey <hex>]\n  \
         mtree-tool inspect <file.mtree>\n  mtree-tool dump <file.mtree>"
    );
}

fn keygen() -> Result<(), String> {
    use rand::rngs::OsRng;
    let sk = SigningKey::generate(&mut OsRng);
    println!("private_key={}", hex::encode(sk.to_bytes()));
    println!("public_key={}", pack::pubkey_to_hex(&sk.verifying_key()));
    println!("\n私钥只应留在服务端构建环境；公钥内置到客户端。");
    Ok(())
}

fn flag(rest: &[String], name: &str) -> Option<String> {
    rest.iter()
        .position(|a| a == name)
        .and_then(|i| rest.get(i + 1).cloned())
}

fn positional(rest: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in rest {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(a);
    }
    out
}

/// 把一个业务包目录打成签名的 `.bizpkg`。
///
/// 目录里四份 JSON 的字段名必须与 Rust 结构体一致（snake_case）；
/// 这里不做任何字段改名，避免"打包脚本悄悄兼容了错误格式"。
fn pkg_build(rest: &[String]) -> Result<(), String> {
    use tellerkit_core::pkg::BizPackage;

    let pos = positional(rest);
    let dir = pos.first().ok_or("缺少业务包目录")?;
    let out = pos.get(1).ok_or("缺少输出 .bizpkg 路径")?;

    let read_json = |name: &str| -> Result<String, String> {
        let path = std::path::Path::new(dir).join(name);
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))
    };

    let manifest = serde_json::from_str(&read_json("manifest.json")?)
        .map_err(|e| format!("manifest.json 解析失败: {e}"))?;
    let menus = serde_json::from_str(&read_json("menus.json")?)
        .map_err(|e| format!("menus.json 解析失败: {e}"))?;
    let permissions = serde_json::from_str(&read_json("permissions.json")?)
        .map_err(|e| format!("permissions.json 解析失败: {e}"))?;
    let routes = serde_json::from_str(&read_json("routes.json")?)
        .map_err(|e| format!("routes.json 解析失败: {e}"))?;
    let transactions = serde_json::from_str(&read_json("transactions.json")?)
        .map_err(|e| format!("transactions.json 解析失败: {e}"))?;

    // 前端资源：把 ui/ 下的文件内容嵌进包
    let mut assets = std::collections::BTreeMap::new();
    let ui_dir = std::path::Path::new(dir).join("ui");
    if let Ok(entries) = std::fs::read_dir(&ui_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("js") {
                let rel = path
                    .strip_prefix(std::path::Path::new(dir))
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = std::fs::read_to_string(&path)
                    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
                assets.insert(rel, source);
            }
        }
    }

    let package = BizPackage {
        manifest,
        menus,
        permissions,
        routes,
        transactions,
        assets,
    };
    let bytes = package
        .encode(&tellerkit_core::mock::demo_signing_key())
        .map_err(|e| format!("打包失败: {e}"))?;
    if let Some(parent) = std::path::Path::new(out).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    std::fs::write(out, &bytes).map_err(|e| format!("写入 {out} 失败: {e}"))?;
    println!(
        "已生成 {out}：{} v{} · {} 个菜单 · {} 字节",
        package.manifest.id,
        package.manifest.version,
        package.menus.len(),
        bytes.len()
    );
    Ok(())
}

fn pkg_verify(rest: &[String]) -> Result<(), String> {
    use tellerkit_core::pkg::{BizPackage, Version};
    let pos = positional(rest);
    let file = pos.first().ok_or("缺少 .bizpkg 路径")?;
    let bytes = std::fs::read(file).map_err(|e| format!("读取失败: {e}"))?;
    let package = BizPackage::decode(&bytes, &tellerkit_core::mock::demo_verifying_key())
        .map_err(|e| format!("验签失败: {e}"))?;
    println!(
        "验签通过: {} v{} · 菜单 {} · 路由 {} · 交易 {}",
        package.manifest.id,
        package.manifest.version,
        package.menus.len(),
        package.routes.len(),
        package.transactions.len()
    );
    let shell = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    let plan = tellerkit_core::pkg::plan_load(vec![package], shell);
    for decision in &plan.skipped {
        println!("  跳过 {}：{}", decision.id, decision.reason);
    }
    Ok(())
}

/// 生成前端预览用的 fixture。
///
/// 数据由**真实核心逻辑**产出（同一份 demo 树、同一个 filter_visible），
/// 因此浏览器里看到的就是客户端会渲染的结果，不存在第二套实现。
fn fixtures(rest: &[String]) -> Result<(), String> {
    use tellerkit_core::filter::filter_visible;
    use tellerkit_core::mock::DemoRole;

    let out = positional(rest)
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "src/fixtures".to_string());
    std::fs::create_dir_all(&out).map_err(|e| format!("创建目录失败: {e}"))?;

    let tree = tellerkit_core::mock::demo_tree(7);
    let pack_bytes = tellerkit_core::pack::MenuPack::encode(
        &tree,
        tellerkit_core::mock::SHELL_COMPAT_MIN,
        tellerkit_core::mock::SHELL_COMPAT_MAX,
        &tellerkit_core::mock::demo_signing_key(),
    )
    .map_err(|e| e.to_string())?;

    let open_started = std::time::Instant::now();
    let _ = tellerkit_core::pack::open(&pack_bytes, &tellerkit_core::mock::demo_verifying_key())
        .map_err(|e| e.to_string())?;
    let open_ms = open_started.elapsed().as_secs_f64() * 1000.0;

    let boot = serde_json::json!({
        "shell_version": env!("CARGO_PKG_VERSION"),
        "pubkey_hex": tellerkit_core::mock::demo_pubkey_hex(),
        "appdata_dir": "（浏览器预览：真实路径在 Tauri 客户端内）",
        "offline": false,
        "local_versions": [7],
        "load": {
            "menu_version": tree.menu_version,
            "source": "local",
            "menu_nodes": tree.nodes.len(),
            "pack_bytes": pack_bytes.len(),
            "load_ms": open_ms
        },
        "notes": []
    });

    let mut sessions = serde_json::Map::new();
    for role in DemoRole::ALL {
        let snapshot = role.snapshot(107);
        let started = std::time::Instant::now();
        let menu = filter_visible(&tree, &snapshot);
        let filter_ms = started.elapsed().as_secs_f64() * 1000.0;
        sessions.insert(
            role.id().to_string(),
            serde_json::json!({
                "snapshot": snapshot,
                "menu": menu,
                "filter_ms": filter_ms
            }),
        );
    }

    let boot_path = format!("{out}/boot.json");
    let sessions_path = format!("{out}/sessions.json");
    std::fs::write(
        &boot_path,
        serde_json::to_string_pretty(&boot).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入失败: {e}"))?;
    std::fs::write(
        &sessions_path,
        serde_json::to_string_pretty(&serde_json::Value::Object(sessions))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入失败: {e}"))?;

    println!(
        "已生成 {boot_path} 与 {sessions_path}（菜单 {} 节点）",
        tree.nodes.len()
    );
    Ok(())
}

/// 导出内置演示菜单树为 JSON（造包的输入样板）。
fn demo(rest: &[String]) -> Result<(), String> {
    let out = positional(rest)
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "demo-tree.json".to_string());
    let tree = tellerkit_core::mock::demo_tree(7);
    let json = serde_json::to_string_pretty(&tree).map_err(|e| e.to_string())?;
    std::fs::write(&out, json).map_err(|e| format!("写入失败: {e}"))?;
    println!("已导出演示菜单树: {out}（{} 个节点）", tree.nodes.len());
    Ok(())
}

fn build(rest: &[String]) -> Result<(), String> {
    let pos = positional(rest);
    let input = pos.first().ok_or("缺少 tree.json 路径")?;
    let output = pos.get(1).ok_or("缺少输出 .mtree 路径")?;

    let signing_key = match flag(rest, "--key") {
        Some(hexkey) => {
            let raw = hex::decode(hexkey.trim()).map_err(|e| format!("私钥不是合法 hex: {e}"))?;
            let arr: [u8; 32] = raw
                .try_into()
                .map_err(|_| "私钥长度必须为 32 字节".to_string())?;
            SigningKey::from_bytes(&arr)
        }
        None => {
            eprintln!("[提示] 未指定 --key，使用公开的演示私钥（仅 demo）");
            tellerkit_core::mock::demo_signing_key()
        }
    };

    let json = std::fs::read_to_string(input).map_err(|e| format!("读取 {} 失败: {e}", input))?;
    let mut tree: MenuTree =
        serde_json::from_str(&json).map_err(|e| format!("解析菜单 JSON 失败: {e}"))?;
    if let Some(v) = flag(rest, "--version") {
        tree.menu_version = v.parse().map_err(|_| "--version 必须是整数".to_string())?;
    }
    tree.validate()
        .map_err(|e| format!("菜单树结构非法: {e}"))?;

    let bytes = MenuPack::encode(
        &tree,
        tellerkit_core::mock::SHELL_COMPAT_MIN,
        tellerkit_core::mock::SHELL_COMPAT_MAX,
        &signing_key,
    )
    .map_err(|e| format!("打包失败: {e}"))?;

    if let Some(dir) = std::path::Path::new(output).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
    }
    std::fs::write(output, &bytes).map_err(|e| format!("写入 {} 失败: {e}", output))?;

    println!(
        "已生成 {output}：menu_version={} 节点数={} 大小={} 字节",
        tree.menu_version,
        tree.nodes.len(),
        bytes.len()
    );
    println!(
        "公钥: {}",
        pack::pubkey_to_hex(&signing_key.verifying_key())
    );
    Ok(())
}

fn parse_pubkey(rest: &[String]) -> Result<VerifyingKey, String> {
    match flag(rest, "--pubkey") {
        Some(h) => pack::pubkey_from_hex(&h).map_err(|e| e.to_string()),
        None => Ok(tellerkit_core::mock::demo_verifying_key()),
    }
}

fn verify(rest: &[String]) -> Result<(), String> {
    let pos = positional(rest);
    let file = pos.first().ok_or("缺少 .mtree 路径")?;
    let bytes = std::fs::read(file).map_err(|e| format!("读取失败: {e}"))?;
    let vk = parse_pubkey(rest)?;
    let pack = MenuPack::verify(&bytes, &vk).map_err(|e| format!("验签失败: {e}"))?;
    let tree = pack.decode().map_err(|e| format!("解码失败: {e}"))?;
    println!(
        "验签通过: menu_version={} 节点数={} shell_compat={}..={}",
        pack.menu_version,
        tree.nodes.len(),
        pack.shell_compat_min,
        pack.shell_compat_max
    );
    Ok(())
}

fn inspect(rest: &[String]) -> Result<(), String> {
    let pos = positional(rest);
    let file = pos.first().ok_or("缺少 .mtree 路径")?;
    let bytes = std::fs::read(file).map_err(|e| format!("读取失败: {e}"))?;
    let pack = MenuPack::parse(&bytes).map_err(|e| format!("解析失败: {e}"))?;
    println!("file          = {file}");
    println!("size          = {} 字节", bytes.len());
    println!("format_version= {}", pack.format_version);
    println!("menu_version  = {}", pack.menu_version);
    println!(
        "shell_compat  = {}..={}",
        pack.shell_compat_min, pack.shell_compat_max
    );
    println!("payload_len   = {} 字节", pack.payload.len());
    println!("signature     = {} 字节", pack::SIGNATURE_LEN);
    Ok(())
}

fn dump(rest: &[String]) -> Result<(), String> {
    let pos = positional(rest);
    let file = pos.first().ok_or("缺少 .mtree 路径")?;
    let bytes = std::fs::read(file).map_err(|e| format!("读取失败: {e}"))?;
    let vk = parse_pubkey(rest)?;
    let tree = pack::open(&bytes, &vk).map_err(|e| format!("打开失败: {e}"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&tree).map_err(|e| e.to_string())?
    );
    Ok(())
}
