//! 更新策略（P3）。
//!
//! 柜面终端的更新有两件事不能出错：**别在交易中要求重启**，
//! **出问题要能整体退回**。所以策略与执行分开：
//! 这里只回答"现在该不该下、下了什么时候生效"，下载与替换由更新器做。
//!
//! 灰度按网点做，不按终端随机：同一个网点的柜员第二天应该看到同样的版本，
//! 否则排障时没人能说清"她那台为什么不一样"。

use crate::pkg::Version;
use serde::{Deserialize, Serialize};

/// 发布通道。柜面默认 `Stable`，`Canary` 只给试点网点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Canary,
}

/// 服务端给出的候选版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCandidate {
    version: Version,
    channel: Channel,
    /// 灰度比例 0-100。100 表示全量。
    rollout_percent: u8,
}

impl UpdateCandidate {
    pub fn new(version: Version, channel: Channel, rollout_percent: u8) -> Self {
        Self {
            version,
            channel,
            rollout_percent,
        }
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    pub fn rollout_percent(&self) -> u8 {
        self.rollout_percent
    }
}

/// 本次检查的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateAction {
    /// 不动。
    Skip,
    /// 后台下载，但**不在本次会话生效**（等日终或次日开机）。
    DownloadOnly,
    /// 后台下载并提示柜员"下次启动生效"。
    DownloadAndNotify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateDecision {
    action: UpdateAction,
    reason: String,
}

impl UpdateDecision {
    pub fn new(action: UpdateAction, reason: &str) -> Self {
        Self {
            action,
            reason: reason.into(),
        }
    }

    pub fn action(&self) -> UpdateAction {
        self.action
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// 决定是否更新。
///
/// 入参 `outlet` 参与灰度散列；`local_hour` 用来避开营业高峰。
/// 纯函数：同样的输入永远得到同样的结论，便于复现排障。
pub fn decide(
    current: &Version,
    candidate: &UpdateCandidate,
    channel: Channel,
    outlet: &str,
    local_hour: u8,
) -> UpdateDecision {
    // 顺序有讲究：
    // 1. 通道先行：候选不属于本终端通道时，它根本不该被评估，先报"通道"
    //    比报"已是最新"更接近真实原因（试点包对新旧版本都无关）。
    // 2. 再比版本：没有新版本就不该消耗灰度名额，也不该打扰柜员。
    // 3. 灰度命中：只有该网点被选中才继续；未命中就是"这次没轮到你"。
    // 4. 最后看时段：命中灰度后，营业高峰只下载、不提示，避免柜员在
    //    交易中看到重启提示。
    if candidate.channel() != channel {
        return UpdateDecision {
            action: UpdateAction::Skip,
            reason: format!(
                "候选版本 {} 属于 {} 通道，当前终端在 {} 通道，跳过",
                version_label(candidate.version()),
                channel_label(candidate.channel()),
                channel_label(channel),
            ),
        };
    }

    if candidate.version() <= current {
        return UpdateDecision {
            action: UpdateAction::Skip,
            reason: format!("当前版本 {} 已是最新，无需更新", version_label(current)),
        };
    }

    let bucket = rollout_bucket(outlet, candidate.version());
    // `rollout_percent >= 100`（含服务端越界下发）一律视为全量，
    // 0 则任何网点都不命中：`bucket` 最大 99，`99 < 0` 不成立。
    if candidate.rollout_percent() < 100 && bucket >= candidate.rollout_percent() {
        return UpdateDecision {
            action: UpdateAction::Skip,
            reason: format!(
                "灰度 {}% 未命中灰度（网点 {} 散列桶 {}），本次跳过",
                candidate.rollout_percent(),
                outlet,
                bucket,
            ),
        };
    }

    if (9..=16).contains(&local_hour) {
        return UpdateDecision {
            action: UpdateAction::DownloadOnly,
            reason: format!(
                "命中灰度 {}%，但当前 {} 点处于营业高峰，营业高峰只下载不提示，等日终或次日开机生效",
                candidate.rollout_percent(), local_hour,
            ),
        };
    }

    UpdateDecision {
        action: UpdateAction::DownloadAndNotify,
        reason: format!(
            "命中灰度 {}%，且不在营业高峰，下载后提示柜员下次启动生效",
            candidate.rollout_percent(),
        ),
    }
}

fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Canary => "canary",
    }
}

fn version_label(version: &Version) -> String {
    format!(
        "{}.{}.{}",
        version.major(),
        version.minor(),
        version.patch()
    )
}

/// 灰度判定：把"网点 + 版本"稳定散列到 0-99。
///
/// 不用随机数：同一网点同一版本必须每次得到同样结果，
/// 否则柜员重开一次客户端就可能换一个结论。
pub fn rollout_bucket(outlet: &str, version: &Version) -> u8 {
    let mut hash = FNV_OFFSET_BASIS;
    // 长度前缀 + 三段定宽 u64：拼接无歧义，避免 "ab" + 1 与 "a" + b1 撞成同一输入。
    hash = fnv1a(hash, &(outlet.len() as u64).to_le_bytes());
    hash = fnv1a(hash, outlet.as_bytes());
    for part in [version.major(), version.minor(), version.patch()] {
        hash = fnv1a(hash, &part.to_be_bytes());
    }
    (mix64(hash) % 100) as u8
}

// ---------------- 散列实现 ----------------
//
// 用自己实现的 FNV-1a，而不是 `std::collections::hash_map::DefaultHasher`：
// 后者的算法是标准库私有实现，跨 Rust 版本不承诺稳定；一旦编译器升级换了
// 实现，同一网点的桶会整体漂移，灰度就变成"每次发版随便挑一批网点"。
// 这里把算法写死在代码里，并用黄金值测试钉住。

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// splitmix64 的收尾混合。
///
/// FNV-1a 对"只差最后几个字节"的输入扩散一般，`OUTLET-01`、`OUTLET-02`
/// 这类连续编号容易挤在相近的桶里；混合一轮再取模，网点分布更像散列。
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn current() -> Version {
        Version::new(0, 1, 0)
    }

    fn ver(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    fn cand(version: Version, channel: Channel, rollout_percent: u8) -> UpdateCandidate {
        UpdateCandidate {
            version,
            channel,
            rollout_percent,
        }
    }

    /// 走一次 Stable 通道的默认决策，省掉用例里的样板。
    fn stable(version: Version, percent: u8, outlet: &str, hour: u8) -> UpdateDecision {
        decide(
            &current(),
            &cand(version, Channel::Stable, percent),
            Channel::Stable,
            outlet,
            hour,
        )
    }

    // S-01
    #[test]
    fn s01_new_version_hit_rollout_off_peak_downloads_and_notifies() {
        let d = stable(ver(0, 2, 0), 100, "O-001", 20);
        assert_eq!(d.action, UpdateAction::DownloadAndNotify);
        assert!(d.reason.contains("下次启动"), "{}", d.reason);
    }

    // S-02
    #[test]
    fn s02_same_or_older_candidate_is_skipped_as_up_to_date() {
        for candidate_version in [ver(0, 1, 0), ver(0, 0, 9)] {
            let d = stable(candidate_version, 100, "O-001", 20);
            assert_eq!(d.action, UpdateAction::Skip);
            assert!(d.reason.contains("已是最新"), "{}", d.reason);
        }
    }

    // S-03
    #[test]
    fn s03_channel_mismatch_is_skipped_with_channel_reason() {
        // 版本更新、灰度 100% 也不升：通道不对就不是这个终端的候选。
        let d = decide(
            &current(),
            &cand(ver(0, 2, 0), Channel::Canary, 100),
            Channel::Stable,
            "O-001",
            20,
        );
        assert_eq!(d.action, UpdateAction::Skip);
        assert!(d.reason.contains("通道"), "{}", d.reason);

        // 反向同理：Canary 终端不接收 Stable 候选。
        let d = decide(
            &current(),
            &cand(ver(0, 2, 0), Channel::Stable, 100),
            Channel::Canary,
            "O-001",
            20,
        );
        assert_eq!(d.action, UpdateAction::Skip);
        assert!(d.reason.contains("通道"), "{}", d.reason);
    }

    // S-04
    #[test]
    fn s04_rollout_miss_is_skipped_and_hit_passes() {
        let version = ver(0, 2, 0);
        let mut hit = None;
        let mut miss = None;
        for i in 0..200u32 {
            let outlet = format!("O-{i:03}");
            if rollout_bucket(&outlet, &version) < 10 {
                hit = Some(outlet);
            } else {
                miss = Some(outlet);
            }
            if hit.is_some() && miss.is_some() {
                break;
            }
        }
        let hit = hit.expect("200 个网点里至少应有一个命中 10% 灰度");
        let miss = miss.expect("200 个网点里至少应有一个未命中 10% 灰度");

        // 命中者继续，未命中者带原因跳过。
        assert_eq!(
            stable(version, 10, &hit, 20).action,
            UpdateAction::DownloadAndNotify
        );
        let d = stable(version, 10, &miss, 20);
        assert_eq!(d.action, UpdateAction::Skip);
        assert!(d.reason.contains("未命中灰度"), "{}", d.reason);

        // 0% 任何网点都不命中；100% 任何网点都命中。
        for i in 0..20u32 {
            let outlet = format!("O-{i:03}");
            let zero = stable(version, 0, &outlet, 20);
            assert_eq!(zero.action, UpdateAction::Skip);
            assert!(zero.reason.contains("未命中灰度"), "{}", zero.reason);
            assert_eq!(
                stable(version, 100, &outlet, 20).action,
                UpdateAction::DownloadAndNotify
            );
        }
    }

    // S-05
    #[test]
    fn s05_business_hours_download_only_without_prompting() {
        for hour in 9..=16u8 {
            let d = stable(ver(0, 2, 0), 100, "O-001", hour);
            assert_eq!(d.action, UpdateAction::DownloadOnly, "hour={hour}");
            assert!(d.reason.contains("营业高峰"), "{}", d.reason);
            assert!(d.reason.contains("不提示"), "{}", d.reason);
            assert!(!d.reason.contains("提示柜员"), "{}", d.reason);
        }

        for hour in [0u8, 8, 17, 23] {
            let d = stable(ver(0, 2, 0), 100, "O-001", hour);
            assert_eq!(d.action, UpdateAction::DownloadAndNotify, "hour={hour}");
        }
    }

    // S-06 散列稳定：同输入同输出、跨实例一致、24 个网点不塌成常量。
    #[test]
    fn s06_hash_is_stable_and_dispersed_across_outlets() {
        let version = ver(0, 2, 0);
        let outlets: Vec<String> = (0..24).map(|i| format!("BRANCH-{i:03}")).collect();
        let mut buckets = BTreeSet::new();
        for outlet in &outlets {
            let bucket = rollout_bucket(outlet.as_str(), &version);
            assert!(bucket < 100, "桶必须落在 0-99: {outlet} -> {bucket}");
            assert_eq!(bucket, rollout_bucket(outlet.as_str(), &version));
            // 同内容、另行构造的字符串实例也必须同桶。
            assert_eq!(bucket, rollout_bucket(outlet.clone().as_str(), &version));
            buckets.insert(bucket);
        }
        assert!(
            buckets.len() > 3,
            "24 个网点只落到 {} 个桶，散列退化成常量",
            buckets.len()
        );
    }

    // S-06 跨进程/跨版本：黄金值钉住算法，无意改动会让灰度整体洗牌。
    #[test]
    fn s06_hash_matches_pinned_golden_values() {
        let version = ver(0, 2, 0);
        assert_eq!(rollout_bucket("BRANCH-000", &version), 1);
        assert_eq!(rollout_bucket("BRANCH-007", &version), 9);
        assert_eq!(rollout_bucket("BRANCH-019", &version), 37);
        assert_eq!(rollout_bucket("OUTLET-42", &version), 33);
    }

    #[test]
    fn s06_hash_depends_on_version_so_rollout_reshuffles() {
        assert_ne!(
            rollout_bucket("BRANCH-000", &ver(0, 2, 0)),
            rollout_bucket("BRANCH-000", &ver(0, 3, 0)),
            "换版本应重新洗牌，否则永远是同一批网点当灰度"
        );
    }

    #[test]
    fn decide_is_pure_and_hour_only_comes_from_argument() {
        assert_eq!(
            stable(ver(0, 2, 0), 100, "O-001", 20),
            stable(ver(0, 2, 0), 100, "O-001", 20)
        );
        // 只改时段入参就改变结论：时段来自入参，不是读时钟。
        assert_eq!(
            stable(ver(0, 2, 0), 100, "O-001", 10).action,
            UpdateAction::DownloadOnly
        );
    }
}
