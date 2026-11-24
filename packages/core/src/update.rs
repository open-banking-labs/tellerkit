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
    pub version: Version,
    pub channel: Channel,
    /// 灰度比例 0-100。100 表示全量。
    pub rollout_percent: u8,
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
    pub action: UpdateAction,
    pub reason: String,
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
    unimplemented!()
}

/// 灰度判定：把"网点 + 版本"稳定散列到 0-99。
///
/// 不用随机数：同一网点同一版本必须每次得到同样结果，
/// 否则柜员重开一次客户端就可能换一个结论。
pub fn rollout_bucket(outlet: &str, version: &Version) -> u8 {
    unimplemented!()
}
