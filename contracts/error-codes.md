# 错误码契约

错误分四类，**对外文案一律中文、可操作**，不返回堆栈或 `Debug` 输出。

## 包解析 / 验签（`PackError`）

| 变体 | 触发 | 对外文案 |
|---|---|---|
| `TooShort` | 文件被截断 | 文件长度不足，可能被截断 |
| `BadMagic` | 不是 `.mtree` | magic 不匹配，不是 .mtree 文件 |
| `UnsupportedFormatVersion` | 格式版本不支持 | 不支持的格式版本 |
| `PayloadLenMismatch` | 尾部有多余字节 | payload 长度与头部声明不一致 |
| `SignatureInvalid` | 篡改 / 换密钥 | 签名校验失败：文件已损坏或被篡改 |
| `BadPublicKey` | 公钥非法 | 公钥格式非法 |
| `DecodeFailed` | 反序列化失败 | payload 反序列化失败 |
| `InvalidTree` | 结构非法（重复/缺父/环） | 菜单树结构非法 |
| `SignFailed` | 签名失败（服务端侧） | 签名失败 |

**处置约定**：以上任一错误都必须**拒绝使用该文件**，并由上层决定回退服务端。

## 结构校验（`TreeError`）

`DuplicateCode` · `MissingParent` · `Cycle` · `ActionAsParent`
——打包时校验（非法包不允许下发），加载后再次校验（签名合法 ≠ 结构合法）。

## 本地仓库（`StoreError`）

| 变体 | 含义 | 处置 |
|---|---|---|
| `NoPack` | 本地没有包 | 首次启动，联网一次 |
| `Pack` | 验签失败 | 回退服务端并记录 |
| `Io` | 文件操作失败 | 报可读错误，不静默 |
| `BadDir` | 目录不可创建 | 提示权限问题 |

## 启动编排（`BootError`）

| 变体 | 含义 | 对外文案 |
|---|---|---|
| `Offline` | 无本地包且拉不到 | 本地没有可用菜单包，且无法从服务端获取（…）。请先联网启动一次。 |
| `Install` | 服务端包安装失败 | 服务端菜单包安装失败：… |

## 约定

1. 命令层把上述错误翻译成字符串返回；**不得**把 `Debug` 直接抛给前端。
2. 任何"异常但可继续"的情况（断网、新版本下载失败）**不是错误**，走 `UpdateReport.note`。
3. 新增错误变体 = 改契约，必须同步本文件与 `check-contracts`。
