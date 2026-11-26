//! 业务包（`.bizpkg`）：签名容器 + 版本兼容 + 依赖拓扑 + 菜单合并。
//!
//! 设计沿用 `.mtree` 的教训：容器自己带版本，签名覆盖头部与载荷，
//! 结构非法直接拒绝。区别是业务包多了三件必须管住的事：
//!
//! 1. **不能劫持菜单**：包注入的节点不得覆盖已存在的 code；
//! 2. **不能自证来源**：加载时把 `source` 强制改写为 `pkg:<id>`；
//! 3. **不能越权**：包里声明的权限码只是"供给"，授权仍在服务端。

use crate::menu::model::{MenuNode, MenuTree, TreeError};
use crate::menu::pack::PackError;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const PKG_MAGIC: &[u8; 8] = b"MTKPKG01";
pub const PKG_FORMAT_VERSION: u16 = 1;
pub const PKG_HEADER_LEN: usize = 26;
pub const PKG_SIGNATURE_LEN: usize = 64;

/// 业务包清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BizManifest {
    id: String,
    name: String,
    version: String,
    /// 兼容的主壳版本下限（含）。
    min_shell: String,
    /// 兼容的主壳版本上限（含）。支持 `0.x` 这类主版本通配。
    max_shell: String,
    #[serde(default)]
    deps: Vec<String>,
    entry: String,
}

impl BizManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &str,
        name: &str,
        version: &str,
        min_shell: &str,
        max_shell: &str,
        deps: Vec<String>,
        entry: &str,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            min_shell: min_shell.into(),
            max_shell: max_shell.into(),
            deps,
            entry: entry.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn min_shell(&self) -> &str {
        &self.min_shell
    }

    pub fn max_shell(&self) -> &str {
        &self.max_shell
    }

    pub fn deps(&self) -> &[String] {
        &self.deps
    }

    pub fn entry(&self) -> &str {
        &self.entry
    }
}

/// 包内容。字段与 `packages/biz-demo/` 下的文件一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BizPackage {
    manifest: BizManifest,
    #[serde(default)]
    menus: Vec<MenuNode>,
    /// 本包**声明**能提供哪些权限码。授权仍在服务端，这里不产生任何权限。
    #[serde(default)]
    permissions: Permissions,
    #[serde(default)]
    routes: BTreeMap<String, String>,
    #[serde(default)]
    transactions: Vec<TxMeta>,
    /// 前端资源：包内路径 → 模块源码。
    ///
    /// 打包时把 `ui/*.js` 的内容嵌进来，是因为包在运行时是**外部文件**，
    /// 前端没法 `import("/path/to/ui/entry.js")`——只能由主壳把源码交给前端，
    /// 前端用 Blob URL 加载。代价是包体积变大，收益是不需要本地文件服务器。
    #[serde(default)]
    assets: BTreeMap<String, String>,
}

/// 权限声明。只有 actions：菜单 code 已经在 `menus` 里了，重复声明会两头不一致。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    actions: Vec<String>,
}

impl Permissions {
    pub fn new(actions: Vec<String>) -> Self {
        Self { actions }
    }

    pub fn actions(&self) -> &[String] {
        &self.actions
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxMeta {
    code: String,
    #[serde(default)]
    idempotency: String,
    #[serde(default)]
    offline_capable: bool,
    #[serde(default)]
    devices: Vec<String>,
}

impl TxMeta {
    pub fn new(code: &str, idempotency: &str, offline_capable: bool, devices: Vec<String>) -> Self {
        Self {
            code: code.into(),
            idempotency: idempotency.into(),
            offline_capable,
            devices,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn idempotency(&self) -> &str {
        &self.idempotency
    }

    pub fn offline_capable(&self) -> bool {
        self.offline_capable
    }

    pub fn devices(&self) -> &[String] {
        &self.devices
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PkgError {
    #[error("文件长度不足，可能被截断")]
    TooShort,
    #[error("magic 不匹配，不是 .bizpkg 文件")]
    BadMagic,
    #[error("不支持的包格式版本: {0}")]
    UnsupportedFormatVersion(u16),
    #[error("payload 长度与头部声明不一致")]
    PayloadLenMismatch,
    #[error("签名校验失败：业务包已损坏或被篡改")]
    SignatureInvalid,
    #[error("载荷解析失败: {0}")]
    DecodeFailed(String),
    #[error("包 ID 非法: {0}（至少两段，如 biz.demo）")]
    BadPackageId(String),
    #[error("版本号非法: {0}")]
    BadVersion(String),
    #[error("菜单树结构非法: {0}")]
    InvalidTree(#[from] TreeError),
    #[error("业务包菜单节点必须声明 parent_code 或使用命名空间前缀: {0}")]
    UnnamespacedMenu(String),
}

// ---------------- 版本 ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn major(&self) -> u64 {
        self.major
    }

    pub fn minor(&self) -> u64 {
        self.minor
    }

    pub fn patch(&self) -> u64 {
        self.patch
    }

    /// 只接受 `major.minor.patch`。
    ///
    /// 不做宽容解析：柜面终端的版本兼容判断不能建立在"猜"上，
    /// 写错版本号就应该在加载时报错，而不是悄悄按 0.0.0 处理。
    pub fn parse(input: &str) -> Result<Self, PkgError> {
        let parts: Vec<&str> = input.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(PkgError::BadVersion(input.to_string()));
        }
        let parse = |s: &str| {
            s.parse::<u64>()
                .map_err(|_| PkgError::BadVersion(input.to_string()))
        };
        Ok(Self {
            major: parse(parts[0])?,
            minor: parse(parts[1])?,
            patch: parse(parts[2])?,
        })
    }
}

/// 主壳兼容区间。
///
/// `max_shell` 允许 `0.x` 这种写法，等价于"主版本 0 的任意次版本"，
/// 因为柜面主壳按主版本发版，业务包要能跨次版本存活。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellRange {
    min: Version,
    /// 开区间上界：等于 max 所在主版本 + 1。
    max_exclusive: Version,
}

impl ShellRange {
    pub fn new(min: Version, max_exclusive: Version) -> Self {
        Self { min, max_exclusive }
    }

    pub fn min(&self) -> &Version {
        &self.min
    }

    pub fn max_exclusive(&self) -> &Version {
        &self.max_exclusive
    }

    pub fn parse(min: &str, max: &str) -> Result<Self, PkgError> {
        let min_v = Version::parse(min)?;
        let max_exclusive = if max.ends_with(".x") {
            let major: u64 = max
                .trim_end_matches(".x")
                .parse()
                .map_err(|_| PkgError::BadVersion(max.to_string()))?;
            Version {
                major: major + 1,
                minor: 0,
                patch: 0,
            }
        } else {
            let max_v = Version::parse(max)?;
            Version {
                major: max_v.major + 1,
                minor: 0,
                patch: 0,
            }
        };
        Ok(Self {
            min: min_v,
            max_exclusive,
        })
    }

    pub fn accepts(&self, shell: Version) -> bool {
        shell >= self.min && shell < self.max_exclusive
    }
}

// ---------------- 容器 ----------------

impl BizPackage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        manifest: BizManifest,
        menus: Vec<MenuNode>,
        permissions: Permissions,
        routes: BTreeMap<String, String>,
        transactions: Vec<TxMeta>,
        assets: BTreeMap<String, String>,
    ) -> Self {
        Self {
            manifest,
            menus,
            permissions,
            routes,
            transactions,
            assets,
        }
    }

    pub fn id(&self) -> String {
        self.manifest.id.clone()
    }

    pub fn version(&self) -> String {
        self.manifest.version.clone()
    }

    pub fn menu_count(&self) -> usize {
        self.menus.len()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    /// 查路由对应的模块名，不暴露整张路由表。
    pub fn route(&self, route: &str) -> Option<String> {
        self.routes.get(route).cloned()
    }

    /// 取模块源码副本，不暴露资源表。
    pub fn asset(&self, name: &str) -> Option<String> {
        self.assets.get(name).cloned()
    }

    /// 兼容的主壳版本下限（副本，不暴露 manifest）。
    pub fn min_shell(&self) -> String {
        self.manifest.min_shell.clone()
    }

    /// 兼容的主壳版本上限（副本）。
    pub fn max_shell(&self) -> String {
        self.manifest.max_shell.clone()
    }

    /// 依赖包 ID 列表（副本）。
    pub fn deps(&self) -> Vec<String> {
        self.manifest.deps.clone()
    }

    /// 打包成带签名的 `.bizpkg` 字节流。
    pub fn encode(&self, signing_key: &SigningKey) -> Result<Vec<u8>, PkgError> {
        validate_manifest(&self.manifest)?;
        let payload = rmp_serde::to_vec(&self)
            .map_err(|e| PkgError::DecodeFailed(format!("序列化失败: {e}")))?;
        let payload_len: u32 = payload
            .len()
            .try_into()
            .map_err(|_| PkgError::DecodeFailed("载荷过大".into()))?;

        let mut header = Vec::with_capacity(PKG_HEADER_LEN);
        header.extend_from_slice(PKG_MAGIC);
        header.extend_from_slice(&PKG_FORMAT_VERSION.to_le_bytes());
        header.extend_from_slice(&payload_len.to_le_bytes());
        header.extend_from_slice(&0u64.to_le_bytes()); // 预留：包内容版本
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        debug_assert_eq!(header.len(), PKG_HEADER_LEN);

        let digest = Sha256::new()
            .chain_update(&header)
            .chain_update(&payload)
            .finalize();
        let signature = signing_key.sign(digest.as_slice());

        let mut out = Vec::with_capacity(PKG_HEADER_LEN + payload.len() + PKG_SIGNATURE_LEN);
        out.extend_from_slice(&header);
        out.extend_from_slice(&payload);
        out.extend_from_slice(&signature.to_bytes());
        Ok(out)
    }

    /// 验签并解析。
    pub fn decode(bytes: &[u8], verifying_key: &VerifyingKey) -> Result<BizPackage, PkgError> {
        if bytes.len() < PKG_HEADER_LEN + PKG_SIGNATURE_LEN {
            return Err(PkgError::TooShort);
        }
        if &bytes[..8] != PKG_MAGIC {
            return Err(PkgError::BadMagic);
        }
        let format_version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if format_version != PKG_FORMAT_VERSION {
            return Err(PkgError::UnsupportedFormatVersion(format_version));
        }
        let payload_len = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
        let expected = PKG_HEADER_LEN + payload_len + PKG_SIGNATURE_LEN;
        if bytes.len() < expected {
            return Err(PkgError::TooShort);
        }
        if bytes.len() > expected {
            return Err(PkgError::PayloadLenMismatch);
        }

        let payload = &bytes[PKG_HEADER_LEN..PKG_HEADER_LEN + payload_len];
        let digest = Sha256::new()
            .chain_update(&bytes[..PKG_HEADER_LEN])
            .chain_update(payload)
            .finalize();
        let sig_bytes: [u8; PKG_SIGNATURE_LEN] = bytes[PKG_HEADER_LEN + payload_len..]
            .try_into()
            .map_err(|_| PkgError::TooShort)?;
        verifying_key
            .verify(digest.as_slice(), &Signature::from_bytes(&sig_bytes))
            .map_err(|_| PkgError::SignatureInvalid)?;

        let package: BizPackage =
            rmp_serde::from_slice(payload).map_err(|e| PkgError::DecodeFailed(e.to_string()))?;
        validate_manifest(&package.manifest)?;
        validate_menu_namespace(&package)?;
        Ok(package)
    }
}

fn validate_manifest(manifest: &BizManifest) -> Result<(), PkgError> {
    // 包 ID 至少两段（org.domain）；一段的 ID 迟早会和别人的撞。
    // 不强制三段：`biz.demo` 这种"域.业务"读起来更清楚，约束多一段只会催生 `biz.demo.core` 这类凑数命名。
    if manifest.id.split('.').count() < 2 {
        return Err(PkgError::BadPackageId(manifest.id.clone()));
    }
    Version::parse(&manifest.version)?;
    ShellRange::parse(&manifest.min_shell, &manifest.max_shell)?;
    Ok(())
}

/// 包注入的菜单必须落在自己的命名空间里，否则可以挂到别人的树下面。
fn validate_menu_namespace(package: &BizPackage) -> Result<(), PkgError> {
    let prefix = format!("{}.", package.manifest.id);
    for node in &package.menus {
        // 根节点可以就是包 ID 本身（`biz.demo`），子节点必须在 `biz.demo.` 之下
        if node.code() != package.manifest.id.as_str() && !node.code().starts_with(&prefix) {
            return Err(PkgError::UnnamespacedMenu(node.code().to_string()));
        }
    }
    Ok(())
}

// ---------------- 加载计划 ----------------

/// 一个包的加载结论。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageDecision {
    id: String,
    loaded: bool,
    reason: String,
}

impl PackageDecision {
    pub fn new(id: &str, loaded: bool, reason: &str) -> Self {
        Self {
            id: id.into(),
            loaded,
            reason: reason.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadPlan {
    /// 通过校验、按依赖拓扑排好序的包。
    ordered: Vec<BizPackage>,
    /// 被跳过的包及原因（不加载 ≠ 报错，柜面要能继续用别的功能）。
    skipped: Vec<PackageDecision>,
}

impl LoadPlan {
    pub fn new(ordered: Vec<BizPackage>, skipped: Vec<PackageDecision>) -> Self {
        Self { ordered, skipped }
    }

    pub fn ordered(&self) -> &[BizPackage] {
        &self.ordered
    }

    pub fn skipped(&self) -> &[PackageDecision] {
        &self.skipped
    }
}

/// 按主壳版本、依赖关系排出加载顺序。
///
/// 规则：
/// - 版本不兼容 → 跳过，说明原因；
/// - 依赖不在待加载集合里 → 跳过（不做"先加载了再说"）；
/// - 有环 → 全部跳过环上的包：宁可少加载，不要加载一半；
/// - 其余按拓扑序返回，同层按 ID 排序，保证顺序可复现。
pub fn plan_load(packages: Vec<BizPackage>, shell: Version) -> LoadPlan {
    let mut plan = LoadPlan::default();
    let mut candidates: BTreeMap<String, BizPackage> = BTreeMap::new();

    for package in packages {
        let id = package.id();
        let range = match ShellRange::parse(&package.min_shell(), &package.max_shell()) {
            Ok(r) => r,
            Err(e) => {
                plan.skipped.push(PackageDecision {
                    id,
                    loaded: false,
                    reason: format!("版本范围非法：{e}"),
                });
                continue;
            }
        };
        if !range.accepts(shell) {
            plan.skipped.push(PackageDecision {
                id,
                loaded: false,
                reason: format!(
                    "不兼容当前主壳 {}.{}.{}",
                    shell.major, shell.minor, shell.patch
                ),
            });
            continue;
        }
        candidates.insert(id, package);
    }

    let available: HashSet<String> = candidates.keys().cloned().collect();
    // 依赖缺失的直接剔除，再迭代，直到没有新的剔除（依赖可能是链式的）
    loop {
        let mut dropped = Vec::new();
        for (id, package) in &candidates {
            for dep in package.deps() {
                if !available.contains(&dep) || !candidates.contains_key(&dep) {
                    dropped.push((id.clone(), dep.clone()));
                    break;
                }
            }
        }
        if dropped.is_empty() {
            break;
        }
        for (id, dep) in dropped {
            if candidates.remove(&id).is_some() {
                plan.skipped.push(PackageDecision {
                    id,
                    loaded: false,
                    reason: format!("缺少依赖 {dep}"),
                });
            }
        }
    }

    // 拓扑排序 + 环检测
    let mut ordered: Vec<BizPackage> = Vec::new();
    let mut done: HashSet<String> = HashSet::new();
    let mut remaining: Vec<String> = candidates.keys().cloned().collect();

    while !remaining.is_empty() {
        let mut progressed = false;
        for id in remaining.clone() {
            let package = &candidates[&id];
            if package.deps().iter().all(|d| done.contains(d)) {
                done.insert(id.clone());
                remaining.retain(|x| x != &id);
                ordered.push(package.clone());
                progressed = true;
            }
        }
        if !progressed {
            // 剩下的全是环或依赖已剔除的，一并跳过
            for id in remaining {
                plan.skipped.push(PackageDecision {
                    id,
                    loaded: false,
                    reason: "依赖成环或依赖未满足".into(),
                });
            }
            break;
        }
    }

    plan.ordered = ordered;
    plan
}

// ---------------- 菜单合并 ----------------

/// 把包里的菜单增量并入全量树，返回冲突的 code。
///
/// 冲突策略沿用 `MenuTree::merge_package`：**已存在的优先**，包节点被丢弃。
/// 这条不能反过来——否则装一个包就能改掉主壳的导航。
pub fn merge_packages(tree: &mut MenuTree, packages: &[BizPackage]) -> Vec<String> {
    let mut conflicts = Vec::new();
    for package in packages {
        let mut nodes = package.menus.clone();
        // 来源不可自证：统一改写，避免包伪装成 shell 节点
        for node in &mut nodes {
            node.set_source(format!("pkg:{}", package.manifest.id));
        }
        conflicts.extend(tree.merge_package(nodes));
    }
    conflicts
}

/// 校验合并后的树仍然结构合法（父节点可能由别的包提供）。
pub fn validate_merged(tree: &MenuTree) -> Result<(), PackError> {
    tree.validate().map_err(PackError::InvalidTree)?;
    Ok(())
}

/// 扫描目录下的 `*.bizpkg`，验签后做成加载计划。
///
/// 坏包不阻断启动：记一条决策，其余照常加载。柜面宁可少一个业务，
/// 也不要因为别人放了个坏文件就开不了机。
pub fn scan_dir(
    dir: &std::path::Path,
    verifying_key: &VerifyingKey,
    shell: Version,
) -> (LoadPlan, Vec<PackageDecision>) {
    let mut blobs = Vec::new();
    let mut rejected = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bizpkg") {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => blobs.push(bytes),
                Err(e) => rejected.push(PackageDecision {
                    id: path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("（未知）")
                        .to_string(),
                    loaded: false,
                    reason: format!("读取失败：{e}"),
                }),
            }
        }
    }
    let (plan, mut failures) = load_packages(&blobs, verifying_key, shell);
    rejected.append(&mut failures);
    (plan, rejected)
}

/// 便捷：从字节加载一批包并做出加载计划。
pub fn load_packages(
    blobs: &[Vec<u8>],
    verifying_key: &VerifyingKey,
    shell: Version,
) -> (LoadPlan, Vec<PackageDecision>) {
    let mut rejected = Vec::new();
    let mut good = Vec::new();
    for blob in blobs {
        match BizPackage::decode(blob, verifying_key) {
            Ok(p) => good.push(p),
            Err(e) => rejected.push(PackageDecision {
                id: "（未知）".into(),
                loaded: false,
                reason: e.to_string(),
            }),
        }
    }
    let plan = plan_load(good, shell);
    let mut all_rejected = rejected;
    all_rejected.extend(plan.skipped.clone());
    (plan, all_rejected)
}

/// 依赖图里每个包的层级深度，供诊断使用（0 = 无依赖）。
pub fn dependency_depth(packages: &[BizPackage]) -> HashMap<String, usize> {
    let mut depth = HashMap::new();
    for package in packages {
        let d = package
            .deps()
            .iter()
            .map(|dep| depth.get(dep).copied().unwrap_or(0) + 1)
            .max()
            .unwrap_or(0);
        depth.insert(package.id(), d);
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{demo_signing_key, demo_verifying_key};
    use crate::perm::PermSnapshot;
    use rand::rngs::OsRng;

    fn node(code: &str, parent: Option<&str>, sort: i32) -> MenuNode {
        MenuNode::page(code, parent, code, sort)
    }

    fn pkg(id: &str, deps: &[&str]) -> BizPackage {
        BizPackage {
            manifest: BizManifest {
                id: id.into(),
                name: id.into(),
                version: "0.1.0".into(),
                min_shell: "0.1.0".into(),
                max_shell: "0.x".into(),
                deps: deps.iter().map(|s| s.to_string()).collect(),
                entry: "ui/entry.js".into(),
            },
            menus: vec![node(&format!("{id}.home"), None, 100)],
            permissions: Permissions {
                actions: vec![format!("{id}.read")],
            },
            routes: BTreeMap::new(),
            transactions: Vec::new(),
            assets: BTreeMap::new(),
        }
    }

    const SHELL: Version = Version {
        major: 0,
        minor: 1,
        patch: 0,
    };

    // S-01
    #[test]
    fn valid_package_roundtrips_and_merges() {
        let sk = demo_signing_key();
        let bytes = pkg("biz.demo", &[]).encode(&sk).unwrap();
        let loaded = BizPackage::decode(&bytes, &demo_verifying_key()).unwrap();
        assert_eq!(loaded.manifest.id, "biz.demo");

        let mut tree = MenuTree::new(1, vec![]);
        let conflicts = merge_packages(&mut tree, &[loaded]);
        assert!(conflicts.is_empty());
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.nodes()[0].source(), "pkg:biz.demo");
    }

    // S-02
    #[test]
    fn tampered_package_is_rejected() {
        let sk = demo_signing_key();
        let mut bytes = pkg("biz.demo", &[]).encode(&sk).unwrap();
        let idx = bytes.len() / 2;
        bytes[idx] ^= 0x01;
        assert_eq!(
            BizPackage::decode(&bytes, &demo_verifying_key()).unwrap_err(),
            PkgError::SignatureInvalid
        );
    }

    #[test]
    fn wrong_key_is_rejected() {
        let bytes = pkg("biz.demo", &[]).encode(&demo_signing_key()).unwrap();
        let other = SigningKey::generate(&mut OsRng);
        assert_eq!(
            BizPackage::decode(&bytes, &other.verifying_key()).unwrap_err(),
            PkgError::SignatureInvalid
        );
    }

    // S-03
    #[test]
    fn incompatible_shell_version_is_skipped_with_reason() {
        let mut package = pkg("biz.demo", &[]);
        package.manifest.min_shell = "0.2.0".into();
        let plan = plan_load(vec![package], SHELL);
        assert!(plan.ordered.is_empty());
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].reason.contains("不兼容"));
    }

    #[test]
    fn wildcard_max_shell_accepts_same_major() {
        let range = ShellRange::parse("0.1.0", "0.x").unwrap();
        assert!(
            !range.accepts(Version {
                major: 1,
                minor: 0,
                patch: 0
            }),
            "主版本 1 不在 0.x 区间内"
        );
        assert!(range.accepts(Version {
            major: 0,
            minor: 9,
            patch: 9
        }));
    }

    // S-04
    #[test]
    fn dependency_order_is_respected() {
        let plan = plan_load(
            vec![
                pkg("biz.b", &["biz.a"]),
                pkg("biz.a", &[]),
                pkg("biz.c", &[]),
            ],
            SHELL,
        );
        let ids: Vec<&str> = plan
            .ordered
            .iter()
            .map(|p| p.manifest.id.as_str())
            .collect();
        assert_eq!(ids, vec!["biz.a", "biz.b", "biz.c"]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn missing_dependency_skips_dependent_package() {
        let plan = plan_load(vec![pkg("biz.b", &["biz.missing"])], SHELL);
        assert!(plan.ordered.is_empty());
        assert!(plan.skipped[0].reason.contains("缺少依赖"));
    }

    #[test]
    fn cyclic_dependency_skips_whole_cycle() {
        let plan = plan_load(
            vec![pkg("biz.x", &["biz.y"]), pkg("biz.y", &["biz.x"])],
            SHELL,
        );
        assert!(plan.ordered.is_empty());
        assert_eq!(plan.skipped.len(), 2);
        assert!(plan.skipped.iter().all(|s| s.reason.contains("成环")));
    }

    // S-05
    #[test]
    fn existing_menu_wins_over_package() {
        let mut tree = MenuTree::new(1, vec![node("biz.demo.home", None, 1)]);
        tree.nodes_mut()[0].set_title("主壳的节点".into());
        let conflicts = merge_packages(&mut tree, &[pkg("biz.demo", &[])]);
        assert_eq!(conflicts, vec!["biz.demo.home".to_string()]);
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.nodes()[0].title(), "主壳的节点");
    }

    #[test]
    fn package_cannot_escape_its_namespace() {
        let mut package = pkg("biz.demo", &[]);
        package.menus[0] = MenuNode::page("account.withdraw", None, "account.withdraw", 100); // 别人的树
        let sk = demo_signing_key();
        let bytes = package.encode(&sk).unwrap();
        assert!(matches!(
            BizPackage::decode(&bytes, &demo_verifying_key()),
            Err(PkgError::UnnamespacedMenu(_))
        ));
    }

    // S-06
    #[test]
    fn package_permissions_grant_nothing_without_server() {
        use crate::menu::filter::filter_visible;
        use std::collections::BTreeMap as Map;

        let mut tree = MenuTree::new(1, vec![]);
        merge_packages(&mut tree, &[pkg("biz.demo", &[])]);

        // 服务端没授予该 code：包声明了也没用
        let snapshot = PermSnapshot::new("u", "T", "O", 1, vec![], vec![], Map::new(), "柜员");
        assert!(filter_visible(&tree, &snapshot).is_empty());

        // 服务端授予后才出现
        let granted = PermSnapshot::new(
            "u",
            "T",
            "O",
            1,
            vec!["biz.demo.home".into()],
            vec![],
            Map::new(),
            "柜员",
        );
        assert_eq!(filter_visible(&tree, &granted).len(), 1);
    }

    #[test]
    fn bad_package_id_is_rejected() {
        let mut package = pkg("demo", &[]);
        package.manifest.id = "demo".into();
        package.menus[0] = MenuNode::page("demo.home", None, "demo.home", 100);
        assert!(matches!(
            package.encode(&demo_signing_key()),
            Err(PkgError::BadPackageId(_))
        ));
    }

    #[test]
    fn load_packages_reports_rejected_and_skipped_separately() {
        let sk = demo_signing_key();
        let good = pkg("biz.good", &[]).encode(&sk).unwrap();
        let mut bad = pkg("biz.bad", &[]).encode(&sk).unwrap();
        let idx = bad.len() / 2;
        bad[idx] ^= 0xFF;

        let (plan, rejected) = load_packages(&[good, bad], &demo_verifying_key(), SHELL);
        assert_eq!(plan.ordered.len(), 1);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("签名"));
    }

    #[test]
    fn scan_dir_loads_good_and_reports_bad() {
        let dir = tempfile::tempdir().unwrap();
        let sk = demo_signing_key();
        std::fs::write(
            dir.path().join("good.bizpkg"),
            pkg("biz.good", &[]).encode(&sk).unwrap(),
        )
        .unwrap();
        let mut bad = pkg("biz.bad", &[]).encode(&sk).unwrap();
        let idx = bad.len() / 2;
        bad[idx] ^= 0xFF;
        std::fs::write(dir.path().join("bad.bizpkg"), bad).unwrap();
        // 非 bizpkg 后缀应被忽略
        std::fs::write(dir.path().join("notes.txt"), b"ignore me").unwrap();

        let (plan, rejected) = scan_dir(dir.path(), &demo_verifying_key(), SHELL);
        assert_eq!(plan.ordered.len(), 1);
        assert_eq!(plan.ordered[0].manifest.id, "biz.good");
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("签名"));
    }

    #[test]
    fn dependency_depth_is_reported() {
        let plan = plan_load(vec![pkg("biz.a", &[]), pkg("biz.b", &["biz.a"])], SHELL);
        let depth = dependency_depth(&plan.ordered);
        assert_eq!(depth["biz.a"], 0);
        assert_eq!(depth["biz.b"], 1);
    }
}
