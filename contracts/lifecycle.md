# 生命周期契约

四条时序是 P0 的核心，任何改动不得破坏下列不变量。

## 不变量（改了就是破坏契约）

1. **启动路径上不得有菜单网络等待**（本地有包时）。`boot` 只读文件。
2. **验签失败绝不落盘**。坏包不得覆盖好包，也不得以正式文件名写入。
3. **权限快照不落盘**。只在内存与登录响应中存在。
4. **登出立刻清空快照**。下一个账号不得看到上一个账号的菜单。
5. **版本探测在渲染之后**。不得阻塞首屏。

## 启动

```
boot()
  ├─ clean_tmp()
  ├─ load_latest()  ── 读 menu.<ver>.mtree
  │     ├─ Ok  → source=local，零网络
  │     ├─ NoPack → fetch_and_install()  → source=network
  │     └─ Pack(验签失败) → fetch_and_install() → source=network + 告警
  └─ 返回 BootInfo（含 load_ms）
```

断网 + 已有本地包 → 正常启动。断网 + 无本地包 → `BootError::Offline`，文案必须提示"先联网启动一次"。

## 登录

```
login(roleId)
  ├─ 拿 PermSnapshot（P0 为 mock，P1 接真实 IAM）
  ├─ filter_visible(全量树, 快照)   ← 纯内存，<1ms
  └─ 返回 LoginResult（menu 已剪枝）
```

前端拿到 `menu` 后**不再做任何权限判断**。

## 登出 / 切号

```
logout()
  └─ snapshot = None
```

切换账号 = 登出 + 登录，旧快照不得复用。收到 401 同样走这条路径。

## 更新

```
check_update()   ← 前端在首屏渲染后 ~600ms 才调用
  ├─ probe() 离线 → checked=false，继续用本地
  ├─ 版本相同 → updated=false
  └─ 版本不同 → download() → install()（验签）→ updated=true
                   └─ 验签失败 → 拒绝安装，保留本地版本
```

菜单版本更新**不重置权限快照**——两者独立演进。

## 篡改恢复

```
reload_from_disk()
  ├─ load_latest() 验签失败
  ├─ 记录 verify_failed=true
  ├─ fetch_and_install()  ← 服务端救回
  └─ 已登录则重新剪枝
```

此路径在 P0 有端到端验证：改坏文件 → 重启客户端 → 文件自动恢复为合法包。

## 已知缺口（不在 P0 范围）

会话超时锁定（P3）· 并发会话检测（P3）· 权限变更推送（P2）· 包级灰度与索引快照（P2+）
