//! 本地菜单包仓库：落盘、验签加载、原子替换、版本保留与回滚。
//!
//! 目录布局（应用数据目录下）：
//!
//! ```text
//! <appdata>/
//!   menu/
//!     menu.7.mtree        当前生效
//!     menu.6.mtree        N-1，回滚用
//!     menu.8.mtree.tmp    写入中（绝不直接被读取）
//! ```
//!
//! 关键约束：
//! - 只认 `menu.<version>.mtree` 这个命名，`.tmp` 不会被当成有效包；
//! - 写入走 `tmp -> fsync -> rename`，断电不会留下半个文件；
//! - 加载时**先验签再解码**，任何一步失败都不允许使用该文件。

use crate::menu::model::MenuTree;
use crate::menu::pack::{self, MenuPack, PackError};
use ed25519_dalek::VerifyingKey;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const PREFIX: &str = "menu.";
const SUFFIX: &str = ".mtree";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    /// 命中本地文件，版本号一致。
    LocalFile { version: u64 },
    /// 本地缺失 / 验签失败，回退到服务端下发。
    NetworkFallback { version: u64 },
}

#[derive(Debug, Clone)]
pub struct LoadedMenu {
    tree: MenuTree,
    source: LoadSource,
    bytes: usize,
}

impl LoadedMenu {
    pub fn new(tree: MenuTree, source: LoadSource, bytes: usize) -> Self {
        Self {
            tree,
            source,
            bytes,
        }
    }

    pub fn tree(&self) -> &MenuTree {
        &self.tree
    }

    pub fn source(&self) -> &LoadSource {
        &self.source
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// 上层接管菜单树时一次性取出，避免克隆整棵树。
    pub fn into_tree(self) -> MenuTree {
        self.tree
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("本地没有可用的菜单包")]
    NoPack,
    #[error("菜单包校验失败: {0}")]
    Pack(#[from] PackError),
    #[error("文件操作失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("目录 {0} 不存在且无法创建")]
    BadDir(String),
}

/// 本地菜单包仓库。`dir` 由调用方传入（Tauri 侧传应用数据目录）。
#[derive(Debug, Clone)]
pub struct MenuStore {
    dir: PathBuf,
    pubkey: VerifyingKey,
    /// 保留多少个历史版本（默认 2：当前 + N-1）。
    keep: usize,
}

impl MenuStore {
    pub fn new(dir: impl Into<PathBuf>, pubkey: VerifyingKey) -> Self {
        Self {
            dir: dir.into(),
            pubkey,
            keep: 2,
        }
    }

    pub(crate) fn with_keep(mut self, keep: usize) -> Self {
        self.keep = keep.max(1);
        self
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn pubkey(&self) -> &VerifyingKey {
        &self.pubkey
    }

    fn ensure_dir(&self) -> Result<(), StoreError> {
        if self.dir.exists() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .map_err(|_| StoreError::BadDir(self.dir.display().to_string()))
    }

    /// 某个版本对应的文件路径（诊断/演示用）。
    pub fn pack_path(&self, version: u64) -> PathBuf {
        self.path_for(version)
    }

    fn path_for(&self, version: u64) -> PathBuf {
        self.dir.join(format!("{PREFIX}{version}{SUFFIX}"))
    }

    /// 列出本地所有有效命名的包，按版本号倒序。
    pub fn list_versions(&self) -> Vec<u64> {
        let mut versions: Vec<u64> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                let v = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
                v.parse::<u64>().ok()
            })
            .collect();
        versions.sort_unstable_by(|a, b| b.cmp(a));
        versions
    }

    /// 加载最新版本。验签失败会返回 `StoreError::Pack`，由调用方决定回退策略。
    pub fn load_latest(&self) -> Result<LoadedMenu, StoreError> {
        let versions = self.list_versions();
        let Some(&version) = versions.first() else {
            return Err(StoreError::NoPack);
        };
        self.load_version(version)
    }

    /// 加载指定版本（回滚场景）。
    pub(crate) fn load_version(&self, version: u64) -> Result<LoadedMenu, StoreError> {
        let path = self.path_for(version);
        let bytes = fs::read(&path)?;
        let tree = pack::open(&bytes, &self.pubkey)?;
        Ok(LoadedMenu::new(
            tree,
            LoadSource::LocalFile { version },
            bytes.len(),
        ))
    }

    /// 验签后原子安装。返回解码后的树，便于调用方立即使用。
    ///
    /// 安装前先验签：坏包绝不落到正式文件名上。
    pub(crate) fn install(&self, bytes: &[u8]) -> Result<MenuTree, StoreError> {
        let pack = MenuPack::verify(bytes, &self.pubkey)?;
        let tree = pack.decode()?;
        self.ensure_dir()?;

        let final_path = self.path_for(pack.menu_version());
        let tmp_path = self
            .dir
            .join(format!("{PREFIX}{}{SUFFIX}.tmp", pack.menu_version()));

        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &final_path)?;

        // 目录 fsync：确保 rename 落盘（macOS 上不影响正确性，Linux 上必要）。
        if let Ok(dir) = fs::File::open(&self.dir) {
            let _ = dir.sync_all();
        }

        self.prune();
        Ok(tree)
    }

    /// 只保留最近 `keep` 个版本。
    pub(crate) fn prune(&self) {
        let versions = self.list_versions();
        for old in versions.into_iter().skip(self.keep) {
            let _ = fs::remove_file(self.path_for(old));
        }
    }

    /// 清理遗留的 `.tmp` 文件（上次写入被中断时可能残留）。
    pub fn clean_tmp(&self) {
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().ends_with(".mtree.tmp") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    /// 当前生效版本号（不验签，仅供 UI 显示与版本探测比对）。
    pub fn current_version(&self) -> Option<u64> {
        self.list_versions().first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{demo_signing_key, demo_tree, demo_verifying_key, MockHost};

    fn store() -> (tempfile::TempDir, MenuStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MenuStore::new(dir.path().join("menu"), demo_verifying_key());
        (dir, store)
    }

    #[test]
    fn empty_dir_reports_no_pack() {
        let (_d, store) = store();
        assert!(matches!(store.load_latest(), Err(StoreError::NoPack)));
        assert_eq!(store.current_version(), None);
    }

    #[test]
    fn install_then_load_roundtrip() {
        let (_d, store) = store();
        let host = MockHost::new(7, false, false);
        let bytes = host.download().unwrap();
        let installed = store.install(&bytes).unwrap();
        assert_eq!(installed, demo_tree(7));

        let loaded = store.load_latest().unwrap();
        assert_eq!(loaded.tree, demo_tree(7));
        assert_eq!(loaded.source, LoadSource::LocalFile { version: 7 });
        assert_eq!(store.current_version(), Some(7));
    }

    #[test]
    fn tampered_file_on_disk_is_rejected() {
        let (_d, store) = store();
        let bytes = MockHost::new(7, false, false).download().unwrap();
        store.install(&bytes).unwrap();

        // 模拟被十六进制编辑器改过
        let path = store.path_for(7);
        let mut raw = fs::read(&path).unwrap();
        let idx = raw.len() / 2;
        raw[idx] ^= 0x01;
        fs::write(&path, &raw).unwrap();

        assert!(matches!(
            store.load_latest(),
            Err(StoreError::Pack(PackError::SignatureInvalid))
        ));
    }

    #[test]
    fn install_rejects_tampered_bytes_without_writing() {
        let (_d, store) = store();
        let mut bytes = MockHost::new(7, false, false).download().unwrap();
        let idx = bytes.len() / 2;
        bytes[idx] ^= 0xFF;
        assert!(store.install(&bytes).is_err());
        assert!(
            store.list_versions().is_empty(),
            "坏包不允许以正式文件名落盘"
        );
    }

    #[test]
    fn prune_keeps_only_latest_two() {
        let (_d, store) = store();
        for v in [5u64, 6, 7, 8] {
            let bytes = MockHost::new(v, false, false).download().unwrap();
            store.install(&bytes).unwrap();
        }
        assert_eq!(store.list_versions(), vec![8, 7]);
        assert_eq!(store.current_version(), Some(8));
    }

    #[test]
    fn rollback_to_previous_version_is_possible() {
        let (_d, store) = store();
        store
            .install(&MockHost::new(7, false, false).download().unwrap())
            .unwrap();
        store
            .install(&MockHost::new(8, false, false).download().unwrap())
            .unwrap();

        let old = store.load_version(7).unwrap();
        assert_eq!(old.tree.menu_version(), 7);
    }

    #[test]
    fn tmp_files_are_ignored_and_cleanable() {
        let (_d, store) = store();
        store.ensure_dir().unwrap();
        fs::write(store.dir.join("menu.9.mtree.tmp"), b"partial").unwrap();
        assert_eq!(store.list_versions(), Vec::<u64>::new());
        assert!(matches!(store.load_latest(), Err(StoreError::NoPack)));

        store.clean_tmp();
        assert!(!store.dir.join("menu.9.mtree.tmp").exists());
    }

    #[test]
    fn wrong_pubkey_rejects_valid_pack() {
        let dir = tempfile::tempdir().unwrap();
        use rand::rngs::OsRng;
        let other = ed25519_dalek::SigningKey::generate(&mut OsRng);
        let store = MenuStore::new(dir.path().join("menu"), other.verifying_key());
        let bytes = MockHost::new(7, false, false).download().unwrap();
        assert!(matches!(
            store.install(&bytes),
            Err(StoreError::Pack(PackError::SignatureInvalid))
        ));
    }

    #[test]
    fn signing_key_is_deterministic_for_demo() {
        // 演示公钥必须跨进程稳定，否则客户端内置公钥与下发的包对不上。
        let a = pack::pubkey_to_hex(&demo_verifying_key());
        let b = pack::pubkey_to_hex(&demo_signing_key().verifying_key());
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}
