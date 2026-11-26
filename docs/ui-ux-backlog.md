# 柜面 UI/UX 待办（backlog）

- 来源：2026-11-25 的 UI/UX 评审（[评审全文](reviews/ui-ux-review-2026-11.md)）
- 性质：**草案，不是规格**。不新增场景/验收标准；涉及产品形态的条目需人确认
- 用法：其他会话一次只做一行；每行自带「证据 / 改哪里 / 怎么判绿 / 停手条件」
- 状态：`pending` · `doing` · `blocked` · `done` · `rejected`
- 量级：**S** ≤60 行/一个文件；**M** 一页 + 一个组件；**L** 跨层/需契约

> 评审全文目前在 `docs/reviews/`（未提交）。本文自带证据快照，单独拿去开工也不依赖它。
> 若两者冲突，以**重新测到的现状**为准，先更新本文再动手。

## 状态总览

| ID | 事项 | 级别 | 量级 | 依赖 | 状态 |
|---|---|---|---|---|---|
| UX-01 | 生产壳改两列，rail 条件渲染 | P0 | S | — | pending |
| UX-02 | console 收进诊断模式 | P0 | S | UX-01 | pending |
| UX-03 | 交易页任务优先：表单在前，元数据后置 | P0 | M | UX-02 | pending |
| UX-04 | 顶栏会话菜单（登出/切换/关于） | P0 | S | — | pending |
| UX-05 | 金额归一化/校验/大写预览 | P0 | S | — | pending |
| UX-06 | 成功后冻结、新交易、幂等键绑填单 | P0 | M | UX-05 | pending |
| UX-07 | 结果 banner + 四态动作 | P0 | M | UX-06 | pending |
| UX-08 | 锁屏预警与诚实文案 | P0 | S | — | pending |
| UX-09 | 启动失败可重试/回退 | P1 | M | — | pending |
| UX-10 | 打印/重放/动作错误反馈 | P1 | S | — | pending |
| UX-11 | 待处置全局 badge + 重放确认 | P1 | M | — | pending |
| UX-12 | 搜索命令面板 + 菜单语义/焦点 | P1 | M | — | pending |
| UX-13 | 键盘路径：skip/focus/Enter 提交 | P1 | S | UX-12 | pending |
| UX-14 | 对比度/字号/目标尺寸/焦点 token | P1 | M | — | pending |
| UX-15 | 水印定时更新 + 覆盖关键区域 | P1 | S | — | pending |
| UX-16 | 日终对账入口 + dialog + 真实日界文案 | P1 | M | — | pending |
| UX-17 | 登录页环境标识 | P2 | S | — | pending |
| UX-18 | 柜员文案与诊断文案分轨 | P2 | M | UX-02 | pending |
| UX-19 | 样式卫生：`--line`/switch/reduced-motion | P2 | S | — | pending |
| B1 | 手动锁屏命令 | P0 | L | 契约提案 | blocked |
| B2 | 解锁后的草稿恢复 | P0 | L | IAM / 契约 | blocked |
| B3 | 主管授权队列 badge | P1 | L | 契约 | blocked |
| B4 | 顶栏业务日期/主机在线 | P1 | M | 契约 | blocked |
| B5 | 对账 UTC 日界修正 | P1 | L | Rust / 远程构建 | blocked |
| B6 | 凭证要素/打印预览 | P2 | L | 契约 / 产品 | blocked |
| B7 | 异常明细字段（金额/时间/账号） | P1 | M | 契约 | blocked |

**建议顺序**：UX-01 → 04 → 02 → 03 → 05 → 06 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 14 → 15 → 16 → 17–19。B 组先写提案，不挡主线。

---

## 证据快照（2026-11-25，别重测，除非状态变了）

| 事实 | 实测 |
|---|---|
| 生产构建空列 | 1214×653：316px（26%）；1100×700：316px（29%） |
| 主区宽 | 1214：626px；1100：512px（46.5%） |
| 折叠线 | 1100×700：提交按钮底 652 / 主区底 552（差 100px） |
| 金额 | `1,000.50` 提交成 100 分（`PREVIEW-account.deposit-100`） |
| 对比度 | `--muted #7b899e` 白底 3.55:1，`--bg` 约 3.1:1 |
| 字号/目标 | 83 个叶子文本 <12px；19/19 按钮 <44px |
| 水印 | 14:00 生成，14:05 仍 14:00；深色 console 上不可见 |
| 搜索 | 0 结果显示"没有可见菜单"；搜"取款"后 `focusedIndex=-1` |

## 全局约束（每个会话开工前读）

1. 只读区不得改：`contracts/**`、`docs/adr/**`、`docs/constitution.md`、`.agents/presets/**`、`changes/**`（只能新增提案）。
2. 前端只渲染，不判权限；跨语言类型只来自 `generated/`；不新建 `contracts.ts`。
3. 不在本机跑 cargo；Rust 侧走 `scripts/remote-build.sh`。本地只跑 `npm run build` 与 `npm run gate` 的 node 部分。
4. 一次一个文件/一个页面；单次新增 >300 行退回重拆。一次会话只推一个 ID。
5. 缺契约 → 写 `changes/` 提案，不自行加字段/命令。不要为了"看起来完整"造假数据。
6. 不要引入新依赖、第二套前端应用、假 IAM/假密码框。
7. 诊断能力保留：**迁移，不删除**。四态交易、后端剪枝、离线优先、克制视觉是基线，不要改坏。

---

## P0

### UX-01 生产壳改两列，rail 条件渲染

- **证据**：`showDevTools=false` 移除 DevPanel 后，`.body` 仍保留 316px 第三列（`styles.css:146`），生产构建右侧空白。
- **改哪里**：`apps/shell/src/styles.css:144-148`；`apps/shell/src/App.vue:495`（`.body`）、`:570`（DevPanel）。
- **做什么**：默认 `grid-template-columns: 272px minmax(0,1fr)`；仅有 rail 时加 `.with-rail`（316px 第三列）。给 `.body` 绑定 `:class="{ 'with-rail': showDevTools }"`。
- **怎么判绿**：`npm run build`；`vite preview` 在 1100×700、1366×768 下主区右边缘贴视口、无横向滚动；dev 模式仍是三列。
- **停手条件**：不改 DevPanel 内容；不顺手做响应式重排（另立条目）。

### UX-02 console 收进诊断模式

- **证据**：`<footer class="console">`（`App.vue:601-614`）无构建期门控，148px 技术日志随生产包上线；`剪枝耗时/验签/坏包` 对柜员无意义。
- **改哪里**：`App.vue` 的 topbar 与 footer；`styles.css:62-66`（shell 行高）。
- **做什么**：加 `showDiagnostics`（开发默认开、生产默认关）；顶栏加"诊断"开关；console 只在开启时渲染/占高；日志仍保留。
- **怎么判绿**：生产构建默认无 console、主区多出 148px；开关可开合；`npm run build`。
- **停手条件**：不删日志/DevPanel；不新增设置接口或持久化。

### UX-03 交易页任务优先：表单在前，元数据后置

- **证据**：主区顺序是"可用操作 → 节点元数据 → 交易要素"（`App.vue:513-564`）；1100×700 提交按钮在主区底以下 100px。
- **改哪里**：`App.vue:513-564`；必要时在 `styles.css` 给 `.tx-actions` 加 sticky。
- **做什么**：表单第一；"可用操作/节点元数据"收进 `<details>` 或"权限与节点"抽屉；无事件的权限按钮改成只读标签，不用 primary 视觉；提交条贴 main 底部。
- **怎么判绿**：1100×700 首屏能看到金额/账号/提交；首屏不出现 `actionCodes/account.deposit`；`npm run build`。
- **停手条件**：不改 TxPanel 的提交/结果逻辑（UX-05/06/07 负责）；不删元数据，只迁移。

### UX-04 顶栏会话菜单（登出/切换/关于）

- **证据**：生产顶栏没有会话入口；唯一登出在 DevPanel（`DevPanel.vue:90`），被 `v-if` 移除；实测 `logoutBtn: []`。
- **改哪里**：`App.vue:477-492`（topbar）；`logout()` 在 `App.vue:186-190`。
- **做什么**：右侧"会话 ▾"菜单：`切换柜员`（登出回登录）、`退出应用`（先登出，二次确认）、`关于`（shell 版本/离线包版本）。键盘可开合，Esc 关闭，焦点归还。**手动锁屏不在此项**（见 B1）。
- **怎么判绿**：生产构建可完成切换/退出；确认框防误触；Tab 可操作；`npm run build`。
- **停手条件**：不接 IAM、不做密码框；不加假的"锁屏"按钮。

### UX-05 金额归一化/校验/大写预览

- **证据**：`Number.parseFloat`（`TxPanel.vue:20-24`）把 `1,000.50` 变成 1；实测提交 100 分。`12.345` 被静默四舍五入。
- **改哪里**：`TxPanel.vue:20-24`（`amountCents`）、`:108-111`（金额输入）。
- **做什么**：抽 `parseAmountToCents` 纯函数：去空格/逗号/`￥`，只接受 `/^\d+(\.\d{1,2})?$/`，返回整数分；非法给 `aria-invalid` + 行内错误；下方显示 `¥1,000.50` 与中文大写。
- **怎么判绿**：`1,000.50`→100050、`1000.50`→100050、`1000`→100000、`12.345` 被拒；空/0 禁用且有说明；`npm run build`。
- **停手条件**：不改 `SubmitTransactionRequest` 契约；不引入金额库/新依赖；不用浮点存钱。

### UX-06 成功后冻结、新交易、幂等键绑填单

- **证据**：成功后金额/账号/备注不清空、按钮仍可点；每次点击 `crypto.randomUUID()`（`TxPanel.vue:65`）。实测连点会发第二笔。
- **改哪里**：`TxPanel.vue:57-85`、`:118-137`（表单与操作条）、`:152-171`（结果）。
- **做什么**：状态机 `editing → submitting → committed|pending|rejected`；committed/pending 冻结输入与提交，rejected 解锁可改；首次提交生成 key，同一份内容的重试复用，内容变更或点"新交易"时换 key；主操作改为 `打印凭证 / 新交易`；`busy` 防双击。
- **怎么判绿**：committed/pending 后连点不产生第二次请求；字段不可编辑；点"新交易"才清空并换 key；`npm run build`。
- **停手条件**：不改服务端幂等；在服务端幂等联调（`production-readiness` B1）前，不承诺"不会重复入账"。

### UX-07 结果 banner + 四态动作

- **证据**：结果卡在表单下方（`TxPanel.vue:152`），1214×653 要滚动；成功只有 10.5px 标签；`marked_for_reversal` 用 `warn`（`:30`）；无 `aria-live`。
- **改哪里**：`TxPanel.vue:26-31`（状态映射）、`:152-171`（结果模板）。
- **做什么**：结果移到表单上方 banner，`aria-live="polite"`，自动聚焦。四态四套动作：committed `打印/新交易`；pending `查看待重放/继续`；rejected `修改重试`；marked_for_reversal **danger** + `请勿重复提交` + `申请冲正/查询流水`。补金额、账号掩码、时间、流水号。
- **怎么判绿**：四种状态首屏可见且动作正确；读屏能播报；`npm run build`。
- **停手条件**：不加新状态/契约字段；用现有 `TxResult`。

### UX-08 锁屏预警与诚实文案

- **证据**：默认 180s 锁屏（`packages/core/src/session.rs:41-53`），UI 无预警；锁屏唯一动作映射到 `logout`（`App.vue:598`），填单丢失；文案说"重新登录继续"（`LockScreen.vue:32`），实际是重来。
- **改哪里**：`App.vue` 会话轮询；`LockScreen.vue:16-32`。
- **做什么**：按默认 180s 策略：`idle_secs >= 150` 时非模态提示"30 秒后锁屏"；若将来策略可配，阈值跟随策略，不写死。锁屏页显示网点/柜员/角色/原因/时间；文案改成"锁屏会结束当前会话，未提交内容将丢失"。手动锁屏见 B1，恢复见 B2。
- **怎么判绿**：149s 不提示、150s 提示、180s 锁定；锁定层有 dialog 语义与焦点陷阱；`npm run build`。
- **停手条件**：不改锁屏策略/超时；不伪造"解锁后继续"；不加新命令。

---

## P1

### UX-09 启动失败可重试/回退

- **证据**：启动失败 `fatal = String(err)`，LoginView 只显示原始错误，角色按钮禁用，无重试；坏包只能重启。
- **改哪里**：`LoginView.vue:49-56`；`App.vue` 的 boot/init 流程。
- **做什么**：错误页给人话原因、`重试`（再次 `boot`）；若本地包可用，提供"从本地重载"（`reloadFromDisk`），没有就不显示该按钮；可复制诊断码；busy 与 error 分开。
- **怎么判绿**：模拟坏包 → 错误页；修复后点重试可进登录；`npm run build`。
- **停手条件**：不新增 IPC；不接 IAM；不把原始 Rust 错误直接展示给柜员。

### UX-10 打印/重放/动作错误反馈

- **证据**：`printVoucher`/`replay` 无 try/catch（`TxPanel.vue:52-55, 87-99`）；提交失败 `String(err)`（`:75-81`）；失败只进底部 console。
- **改哪里**：`TxPanel.vue` 上述函数与提示区。
- **做什么**：全部 `await api.*` 包 try/catch；错误翻译成"打印未完成，请重试/联系主管"；在操作附近显示；保留诊断详情到日志。
- **怎么判绿**：强制拒绝打印/重放/提交，界面有可读失败提示、无 unhandled rejection；`npm run build`。
- **停手条件**：不建全局错误框架；保持单页范围；不改契约。

### UX-11 待处置全局 badge + 重放确认

- **证据**：异常列表只在 TxPanel 下方；`重放挂起交易` 全部重放、无确认（`TxPanel.vue:134`）；柜员在别的页面看不到待处置。现有 `txPendingCount`/`txExceptions` 可用，但异常 DTO 只有 `key/kind`。
- **改哪里**：`App.vue` 侧栏/顶栏 + `TxPanel.vue:140-150`。
- **做什么**：侧栏"待处置 N"badge（挂起 + 异常）；点击打开轻量面板（数量 + key 前缀 + kind），不做明细；重放前确认"将重放 N 笔"。明细金额/时间等 B7，不在本项伪造。
- **怎么判绿**：提交后 badge 更新；重放前出现确认；取消不调用 API；`npm run build`。
- **停手条件**：不做逐笔选择（需新 API）；异常列表不显示不存在的字段。

### UX-12 搜索命令面板 + 菜单语义/焦点

- **证据**：搜索结果全部 `depth=0`、无父级；0 条时文案是"没有可见菜单"；搜"取款"后 `focusedIndex=-1`；搜索框内方向键被 return（`App.vue:276-279`）；菜单无 tree 语义；分组 `◇` 文案误导；hover 抢焦点（`App.vue:510`）。
- **改哪里**：`App.vue:502`（搜索）、`:276-279`（键盘）、`rows` 计算；`MenuList.vue:19,32,41`。
- **做什么**：`/` 或 `Ctrl+K` 聚焦；结果带父级路径、命中高亮、Alt+N；上下键选择、Enter 打开、Esc 清空并归还焦点；两种空状态分开；过滤后重置焦点；`role=tree/treeitem`、`aria-expanded/selected`；装饰 glyph `aria-hidden`；hover 只做 hover；分组显示子项数、不显示锁；计数改"6 项功能"。
- **怎么判绿**：全键盘完成搜索→打开；空结果文案正确；菜单有 tree 语义；`npm run build`。
- **停手条件**：不引入 fuzzy/向量/新依赖；保留现有 `Alt+N`。

### UX-13 键盘路径：skip/focus/Enter 提交

- **证据**：选中菜单后焦点仍在菜单；到金额输入要 Tab 十余次；表单不是 `<form>`，Enter 不提交；无 skip link。
- **改哪里**：`App.vue` 主区与菜单激活；`TxPanel.vue` 表单容器。
- **做什么**：加"跳到主内容"；选中菜单后焦点移到任务标题/首字段；用 `<form @submit.prevent>`，Enter 提交（或 Ctrl+Enter）；`?` 打开快捷键帮助。
- **怎么判绿**：从登录到打印凭证全程不碰鼠标；Enter 能提交；`npm run build`。
- **停手条件**：不破坏 Alt+N、搜索框方向键；不引入快捷键库。

### UX-14 对比度/字号/目标尺寸/焦点 token

- **证据**：`--muted` 3.55:1；83 个 <12px 节点；19/19 按钮 <44px；按钮默认 1px 蓝环、菜单 2px 自定义；无 reduced-motion。
- **改哪里**：`styles.css` 的 `:root` 与 `.btn/.menu-row/.search/.tag/.hint` 等。
- **做什么**：正文 14px、辅助 ≥12px；`--muted` 调到约 `#5c6b7f`；桌面控件 36-40px、触屏 44px；统一 `:focus-visible` 2px accent + 2px offset；`prefers-reduced-motion`。
- **怎么判绿**：对比度 ≥4.5:1；关键控件高度达标；键盘焦点可见；`npm run build` + 1100/1366 截图。
- **停手条件**：不改品牌色相/整体布局；不引入字体/UI 库。

### UX-15 水印定时更新 + 覆盖关键区域

- **证据**：`stamp` 是 `computed`，无定时器（`Watermark.vue:13-19`）；14:00 到 14:05 不变；深色 console 不可见；z-index 40 低于锁屏 60、对账 70。
- **改哪里**：`Watermark.vue`；`styles.css:830-912`。
- **做什么**：每分钟刷新；`onBeforeUnmount` 清理；深色区域用可读浅色水印；覆盖 console 与 modal（或 modal 内复制）；`pointer-events: none`。
- **怎么判绿**：跨分钟水印变化；对账/锁屏页可见；不挡点击；`npm run build`。
- **停手条件**：不做成交互元素；不加新依赖。

### UX-16 日终对账入口 + dialog + 真实日界文案

- **证据**：`showReconcile` 从不为 true（`App.vue:32,594`）；overlay 无 dialog 语义/Esc（`ReconcilePanel.vue:42`）；文案"本地时间当日 00:00"（`:55`）与 UTC 日界偏差矛盾（`docs/production-readiness.md:28`）。
- **改哪里**：`App.vue` 菜单/入口；`ReconcilePanel.vue`。
- **做什么**：在"系统/日终"给入口（位置需人确认）；`role=dialog`、`aria-modal`、焦点陷阱、Esc；直接显示接口返回的 `since`，文案不再声称本地 00:00；错误可重试、不 `String(err)`。
- **怎么判绿**：菜单可打开；Esc 可关；文案与 `since` 一致；`npm run build`。
- **停手条件**：UTC→本地日界的核心修正见 B5；不改契约；入口位置未确认前先做可回退的入口。

---

## P2

### UX-17 登录页环境标识

- **证据**：720px 卡片首屏是 v7/11.9ms/20 节点和营销句，只有底部 11.5px 免责声明；无机构/网点/终端号。
- **改哪里**：`LoginView.vue:28-66`。
- **做什么**：顶部醒目"演示环境 · 数据为 mock"；显示 shell 版本；机构/网点/终端号仅在有真实来源时显示，没有就标"—"，不编。
- **怎么判绿**：第一眼能分辨环境；无假数据；`npm run build`。
- **停手条件**：数据缺失就留空/标未知；不新增契约。

### UX-18 柜员文案与诊断文案分轨

- **证据**：柜员界面出现 `剪枝耗时`、`包大小`、`权限版本`、`actionCodes`、`幂等键`、`验签`、`坏包`、`节点元数据`、`/tx/account/deposit`。
- **改哪里**：`labels.ts` + `App.vue`/`TxPanel.vue`/`LockScreen.vue`/`LoginView.vue` 的可见文案。
- **做什么**：操作员模式说人话（"本地菜单已就绪 · v7""网络断开，交易暂不可用""已提交，等待主机确认"）；技术细节只在诊断模式出现；建一张小词汇表。
- **怎么判绿**：操作员首屏 grep 不到上述术语；诊断模式仍保留；`npm run build`。
- **停手条件**：不做全局 i18n；不引依赖；不改 labels 的映射 key。

### UX-19 样式卫生：`--line`/switch/reduced-motion

- **证据**：`var(--line)` 在 `styles.css:892,961` 使用但未定义；`.switch input` 只是放大原生 checkbox（`:500-504`）；无 reduced-motion。
- **改哪里**：`styles.css`。
- **做什么**：定义 `--line`；把开关做成真正的 switch（或删除不可用的开关视觉）；补 reduced-motion；顺手清理落在本文件的硬编码色。
- **怎么判绿**：异常/对账分隔线可见；开关外观与状态明确；`npm run build`。
- **停手条件**：不做整体视觉重做；模板内联色（App.vue:605 等）在各自条目顺手处理，不单独大扫除。

---

## 被阻塞 / 需决策（先写提案，不写实现）

### B1 手动锁屏命令

- **现状**：`LockReason::Manual` 已存在（`packages/core/src/session.rs:24-28,146`），但没有 IPC 命令（`generated/commands.ts` 无 `lock_session`）。
- **阻塞**：需要改 `contracts/ipc-commands.md` + Rust command + `npm run bindings`；AI 不得自改契约。
- **下一步**：写 `changes/` 提案（命令名、入参、返回 `SessionState`、审计原因 `manual`），人批准后再做 UI 按钮。

### B2 解锁后的草稿恢复

- **现状**：锁屏唯一动作是 `logout`（`App.vue:598`）；没有 `unlock`/reauth 链路；IAM 未接（`docs/production-readiness.md` B3）。
- **阻塞**：需要认证/解锁命令与"草稿保留"的会话语义；spec 032 要求状态恢复。
- **下一步**：随 IAM 方案一起定；UI 先不假装能恢复，只把"会丢失"说清楚（UX-08）。

### B3 主管授权队列 badge

- **现状**：没有授权队列的查询/审批命令/DTO；主管端只有菜单项，没有数量与列表。
- **阻塞**：需要新契约（队列、计数、审批结果）。
- **下一步**：写 changes 提案；UI 等接口，不用 mock 数量假装实时。

### B4 顶栏业务日期/主机在线

- **现状**：`BootInfo` 只有启动时 `offline`；`SessionState` 只有 locked/reason/idle；没有业务日期与实时主机健康。
- **阻塞**：需要契约字段或健康检查命令；不要用前端时间冒充"业务日期"。
- **下一步**：先显示已有的 `bootInfo.offline`/本地包状态；业务日期/在线状态另写提案。

### B5 对账 UTC 日界修正

- **现状**：`start_of_day` 用 UTC 日界（`docs/production-readiness.md:28`），跨时区网点"今天"会错；UI 文案却写本地 00:00。
- **阻塞**：Rust 核心改动 + 远程构建验证；不是纯前端。
- **下一步**：UX-16 先让 UI 显示真实 `since`；核心修正单独立项，走 `scripts/remote-build.sh`。

### B6 凭证要素/打印预览

- **现状**：`printVoucher` 只传 `amount/account`（`TxPanel.vue:91-96`）；`PrintReceipt` 只有 `job_id/pages`；没有预览。
- **阻塞**：凭证模板要素（网点/柜员/时间/流水号/大写金额）与预览能力需要产品或契约确认。
- **下一步**：先确认打印模板契约，再改字段；不要在 UI 里拼假凭证。

### B7 异常明细字段（金额/时间/账号）

- **现状**：`TxException` 只有 `{ key, kind }`（`generated/TxException.ts`）；异常列表无法显示金额/时间/账号。
- **阻塞**：需要扩展契约或新增查询命令；不能靠前端猜。
- **下一步**：写 changes 提案；UX-11 先做数量与确认，不做明细。

---

## 新会话怎么接手

1. 读根 `AGENTS.md`、`apps/shell/AGENTS.md`、`apps/shell/src/AGENTS.md`，再读本文与对应条目。
2. 选第一个依赖已满足的 `pending` 条目，把状态改成 `doing`（只改这一行）。
3. 先复现证据（一条命令或一次 DOM 检查）。复现不了就停，更新证据，不要硬做。
4. 按"改哪里/做什么"做最小 diff；一次只碰一个文件/页面；超过 300 行拆。
5. 跑 `npm run build`；最终以 `npm run gate` 的 exit code 为准（node 部分；不跑 cargo）。
6. 布局类改动补 1100×700 与 1366×768 截图；把测量值写回条目。
7. 把状态改成 `done`，在条目末尾加一行"结果：PR/commit + 一句证据"。没做完就写 `blocked` + 具体原因。

## 验收口径（所有条目共用）

- **构建**：`npm run build` 退出 0（`vue-tsc --noEmit` + `vite build`）。
- **门禁**：`npm run gate` 退出 0 是唯一裁决（node 部分；不跑 cargo）；只改样式时也至少 `npm run build` 退出 0。不把"记录存在"当"覆盖完整"。
- **布局**：1100×700（最小窗）与 1366×768（常见柜面机）都不出现空列、横向滚动、关键操作在折叠线下。
- **文案**：柜员可见处不出现内部术语；错误必须是人话；不承诺未验证的能力。
- **可访问性**：键盘可达、焦点可见、状态可播报；对比度 ≥4.5:1。
- **不做**：不改只读区；不新增依赖/第二应用；不伪造数据；不擅自扩范围。
