//! 柜面外设抽象（P1 第二片）。
//!
//! 业务代码只面对 trait，不面对厂商型号。换一批设备时只换适配器，
//! 交易逻辑不动——这是"柜面软件生命周期远长于某台设备"的直接应对。
//!
//! 两条硬约束：
//! 1. 设备就绪校验必须发生在任何副作用之前（不能打了一半凭证才发现密码键盘坏了）；
//! 2. 完整卡号与 PIN 块不得进入 Debug、日志或错误信息。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// 设备种类。错误信息与交易元数据都用它，避免出现字符串字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Printer,
    CardReader,
    PinPad,
    Scanner,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Printer => "打印机",
            DeviceKind::CardReader => "读卡器",
            DeviceKind::PinPad => "密码键盘",
            DeviceKind::Scanner => "高拍仪",
        }
    }
}

/// 设备状态。`Faulty` 带原因，因为"缺纸"和"驱动崩了"对柜员的处置完全不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStatus {
    Ready,
    NotConnected,
    Faulty(String),
}

impl DeviceStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, DeviceStatus::Ready)
    }

    fn describe(&self) -> String {
        match self {
            DeviceStatus::Ready => "就绪".into(),
            DeviceStatus::NotConnected => "未连接".into(),
            DeviceStatus::Faulty(reason) => format!("故障：{reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("{kind}不可用（{state}）")]
    NotReady { kind: String, state: String },
    #[error("{0}操作超时")]
    Timeout(&'static str),
    #[error("柜员取消了{0}操作")]
    UserCancelled(&'static str),
    #[error("{0}硬件错误：{1}")]
    Hardware(&'static str, String),
}

/// 打印凭证。模板名与要素分离：套打格式随网点配置变，要素由交易产生。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voucher {
    pub template: String,
    pub tx_code: String,
    #[serde(default)]
    pub reference: Option<String>,
    pub fields: BTreeMap<String, String>,
}

impl Voucher {
    pub fn new(template: &str, tx_code: &str) -> Self {
        Self {
            template: template.to_string(),
            tx_code: tx_code.to_string(),
            reference: None,
            fields: BTreeMap::new(),
        }
    }

    pub fn field(mut self, k: &str, v: &str) -> Self {
        self.fields.insert(k.to_string(), v.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintReceipt {
    pub job_id: String,
    pub pages: u32,
}

/// 卡数据。**只存掩码**：完整 PAN 不允许出现在这个结构体里，
/// 因为结构体一旦被 Debug/序列化，完整卡号就跟着泄露。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardData {
    pub pan_masked: String,
    pub has_track2: bool,
}

impl CardData {
    /// 构造时强制掩码，调用方没有机会传入完整卡号。
    pub fn new(pan: &str, has_track2: bool) -> Self {
        let digits: String = pan.chars().filter(|c| c.is_ascii_digit()).collect();
        let masked = if digits.len() >= 10 {
            format!("{}******{}", &digits[..6], &digits[digits.len() - 4..])
        } else {
            "****".to_string()
        };
        Self {
            pan_masked: masked,
            has_track2,
        }
    }
}

/// PIN 块。手动实现 Debug：默认 derive 会把密文打进日志。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinBlock(String);

impl PinBlock {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// 只允许在送主机时取出，且调用点必须是显式的。
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for PinBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PinBlock(<redacted, {} bytes>)", self.0.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedImage {
    pub path: String,
    pub mime: String,
}

pub trait Printer {
    fn status(&self) -> DeviceStatus;
    fn print(&self, voucher: &Voucher) -> Result<PrintReceipt, DeviceError>;
}

pub trait CardReader {
    fn status(&self) -> DeviceStatus;
    fn read(&self) -> Result<CardData, DeviceError>;
}

pub trait PinPad {
    fn status(&self) -> DeviceStatus;
    fn read_pin(&self) -> Result<PinBlock, DeviceError>;
}

pub trait Scanner {
    fn status(&self) -> DeviceStatus;
    fn capture(&self, kind: &str) -> Result<CapturedImage, DeviceError>;
}

/// 设备集合：业务代码拿到的唯一入口。
///
/// 用四个具名字段而不是 `HashMap<DeviceKind, Box<dyn Any>>`：
/// 类型安全，且编译期就能看出"少接了一类设备"。
pub struct Devices {
    pub printer: Box<dyn Printer>,
    pub card_reader: Box<dyn CardReader>,
    pub pinpad: Box<dyn PinPad>,
    pub scanner: Box<dyn Scanner>,
}

impl Devices {
    /// 交易开始前的就绪校验。
    ///
    /// 必须在任何副作用之前调用：宁可拒绝一笔交易，
    /// 也不要出现"凭证已打出、但密码键盘坏了"的半截状态。
    pub fn ensure(&self, required: &[DeviceKind]) -> Result<(), DeviceError> {
        for kind in required {
            let status = match kind {
                DeviceKind::Printer => self.printer.status(),
                DeviceKind::CardReader => self.card_reader.status(),
                DeviceKind::PinPad => self.pinpad.status(),
                DeviceKind::Scanner => self.scanner.status(),
            };
            if !status.is_ready() {
                return Err(DeviceError::NotReady {
                    kind: kind.label().to_string(),
                    state: status.describe(),
                });
            }
        }
        Ok(())
    }

    pub fn status_of(&self, kind: DeviceKind) -> DeviceStatus {
        match kind {
            DeviceKind::Printer => self.printer.status(),
            DeviceKind::CardReader => self.card_reader.status(),
            DeviceKind::PinPad => self.pinpad.status(),
            DeviceKind::Scanner => self.scanner.status(),
        }
    }
}

// ---- mock 适配器：P3 接真机前，全部业务路径都跑在这套实现上 ----

#[derive(Default)]
pub struct MockPrinter {
    pub state: Option<DeviceStatus>,
    pub printed: std::cell::RefCell<Vec<Voucher>>,
}

impl MockPrinter {
    pub fn ready() -> Self {
        Self::default()
    }

    pub fn faulty(reason: &str) -> Self {
        Self {
            state: Some(DeviceStatus::Faulty(reason.into())),
            ..Default::default()
        }
    }

    pub fn printed(&self) -> std::cell::RefCell<Vec<Voucher>> {
        std::cell::RefCell::new(self.printed.borrow().clone())
    }
}

impl Printer for MockPrinter {
    fn status(&self) -> DeviceStatus {
        self.state.clone().unwrap_or(DeviceStatus::Ready)
    }

    fn print(&self, voucher: &Voucher) -> Result<PrintReceipt, DeviceError> {
        if !self.status().is_ready() {
            return Err(DeviceError::NotReady {
                kind: DeviceKind::Printer.label().to_string(),
                state: self.status().describe(),
            });
        }
        self.printed.borrow_mut().push(voucher.clone());
        Ok(PrintReceipt {
            job_id: format!("PRINT-{}", self.printed.borrow().len()),
            pages: 1,
        })
    }
}

pub struct MockCardReader {
    pub state: DeviceStatus,
    pub pan: String,
}

impl MockCardReader {
    pub fn with_pan(pan: &str) -> Self {
        Self {
            state: DeviceStatus::Ready,
            pan: pan.to_string(),
        }
    }
}

impl CardReader for MockCardReader {
    fn status(&self) -> DeviceStatus {
        self.state.clone()
    }

    fn read(&self) -> Result<CardData, DeviceError> {
        if !self.status().is_ready() {
            return Err(DeviceError::NotReady {
                kind: DeviceKind::CardReader.label().to_string(),
                state: self.status().describe(),
            });
        }
        Ok(CardData::new(&self.pan, true))
    }
}

pub struct MockPinPad {
    pub state: DeviceStatus,
}

impl MockPinPad {
    pub fn ready() -> Self {
        Self {
            state: DeviceStatus::Ready,
        }
    }

    pub fn faulty(reason: &str) -> Self {
        Self {
            state: DeviceStatus::Faulty(reason.into()),
        }
    }
}

impl PinPad for MockPinPad {
    fn status(&self) -> DeviceStatus {
        self.state.clone()
    }

    fn read_pin(&self) -> Result<PinBlock, DeviceError> {
        if !self.status().is_ready() {
            return Err(DeviceError::NotReady {
                kind: DeviceKind::PinPad.label().to_string(),
                state: self.status().describe(),
            });
        }
        Ok(PinBlock::new("encrypted-pin-block-8bytes"))
    }
}

pub struct MockScanner {
    pub state: DeviceStatus,
}

impl MockScanner {
    pub fn ready() -> Self {
        Self {
            state: DeviceStatus::Ready,
        }
    }
}

impl Scanner for MockScanner {
    fn status(&self) -> DeviceStatus {
        self.state.clone()
    }

    fn capture(&self, kind: &str) -> Result<CapturedImage, DeviceError> {
        if !self.status().is_ready() {
            return Err(DeviceError::NotReady {
                kind: DeviceKind::Scanner.label().to_string(),
                state: self.status().describe(),
            });
        }
        Ok(CapturedImage {
            path: format!("/tmp/capture-{kind}.jpg"),
            mime: "image/jpeg".into(),
        })
    }
}

/// 组装一套全就绪的 mock 设备，供测试与 P1.3 的界面使用。
pub fn mock_devices() -> Devices {
    Devices {
        printer: Box::new(MockPrinter::ready()),
        card_reader: Box::new(MockCardReader::with_pan("6222021234567890123")),
        pinpad: Box::new(MockPinPad::ready()),
        scanner: Box::new(MockScanner::ready()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voucher() -> Voucher {
        Voucher::new("voucher-deposit", "account.deposit")
            .field("amount", "100.00")
            .field("account", "6222****0123")
    }

    // S-01
    #[test]
    fn print_succeeds_with_job_id_and_pages() {
        let printer = MockPrinter::ready();
        let receipt = printer.print(&voucher()).unwrap();
        assert_eq!(receipt.job_id, "PRINT-1");
        assert_eq!(receipt.pages, 1);
        assert_eq!(printer.printed.borrow().len(), 1);
    }

    // S-02
    #[test]
    fn faulty_printer_returns_actionable_error_not_panic() {
        let printer = MockPrinter::faulty("缺纸");
        let err = printer.print(&voucher()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("打印机"), "错误里必须说明是哪类设备：{text}");
        assert!(text.contains("缺纸"), "错误里必须说明状态：{text}");
    }

    // S-03
    #[test]
    fn missing_device_is_detected_before_any_side_effect() {
        let devices = Devices {
            printer: Box::new(MockPrinter::ready()),
            card_reader: Box::new(MockCardReader::with_pan("6222")),
            pinpad: Box::new(MockPinPad::faulty("未连接")),
            scanner: Box::new(MockScanner::ready()),
        };
        let err = devices
            .ensure(&[DeviceKind::Printer, DeviceKind::PinPad])
            .unwrap_err();
        assert!(err.to_string().contains("密码键盘"));
    }

    // S-04
    #[test]
    fn pin_block_debug_is_redacted() {
        let block = PinBlock::new("SECRET-PIN-BLOCK");
        let debug = format!("{block:?}");
        assert!(
            !debug.contains("SECRET"),
            "PIN 块不得出现在 Debug 输出：{debug}"
        );
        assert!(debug.contains("redacted"));
    }

    // S-04
    #[test]
    fn card_data_only_keeps_masked_pan() {
        let card = CardData::new("6222021234567890123", true);
        assert_eq!(card.pan_masked, "622202******0123");
        let debug = format!("{card:?}");
        assert!(
            !debug.contains("6222021234567890123"),
            "完整卡号不得出现在结构体或 Debug 中：{debug}"
        );
    }

    // S-05
    #[test]
    fn business_code_only_depends_on_trait() {
        // 这段"业务函数"只认 &dyn Printer：换实现不需要改它。
        fn business_print(printer: &dyn Printer, v: &Voucher) -> Result<String, String> {
            printer
                .print(v)
                .map(|r| r.job_id)
                .map_err(|e| e.to_string())
        }

        assert_eq!(
            business_print(&MockPrinter::ready(), &voucher()).unwrap(),
            "PRINT-1"
        );
        assert!(business_print(&MockPrinter::faulty("卡纸"), &voucher()).is_err());
    }

    #[test]
    fn ready_devices_pass_ensure_for_all_kinds() {
        let devices = mock_devices();
        assert!(devices
            .ensure(&[
                DeviceKind::Printer,
                DeviceKind::CardReader,
                DeviceKind::PinPad,
                DeviceKind::Scanner
            ])
            .is_ok());
        assert_eq!(devices.status_of(DeviceKind::Printer), DeviceStatus::Ready);
    }
}
