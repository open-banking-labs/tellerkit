# 009 传输层 TLS 的取舍

## 现状

`HttpTransport` 只支持 `http://`，全链路明文。这在柜面环境不可接受。
本轮已做的是**失败关闭**：配置里出现 `https://` 会被明确拒绝并回退 mock，
不会悄悄降级成明文；`TK_REQUIRE_TLS=1` 时连 `http://` 也拒绝。

## 为什么没有直接实现

TLS 客户端要引入 `rustls`（及其加密后端 `ring` / `aws-lc-rs`）。

| 风险 | 说明 |
|---|---|
| Windows CI 构建 | `aws-lc-rs` 需要 NASM/CMake；`ring` 需要 perl 与 C 工具链。CI 跑 `windows-latest`，没有验证过 |
| 根证书来源 | 需要 `webpki-roots`（内置）或系统证书库（平台相关） |
| 体积 | rustls + 根证书约增加 1–2MB 二进制 |

**这类"加依赖且可能打挂 Windows CI"的改动，应该先定方案再动手**，
而不是在补安全缺口时顺手把构建搞红。

## 建议方案

1. 依赖：`rustls`（`ring` 后端）+ `webrtc-roots` 等价的 `webpki-roots`；
   禁用平台证书库，保证三平台行为一致（柜面环境不需要企业自签证书场景，先不做）。
2. 端口不变：`HttpTransport` 内部按 scheme 选明文/TLS，`Transport` trait 签名不动。
3. 测试：`rcgen` 生成本地证书，起一个 TLS 假服务器，验证握手与证书校验失败路径。
4. 配置：`https://` 直接启用；`TK_REQUIRE_TLS=1` 在生产构建里默认打开。

## 影响

- `packages/transport-net`：新增 TLS 分支与依赖
- `Cargo.lock`：新增 4–6 个 crate
- CI：Windows 上首次构建会慢，需要验证 `ring`/`aws-lc` 是否可用

## 在此之前的行为（已实现）

| 场景 | 行为 |
|---|---|
| 配置写 `https://` | 拒绝加载 → 回退 mock，启动 notes 说明"需要 TLS" |
| `TK_REQUIRE_TLS=1` + `http://` | 拒绝加载 → 回退 mock |
| 默认（未设开关）+ `http://` | 允许（演示/内网联调），但启动 notes 提示明文 |

最后一行是**已知的、明示的风险**：不设开关就是明文。生产部署必须设 `TK_REQUIRE_TLS=1`。
