# OCR 原生评审附录（2026-11-26）

原生流水线输出，原文保留（英文），仅去除内部 `thinking` 字段。

## 运行元数据

| 项 | 值 |
|---|---|
| 目标 | `--commit 2a744a2`（vs `155e0fa`） |
| 模型 | `tencent-cloud/hy4-flash`（magpie `127.0.0.1:3425`） |
| 覆盖 | 45 / 73 个变更文件 |
| 结果 | 20 条 finding，complete |
| 耗时 / token | 12m53s / 13,220,722 |
| 原始 JSON | `/tmp/tellerkit-ocr/head.json` |

> 工具链修复：本机默认 `git`（2.56.0，`/usr/local`）未编译 PCRE，OCR 的 `git grep -P` 全部失败；改用 Apple `/usr/bin/git`（2.50.1，含 PCRE）的 PATH 垫片后完成。

## 影响分析（原文）

This commit makes the desktop shell fail closed in release builds and adds a real Rust-side session gate on all money/print/reversal/replay/package entry points; most of the remaining diff is a field-privacy/facade refactor that leaves IPC contracts intact. The compatibility cost is concentrated in deployment: release now needs a build-injected verification key, signed+TLS endpoint config, and can no longer silently become a demo terminal.

## Behavior changes

| Scenario | Before | After |
|---|---|---|
| Release startup with `endpoints.json` missing/invalid/unsigned/`http://` | `build_remote` fell back to `MockRemote`, printed a note and booted normally into a fake backend | `bootstrap::resolve_remote` returns `Err`; `build_remote` sets `AppState.startup_fatal`; `lock()` rejects **every** command including `boot`, so the front-end shows `启动被拒绝：…` on its error page. The mock is still constructed but unreachable |
| Bad env flag, e.g. `TK_REQUIRE_TLS=maybe` | Lenient parse (`v=="1"\|\|eq("true")`) treated it as off, i.e. fail-open with no TLS enforcement | `env_flag` returns an error naming the variable; startup is refused in **both** debug and release |
| Release build without `TK_MENU_PUBKEY_HEX` | Build succeeded; `MenuStore` and `pkg::scan_dir` used `demo_verifying_key()` | `build.rs` panics on `PROFILE=release`; if bypassed, `menu_verifying_key()` refuses to use a demo key in release |
| Locked / logged-out session calls submit, replay, reversal, print or package module | No session check; calls reached `TxEngine`/host. The background replay worker only skipped when `snapshot == None`, so a *locked but still logged-in* terminal kept auto-replaying | `require_unlocked` denies with `尚未登录`/`会话已锁定`, writes a `denied` audit event and never touches the host; the worker is gated the same way |
| Journal write / readback failure | Stored in `TxEngine.journal_error` and surfaced once via `take_journal_error()` in `replay_pending` as a `replay/failed` audit event | Field and method removed; write errors are appended into `TxResult.message` (and cached); startup readback failure is only `eprintln!` — no audit record at all |

### Compatibility cost of the release gate

Deployment must now ship `endpoints.json` + `.sig`, set `TK_CONFIG_PUBKEY`, and inject `TK_MENU_PUBKEY_HEX` at build time; otherwise release refuses to start or fails to build. This is deliberate policy, not an accidental regression. Menu and business packages are also verified with the injected key instead of the demo key (`boot()` passes `self.state.verifying_key` to `pkg::scan_dir` and `MenuStore`, and reports that key as `pubkey_hex`), so demo-signed packages will be skipped in release. Debug keeps the old fallback-with-note behavior.

### Consequence of the session-gate implementation

The replay worker polls every 60s by default. Previously it skipped silently when logged out; now every tick while logged out or locked calls `replay_pending()` and writes a `denied` audit event with actor `unknown` (the code comment explicitly accepts this: "skip 已留痕"). Pending transactions consequently wait until the terminal is unlocked. This is the intended tradeoff, but it produces recurring audit noise before login.

### Refactor scope and contracts

Core submodules `filter/model/pack/store/sync` moved under a private `menu/` module and are re-exported from the crate root; DTO fields became private with `new()`/accessors. Public symbol names from `tellerkit_core` are preserved and `check-contracts.mjs` was loosened to accept private fields, consistent with "contracts unchanged"; no `contracts/` file appears in the change inventory. Two background themes are **not** evidenced in this comparison: commands remain synchronous `pub fn` under `Mutex` (no `async fn`), and no audit hash chain exists in `journal-sqlite`.

## Verification note

Unit assertions for the new behavior exist in source — `bootstrap.rs` policy/resolve tests, `lib.rs` key-resolution tests, `app.rs` locked/missing/unlocked session tests, `tx.rs` idempotency tests — but **none were executed here** (no cargo/gate run). Release-mode specifics (`cfg!(debug_assertions)`, the `build.rs` panic), the ts-rs export of private fields and `generated/` consistency therefore remain unverified. The front-end error display was inspected, not run: `init()` catches `boot()` rejection and renders `fatal` on the login page.

## Pre-merge maintainer decisions

- **Acceptance check — release fail-closed path.** With a release artifact, vary `TK_MENU_PUBKEY_HEX` (present/absent) and `endpoints.json` (absent/unsigned/plaintext) and confirm `boot()` returns `启动被拒绝：…` and the page shows it. Source tests cover the decisions but were not executed.
- **Acceptance check — journal-failure visibility.** Since `take_journal_error` was removed, corrupt the journal before startup and force a write failure; the pass condition is that a failed read/write is still observable to ops (audit tail or startup note). Decide whether stderr-only is acceptable for a bank audit trail.
- **Maintainer choice — key rollout.** Confirm the build pipeline can inject `TK_MENU_PUBKEY_HEX` and that demo-signed menu/biz packages can be re-signed. Keeping the demo key in release would defeat the gate, so the recommended option is to proceed with injection.


## 逐条 finding（原文，按 OCR 顺序）

### 1. [low] bug — `tools/check-module-depth.mjs:41-44`

The pub-field scan only recognizes braced structs. `pub struct StartupFatal(pub Option<String>);` in `apps/shell/src-tauri/src/bootstrap.rs:47` is a public tuple field and is invisible to both the baseline report and `--strict`, i.e. exactly the field leak Article 11 targets. Add a tuple-struct branch to the parser. (`--strict` is not wired into `npm run gate` yet, so today this is a false-green report rather than a CI break.)

```rust
const structs = new Map();
for (const f of files) {
  const text = stripTest(f.text);
  const re = /pub struct (\w+)\s*\{([\s\S]*?)^\}/gm;
```

### 2. [low] other — `tools/check-bank-names.mjs:20-20`

Dead import: `rel` is never referenced in this file (only `ROOT` is used, at lines 22, 87 and 108). This will trip no-unused-vars linting in the gate tooling. Remove `rel` from the import, e.g. `import { ROOT } from "./paths.mjs";`.

```rust
import { ROOT, rel } from "./paths.mjs";
```

### 3. [low] maintainability — `packages/core/src/lib.rs:24-28`

The crate-root re-export publishes `reload()` but omits its return type `ReloadOutcome`. Because `menu` is now a private module, downstream crates can no longer name this type (previously reachable as `tellerkit_core::sync::ReloadOutcome`), so they cannot use it in wrapper signatures, struct fields, or trait impls—only via type inference at the call site. Include `ReloadOutcome` in this re-export list (or stop re-exporting `reload()`) to keep the public API self-consistent.

```rust
pub use menu::{
    boot_load, check_and_update, filter_visible, open, pubkey_from_hex, pubkey_to_hex, reload,
    BootError, BootOutcome, LoadSource, LoadedMenu, MenuNode, MenuPack, MenuStore, MenuTree,
    MenuView, NodeStatus, NodeType, PackError, StoreError, TreeError, UpdateOutcome, SIGNATURE_LEN,
};
```

### 4. [medium] security — `.githooks/pre-commit:5-5`

This hook delegates to a checker that reads working-tree files (`git ls-files --cached --others --exclude-standard` + `readFileSync`), not the staged blobs. A file containing a bank name can be staged, then sanitized in the working tree; the hook will pass and the staged (offending) content still enters the commit. Since this is described as the last line of defense before history, it must validate the index content (e.g., iterate `git diff --cached --name-only -z` and read `git show :<path>`, or pass staged content to the checker). In its current form the hook's guarantee is bypassable even though CI's gate would later catch it.

```rust
exec node tools/check-bank-names.mjs
```

### 5. [medium] other — `package.json:17-17`

`check-module-depth.mjs` runs in observation mode unless invoked with `--strict`: its own code only builds failures when `process.argv.includes("--strict")`. Wiring it into the mandatory gate without that flag means this new gate step can never fail on pure `pub` fields or files exceeding the `pub fn` limit — it always exits 0. For a 'deep modules / narrow interfaces' refactor this produces a false green. Either pass `--strict` now that the baseline is (presumably) clean, or keep the observation-only run out of `gate` and expose it via the `depth` script so the gate only contains enforcing checks.

```rust
"gate": "node tools/check-boundaries.mjs && node tools/check-contracts.mjs && node tools/check-tasks.mjs && node tools/check-ai-rules.mjs && node tools/check-trajectory.mjs && node tools/check-bank-names.mjs && node tools/check-version.mjs && node tools/check-module-depth.mjs && cargo fmt --all -- --check",
```

### 6. [medium] security — `tools/check-boundaries.mjs:71-77`

The new regex still requires at least one `module::` segment before the call. A direct import (`use tellerkit_core::boot_load;` followed by `boot_load(&state, ...)` in commands.rs) evades the gate, even though the core root re-exports all these functions and this change explicitly aimed to close import-based bypasses. Make the path prefix optional (and tolerate optional turbofish) or separately reject imports of these symbols in the command layer.

```rust
].map(([name, why]) => [
  name === "ensure_allowed"
    ? new RegExp(`\\.${name}\\s*\\(`)
    : new RegExp(`(?:^|[^A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)+${name}\\s*\\(`),
  name,
  why,
]);
```

### 7. [medium] security — `tools/check-ai-rules.mjs:91-96`

This removes the only automated assertion for the security-boundary wording and the ten constitution headings. A PR that deletes e.g. `服务端对每一次请求独立鉴权` now passes every gate, while `docs/constitution.md` line 40 still claims check-ai-rules verifies that phrase's existence. The 'phrase checks reward stale docs' rationale calls for a stronger check (registered hash of reviewed docs, heading + phrase in a ledger), not for dropping the check; if authenticity is delegated to human review, the constitution's claimed check should be updated in the same change.

```rust
// 6/7. 文档存在性只查文件在不在，不查标题/短语是否齐全。
// 短语存在性检查奖励陈旧文档：内容写错但只要关键词还在，门禁照样绿（架构评审 D-02）。
// 文档的真实性由项目级台账与人工评审负责，不在这道静态门禁里自欺。
for (const doc of ["docs/constitution.md", "docs/security-boundary.md"]) {
  if (!existsSync(join(ROOT, doc))) errors.push(`缺少 ${doc}`);
}
```

### 8. [medium] bug — `tools/check-trajectory.mjs:28-35`

Errors collected here are discarded on the early-success path: when `.agents/dispatches` is absent, invalid registry JSON or stale keys still print `通过` and exit 0. Also, `JSON.parse` may produce `null`, in which case `Object.keys(superseded)` throws a raw TypeError instead of a clear gate failure. Move the `if (!existsSync(DIR))` early return after the `errors.length` check, and verify `superseded` is a non-null object before use (e.g. reject arrays/null in the parse branch).

```rust
let superseded = {};
if (existsSync(SUPERSEDED)) {
  try {
    superseded = JSON.parse(readFileSync(SUPERSEDED, "utf8"));
  } catch {
    errors.push(".agents/dispatch-superseded.json 不是合法 JSON");
  }
}
```

### 9. [medium] bug — `packages/core/src/tx.rs:370-372`

Appending a storage failure to `TxResult.message` *before* the idempotency insert makes a transient journal error part of the permanent business result. Later duplicate submissions return `重复提交：返回首次结果（...（记账失败：...））` even after the DB is healthy again, and the raw DB error text is rendered directly in TxPanel.vue. Cache a clean business result and surface operational errors through a separate channel (or at least keep them out of the idempotency record).

```rust
if let Some(journal) = self.journal.as_deref() {
            if let Err(e) = journal.record(&request, &result) {
                result.message.push_str(&format!("（记账失败：{e}）"));
```

### 10. [medium] bug — `packages/core/src/replay.rs:420-420`

`replay_one` removes the request from the in-memory queue before submitting; if `journal.dequeue` fails it only appends the error to `TxResult.message`. Treating `Committed` as unqualified success here calls `record_success`, dropping the key from both memory and the scheduler while the durable `tx_queue` row remains. `ReplayRun` has no error field, so `ShellApp::replay_pending` (and the background worker) audit `replay/tx_queue/ok` and report success; after a restart the row is loaded and silently replayed again. Propagate journal failures into the run/audit and avoid recording success until dequeue actually succeeds.

```rust
if result.status() == TxStatus::Committed {
```

### 11. [medium] bug — `apps/shell/src-tauri/src/app.rs:575-575`

The background worker calls this every tick (default 60s) even before any login and when there is no pending work. Because `require_unlocked` is the first statement, every rejection writes a `denied` audit row with actor `unknown` — the old worker explicitly skipped when `snapshot.is_none()` for exactly this reason, and S-04 only asks for a skip audit when there are pending transactions. An unattended terminal at the login screen will accumulate ~1440 noise rows/day, obscuring real compliance events. Move the session check after the "no pending work" early return, or have the worker skip when there is no active session and only audit real skips.

```rust
self.require_unlocked("replay")?;
```

### 12. [medium] bug — `apps/shell/src-tauri/src/app.rs:585-587`

`take_journal_error()` was removed from `TxEngine`; core now appends ledger write failures to each `TxResult.message`, but `replay::run_once` only checks `result.status()` and discards the result/message. This function therefore has no path to surface or audit a journal failure that occurs during replay (e.g. a transaction committed to the host but not recorded locally), which was previously written as a `replay`/`tx_queue`/`failed` audit event. Surface those errors from `run_once` (or retain an explicit accessor) and audit them here, otherwise degraded replay processing becomes invisible to operators.

```rust
if run.attempted() == 0 && run.still_pending() == 0 {
            return Ok("没有待重放的交易".to_string());
        }
```

### 13. [medium] bug — `tools/check-module-depth.mjs:41-43`

The struct index is keyed only by the struct's identifier, so a later file with the same struct name silently overwrites the earlier entry. This repo already has such collisions: `StatusRow`/`Reconciliation` (app.rs vs commands.rs), `ReloadOutcome` (app.rs vs menu/sync.rs), `RecentTx` (journal-sqlite/src/lib.rs vs commands.rs). Consequences in `--strict` mode: (1) a `pub` field in the overwritten struct is invisible to the "target 0 pub fields" failure, so the gate can stay green on a real leak; (2) `fieldsByFile` loses that struct's field names, so matching getter/`set_` accessors are counted as interface width and can trip `PUB_FN_LIMIT` spuriously. Key the map by file + name (or store an array of structs), keep `name` on the value, and iterate `structs.values()` downstream.

```rust
const structs = new Map();
for (const f of files) {
  const text = stripTest(f.text);
```

### 14. [medium] security — `apps/shell/src-tauri/src/bootstrap.rs:78-88`

This drops the existing 'pubkey present implies signature required' default from `config_sig::require_signed()` / `LoadPolicy::from_env()`. In a debug build with `TK_CONFIG_PUBKEY` set but `TK_REQUIRE_SIGNED_CONFIG` unset, `require_signed` is now false, so `resolve_remote` loads an unsigned `endpoints.json` silently; before this refactor the same environment required a valid `.sig`. That is a fail-open regression on the very config that decides where transactions are routed. Base the debug default on `crate::config_sig::require_signed()` (or `LoadPolicy::from_env()`) and only force both flags on for release builds.

```rust
pub fn policy(
    release: bool,
    require_signed: Option<bool>,
    require_tls: Option<bool>,
) -> LoadPolicy {
    LoadPolicy::new(
        crate::config_sig::config_pubkey(),
        release || require_signed.unwrap_or(false),
        release || require_tls.unwrap_or(false),
    )
}
```

### 15. [medium] bug — `packages/core/src/device.rs:117-118`

Making the `PrintReceipt` fields private without adding any constructor makes the type impossible to construct outside this module. `Printer::print` is a public trait whose documented purpose is swapping in different device adapters ("换型号只换适配器", P3 real hardware), but an external adapter cannot return a `PrintReceipt` anymore — only this module's mocks can. `CapturedImage` has the same problem: private `path`/`mime` with no `new()` (and no getters), so `Scanner::capture` is likewise unimplementable from outside. Add `PrintReceipt::new(job_id, pages)` and `CapturedImage::new(path, mime)` (plus getters if consumers need them), or keep these DTO fields public as the contract shape requires.

```rust
job_id: String,
    pages: u32,
```

### 16. [medium] bug — `packages/core/src/device.rs:92-106`

The new `Voucher` accessors expose `template()`/`tx_code()` but leave `reference` and `fields` write-only. An external `Printer` adapter receives `&Voucher` and has to render the actual voucher content (amount, account, etc. live in `fields`, plus `reference`); after this privatization it cannot read either. The mock printer works only because it lives in the same module. Add `reference(&self) -> Option<&str>` and `fields(&self) -> &BTreeMap<String, String>` so the public trait remains implementable outside `device.rs`.

```rust
pub fn template(&self) -> &str {
        &self.template
    }

    pub fn tx_code(&self) -> &str {
        &self.tx_code
    }

    pub fn set_reference(&mut self, reference: Option<String>) {
        self.reference = reference;
    }

    pub fn set_fields(&mut self, fields: BTreeMap<String, String>) {
        self.fields = fields;
    }
```

### 17. [high] bug — `packages/core/src/tx.rs:309-311`

`SqliteJournal::pending()` can fail at runtime (e.g. ciphertext decryption or JSON parse error in the queue row). This branch then leaves `self.pending` empty, still installs the journal, and only writes to stderr. With `take_journal_error()` gone and `lib.rs::boot` only reporting `SqliteJournal::open` failures via `journal_warning`, there is no UI/audit signal at all: previously persisted pending transactions are never re-queued for replay and nobody is alerted. Make this failure observable (e.g. a typed error / sticky load error surfaced at boot or replay) or fail closed instead of treating the queue as empty — the adjacent comment explicitly says it must not be silent.

```rust
// 这里还没有 TxResult 可以挂提示，只能写诊断日志：
            // 读不回挂起队列不能静默当作空队列。
            Err(e) => eprintln!("读取待重放队列失败：{e}"),
```

### 18. [high] security — `apps/shell/src-tauri/src/commands.rs:269-270`

The new session gate can be bypassed through this command. `ShellApp::login` unconditionally overwrites `state.session` with a fresh, unlocked `SessionGuard` (and a new snapshot), and neither this command nor `login()` checks `require_unlocked`/current lock state. The frontend `LockScreen` only offers `@reauth="logout"`, so there is no legitimate UI path that logs in while locked — but any webview caller can invoke `invoke("login", {roleId: "supervisor"})` directly (role IDs are trivially guessable) and instantly unlock every protected command as any role. Since the gate claims Rust state is the only source of truth, login must also be gated: reject login while an active session is locked (force explicit logout/re-auth first), or require a separate Rust-side unlock authorization.

```rust
let outcome = ShellApp::new(&mut guard).login(&role_id)?;
    Ok(LoginResult {
```

### 19. [high] security — `apps/shell/src-tauri/src/endpoint_config.rs:232-232`

`require_tls` (forced on for release by `bootstrap::policy`) is only enforced for `url` fields with an `http://` prefix. A `tcp` spec has no `url`, so it is never checked; `build()` then constructs a `TcpTransport` that uses a bare `TcpStream` with no TLS at all. Because production contracts use TCP services (e.g. `acct.core`, `card.auth`), a signed release config can still send banking traffic in cleartext while startup reports that TLS is enforced. Reject `transport == "tcp"` when `require_tls` is true (or route TCP through a TLS-capable transport) so the fail-closed guarantee covers the actual production transport.

```rust
if let Some(url) = spec.url() {
```

### 20. [high] security — `apps/shell/src-tauri/src/app.rs:300-302`

The gate only reads the stored `locked` flag, which is set by `SessionGuard::evaluate` from `session_tick`/`session_blur` commands issued by the frontend. The comment says frontend ticks can be skipped, but if a page stops calling `session_tick` (or an iframe under the lock overlay keeps invoking commands), idle timeout never materializes: `is_locked()` stays false and fund/print/reversal/replay commands keep reaching the host indefinitely. Enforce the idle rule at the command boundary itself — evaluate the guard against `self.clock.now_secs()` inside `require_unlocked` (needs `&mut self`, so `package_module` must also become mutable), audit a newly acquired lock, then check `is_locked()`.

```rust
fn require_unlocked(&self, action: &str) -> Result<(), String> {
        match &self.state.session {
            Some(g) if !g.is_locked() => Ok(()),
```
