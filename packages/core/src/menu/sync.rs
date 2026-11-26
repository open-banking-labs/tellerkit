//! 启动与同步编排：把"本地包优先、网络兜底、版本探测"写成可测的纯逻辑。
//!
//! 命令层（`src-tauri/src/commands.rs`）只负责加锁和翻译错误，真正的决策在这里，
//! 因此这些路径可以在没有 GUI 的环境里被完整测到。

use crate::banking::MenuSource;
use crate::menu::pack;
use crate::menu::store::{LoadSource, LoadedMenu, MenuStore, StoreError};

/// 启动结果。
#[derive(Debug, Clone)]
pub struct BootOutcome {
    loaded: LoadedMenu,
    /// 给用户看的说明（首次联网、验签失败回退等）。
    notes: Vec<String>,
}

impl BootOutcome {
    pub fn loaded(&self) -> &LoadedMenu {
        &self.loaded
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// 交给上层接管菜单树时一次性取出，避免为所有权克隆整棵树。
    pub fn into_parts(self) -> (LoadedMenu, Vec<String>) {
        (self.loaded, self.notes)
    }
}

/// 重新加载结果，额外暴露"是否发生了验签失败"。
#[derive(Debug, Clone)]
pub struct ReloadOutcome {
    loaded: LoadedMenu,
    verify_failed: bool,
    notes: Vec<String>,
}

impl ReloadOutcome {
    pub fn loaded(&self) -> &LoadedMenu {
        &self.loaded
    }

    pub fn verify_failed(&self) -> bool {
        self.verify_failed
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// 交给上层接管菜单树时一次性取出。
    pub fn into_loaded(self) -> LoadedMenu {
        self.loaded
    }
}

/// 版本探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    checked: bool,
    local: Option<u64>,
    latest: Option<u64>,
    updated: bool,
    note: String,
}

impl UpdateOutcome {
    pub fn checked(&self) -> bool {
        self.checked
    }

    pub fn local(&self) -> Option<u64> {
        self.local
    }

    pub fn latest(&self) -> Option<u64> {
        self.latest
    }

    pub fn updated(&self) -> bool {
        self.updated
    }

    pub fn note(&self) -> &str {
        &self.note
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BootError {
    #[error("本地没有可用菜单包，且无法从服务端获取（{0}）。请先联网启动一次。")]
    Offline(String),
    #[error("服务端菜单包安装失败：{0}")]
    Install(String),
}

fn fetch(
    host: &dyn MenuSource,
    store: &MenuStore,
    notes: &mut Vec<String>,
) -> Result<LoadedMenu, BootError> {
    let bytes = host
        .download_menu()
        .map_err(|e| BootError::Offline(e.to_string()))?;
    let version = pack::peek_version(&bytes)
        .ok_or_else(|| BootError::Install("服务端下发的包结构非法（头部无法解析）".to_string()))?;
    let tree = store
        .install(&bytes)
        .map_err(|e| BootError::Install(e.to_string()))?;
    notes.push(format!("已从服务端安装菜单包 v{version}"));
    Ok(LoadedMenu::new(
        tree,
        LoadSource::NetworkFallback { version },
        bytes.len(),
    ))
}

/// 重新加载：优先本地，失败则回退服务端。`verify_failed` 表示本地包验签没过。
pub fn reload(host: &dyn MenuSource, store: &MenuStore) -> Result<ReloadOutcome, BootError> {
    let mut notes = Vec::new();
    match store.load_latest() {
        Ok(loaded) => Ok(ReloadOutcome {
            loaded,
            verify_failed: false,
            notes,
        }),
        Err(err) => {
            let verify_failed = matches!(err, StoreError::Pack(_));
            notes.push(match &err {
                StoreError::Pack(e) => format!("本地菜单包验签失败（{e}），已回退服务端"),
                StoreError::NoPack => "本地无菜单包：首次启动需要联网一次".to_string(),
                other => format!("本地菜单包不可用（{other}），已回退服务端"),
            });
            let loaded = fetch(host, store, &mut notes)?;
            Ok(ReloadOutcome {
                loaded,
                verify_failed,
                notes,
            })
        }
    }
}

/// 启动路径：本地命中时**完全不碰网络**。
pub fn boot_load(host: &dyn MenuSource, store: &MenuStore) -> Result<BootOutcome, BootError> {
    let outcome = reload(host, store)?;
    Ok(BootOutcome {
        loaded: outcome.loaded,
        notes: outcome.notes,
    })
}

/// 版本探测 + 按需静默安装。
///
/// 注意：这里**不**清空任何会话状态——菜单版本与权限版本互相独立。
pub fn check_and_update(host: &dyn MenuSource, store: &MenuStore) -> UpdateOutcome {
    let local = store.current_version();
    let probe = match host.probe_menu() {
        Ok(probe) => probe,
        Err(err) => {
            return UpdateOutcome {
                checked: false,
                local,
                latest: None,
                updated: false,
                note: format!("版本探测不可用（{err}），继续使用本地菜单"),
            }
        }
    };

    if local == Some(probe.latest()) {
        return UpdateOutcome {
            checked: true,
            local,
            latest: Some(probe.latest()),
            updated: false,
            note: format!("菜单已是最新（v{}）", probe.latest()),
        };
    }

    let bytes = match host.download_menu() {
        Ok(bytes) => bytes,
        Err(err) => {
            return UpdateOutcome {
                checked: true,
                local,
                latest: Some(probe.latest()),
                updated: false,
                note: format!("发现新版本但下载失败（{err}），继续使用本地菜单"),
            }
        }
    };

    match store.install(&bytes) {
        Ok(_) => UpdateOutcome {
            checked: true,
            local,
            latest: Some(probe.latest()),
            updated: true,
            note: format!("已静默更新到菜单 v{}", probe.latest()),
        },
        Err(err) => UpdateOutcome {
            checked: true,
            local,
            latest: Some(probe.latest()),
            updated: false,
            note: format!("新菜单包校验失败，已拒绝安装：{err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{demo_verifying_key, MockHost};

    fn store_for(dir: &std::path::Path) -> MenuStore {
        MenuStore::new(dir.join("menu"), demo_verifying_key())
    }

    #[test]
    fn first_run_online_fetches_and_installs() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        let host = MockHost::new(7);

        let outcome = boot_load(&host, &store).unwrap();
        assert_eq!(
            *outcome.loaded().source(),
            LoadSource::NetworkFallback { version: 7 }
        );
        assert!(outcome.notes.iter().any(|n| n.contains("已从服务端安装")));
        assert_eq!(store.current_version(), Some(7));
    }

    #[test]
    fn first_run_offline_fails_with_actionable_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        let host = MockHost {
            offline: true,
            ..MockHost::new(7)
        };
        let err = boot_load(&host, &store).unwrap_err();
        assert!(matches!(err, BootError::Offline(_)));
        assert!(err.to_string().contains("请先联网启动一次"));
        assert_eq!(store.current_version(), None);
    }

    #[test]
    fn offline_boot_uses_local_pack_without_touching_network() {
        // P0 验收第 1 条：断网冷启动必须可用。
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let host = MockHost {
            offline: true,
            ..MockHost::new(99) // 服务端版本更高也没关系，断网就不该去问
        };
        let outcome = boot_load(&host, &store).unwrap();
        assert_eq!(
            *outcome.loaded().source(),
            LoadSource::LocalFile { version: 7 }
        );
        assert!(outcome.notes.is_empty(), "命中的本地包不应产生任何提示");
    }

    #[test]
    fn corrupted_local_pack_falls_back_to_network() {
        // P0 验收第 4 条：篡改 → 验签失败 → 回退服务端。
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let path = store.pack_path(7);
        let mut raw = std::fs::read(&path).unwrap();
        let idx = raw.len() / 2;
        raw[idx] ^= 0xFF;
        std::fs::write(&path, raw).unwrap();

        let outcome = reload(&MockHost::new(7), &store).unwrap();
        assert!(outcome.verify_failed);
        assert_eq!(
            *outcome.loaded().source(),
            LoadSource::NetworkFallback { version: 7 }
        );
        assert!(outcome.notes.iter().any(|n| n.contains("验签失败")));
    }

    #[test]
    fn update_is_skipped_when_versions_match() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let outcome = check_and_update(&MockHost::new(7), &store);
        assert!(outcome.checked);
        assert!(!outcome.updated);
        assert_eq!(outcome.note, "菜单已是最新（v7）");
    }

    #[test]
    fn newer_server_version_is_installed_silently() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let outcome = check_and_update(&MockHost::new(8), &store);
        assert!(outcome.updated);
        assert_eq!(outcome.latest, Some(8));
        assert_eq!(store.current_version(), Some(8));
    }

    #[test]
    fn update_fails_gracefully_when_offline() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let host = MockHost {
            offline: true,
            ..MockHost::new(8)
        };
        let outcome = check_and_update(&host, &store);
        assert!(!outcome.checked);
        assert!(!outcome.updated);
        assert!(outcome.note.contains("版本探测不可用"));
        assert_eq!(store.current_version(), Some(7), "断网不得破坏本地包");
    }

    #[test]
    fn tampered_download_is_rejected_and_local_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        store
            .install(&MockHost::new(7).download_menu().unwrap())
            .unwrap();

        let host = MockHost {
            serve_tampered: true,
            ..MockHost::new(8)
        };
        let outcome = check_and_update(&host, &store);
        assert!(!outcome.updated);
        assert!(outcome.note.contains("校验失败"));
        assert_eq!(store.current_version(), Some(7), "坏包不得覆盖好包");
    }

    #[test]
    fn reload_reports_no_pack_for_empty_dir_online() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_for(dir.path());
        let outcome = reload(&MockHost::new(7), &store).unwrap();
        assert!(!outcome.verify_failed);
        assert!(outcome.notes.iter().any(|n| n.contains("本地无菜单包")));
    }
}
