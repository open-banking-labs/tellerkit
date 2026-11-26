//! 柜面外设抽象（P1 第二片）。
//!
//! 业务代码只面对 trait，不面对厂商型号。换一批设备时只换适配器，
//! 交易逻辑不动——这是"柜面软件生命周期远长于某台设备"的直接应对。
//!
//! 两条硬约束：
//! 1. 设备就绪校验必须发生在任何副作用之前（不能打了一半凭证才发现密码键盘坏了）；
//! 2. 完整卡号与 PIN 块不得进入 Debug、日志或错误信息。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// 型号不在白名单。必须带型号名：现场据此判断是"登记漏了"还是"插错设备"。
    #[error("{kind} 型号未授权：{model}")]
    NotWhitelisted { kind: String, model: String },
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
    template: String,
    tx_code: String,
    #[serde(default)]
    reference: Option<String>,
    fields: BTreeMap<String, String>,
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

    pub fn field(mut self, k: &str, v: &str) -> Self {
        self.fields.insert(k.to_string(), v.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PrintReceipt {
    job_id: String,
    pages: u32,
}

/// 卡数据。**只存掩码**：完整 PAN 不允许出现在这个结构体里，
/// 因为结构体一旦被 Debug/序列化，完整卡号就跟着泄露。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardData {
    pan_masked: String,
    has_track2: bool,
}

impl CardData {
    /// 构造时强制掩码，调用方没有机会传入完整卡号。
    pub fn new(pan: &str, has_track2: bool) -> Self {
        let digits: String = pan.chars().filter(|c| c.is_ascii_digit()).collect();
        // 短于 13 位不可能是合法 PAN；此时保留 6+4 会把所有数字都露出来
        // （10 位时 6+4 正好覆盖全部数字），所以整卡掩码。
        let masked = if digits.len() >= 13 {
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
///
/// **刻意不实现 Serialize/Deserialize**：Debug 脱敏只挡住日志，
/// 一旦它被序列化进 IPC 或日志字段，明文照样出去。要送主机只能走 `expose()`，
/// 让每个出网点都显式可见。
#[derive(Clone, PartialEq, Eq)]
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
    path: String,
    mime: String,
}

pub trait Printer: Send + Sync {
    /// 适配器登记的型号。白名单按它放行，必须是稳定字面量，不随运行时状态变化。
    fn model(&self) -> &'static str;
    fn status(&self) -> DeviceStatus;
    fn print(&self, voucher: &Voucher) -> Result<PrintReceipt, DeviceError>;
}

pub trait CardReader: Send + Sync {
    fn model(&self) -> &'static str;
    fn status(&self) -> DeviceStatus;
    fn read(&self) -> Result<CardData, DeviceError>;
}

pub trait PinPad: Send + Sync {
    fn model(&self) -> &'static str;
    fn status(&self) -> DeviceStatus;
    fn read_pin(&self) -> Result<PinBlock, DeviceError>;
}

pub trait Scanner: Send + Sync {
    fn model(&self) -> &'static str;
    fn status(&self) -> DeviceStatus;
    fn capture(&self, kind: &str) -> Result<CapturedImage, DeviceError>;
}

/// mock 适配器登记的型号。收敛成常量，避免 `allow_mocks()` 与实现里的
/// 字面量各写一份后漂移。
const MOCK_PRINTER_MODEL: &str = "mock-printer";
const MOCK_CARD_READER_MODEL: &str = "mock-card-reader";
const MOCK_PINPAD_MODEL: &str = "mock-pinpad";
const MOCK_SCANNER_MODEL: &str = "mock-scanner";

/// 设备型号策略。默认拒绝：没登记过的型号不准用。
///
/// 与"设备是否就绪"是两件事：就绪回答"能不能用"，本策略回答"准不准用"。
/// 两者都通过才允许产生副作用，见 [`Devices::ensure_allowed`]。
///
/// 内部不提供"默认放行"的构造：`deny_all()` 是空集合，`allow(&[])` 也是空集合，
/// 写完白名单却漏了型号，结果同样是拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePolicy {
    allowed: BTreeSet<String>,
}

impl DevicePolicy {
    /// 空策略：拒绝一切设备（默认态，防止忘记配置就放行）。
    pub fn deny_all() -> Self {
        Self {
            allowed: BTreeSet::new(),
        }
    }

    /// 只允许这些型号。空切片等价于拒绝一切，不是"不限制"。
    pub fn allow(models: &[&str]) -> Self {
        Self {
            allowed: models.iter().map(|m| (*m).to_string()).collect(),
        }
    }

    /// 演示/测试用：允许 mock 适配器的型号。
    pub fn allow_mocks() -> Self {
        Self::allow(&[
            MOCK_PRINTER_MODEL,
            MOCK_CARD_READER_MODEL,
            MOCK_PINPAD_MODEL,
            MOCK_SCANNER_MODEL,
        ])
    }

    /// 型号是否被放行。`deny_all()` 对任何输入都返回 `false`。
    pub fn permits(&self, model: &str) -> bool {
        self.allowed.contains(model)
    }
}

/// 设备集合：业务代码拿到的唯一入口。
///
/// 用四个具名字段而不是 `HashMap<DeviceKind, Box<dyn Any>>`：
/// 类型安全，且编译期就能看出"少接了一类设备"。
pub struct Devices {
    printer: Box<dyn Printer>,
    card_reader: Box<dyn CardReader>,
    pinpad: Box<dyn PinPad>,
    scanner: Box<dyn Scanner>,
}

impl Devices {
    /// 打印机入口。只暴露 trait 视图，调用方拿不到 Box 也无法替换设备。
    pub fn printer(&self) -> &dyn Printer {
        self.printer.as_ref()
    }

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

    /// 型号白名单校验：四类设备的 `model()` 都必须被策略放行。
    ///
    /// 与 [`Devices::ensure`] 互不影响：就绪但型号未登记仍然拒绝，
    /// 型号已登记但设备不就绪也仍然由 `ensure` 拒绝。两次校验都应在副作用之前完成。
    pub fn ensure_allowed(&self, policy: &DevicePolicy) -> Result<(), DeviceError> {
        // 固定按字段顺序检查四类设备：任一台未登记即拒绝，
        // 且错误必须指出种类与型号，方便现场判断该补登记还是该换设备。
        for (kind, model) in [
            (DeviceKind::Printer, self.printer.model()),
            (DeviceKind::CardReader, self.card_reader.model()),
            (DeviceKind::PinPad, self.pinpad.model()),
            (DeviceKind::Scanner, self.scanner.model()),
        ] {
            if !policy.permits(model) {
                return Err(DeviceError::NotWhitelisted {
                    kind: kind.label().to_string(),
                    model: model.to_string(),
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
    state: Option<DeviceStatus>,
    printed: std::sync::Mutex<Vec<Voucher>>,
    next_job: std::sync::atomic::AtomicU32,
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

    /// 已打印凭证的视图（不是副本：副本会让人误以为能改到内部状态）。
    pub fn printed(&self) -> std::sync::MutexGuard<'_, Vec<Voucher>> {
        self.printed.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn printed_count(&self) -> usize {
        self.printed().len()
    }

    #[cfg(test)]
    pub fn clear_printed(&self) {
        self.printed().clear();
    }
}

impl Printer for MockPrinter {
    fn model(&self) -> &'static str {
        MOCK_PRINTER_MODEL
    }

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
        self.printed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(voucher.clone());
        // 作业号用独立计数器：从 len() 派生会在清空后复用旧号，
        // 若上层拿它当对账键就会撞号。
        let n = self
            .next_job
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        Ok(PrintReceipt {
            job_id: format!("PRINT-{n}"),
            pages: 1,
        })
    }
}

pub struct MockCardReader {
    state: DeviceStatus,
    pan: String,
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
    fn model(&self) -> &'static str {
        MOCK_CARD_READER_MODEL
    }

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
    state: DeviceStatus,
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

    pub fn not_connected() -> Self {
        Self {
            state: DeviceStatus::NotConnected,
        }
    }
}

impl PinPad for MockPinPad {
    fn model(&self) -> &'static str {
        MOCK_PINPAD_MODEL
    }

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
    state: DeviceStatus,
}

impl MockScanner {
    pub fn ready() -> Self {
        Self {
            state: DeviceStatus::Ready,
        }
    }
}

impl Scanner for MockScanner {
    fn model(&self) -> &'static str {
        MOCK_SCANNER_MODEL
    }

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
        assert_eq!(printer.printed_count(), 1);
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

    // ---- T-04 对抗用例 ----

    /// 与 mock 共享同一份打印记录，用来观察"预检失败是否产生副作用"。
    struct SharedPrinter(std::sync::Arc<MockPrinter>);

    impl Printer for SharedPrinter {
        fn model(&self) -> &'static str {
            self.0.model()
        }

        fn status(&self) -> DeviceStatus {
            self.0.status()
        }

        fn print(&self, voucher: &Voucher) -> Result<PrintReceipt, DeviceError> {
            self.0.print(voucher)
        }
    }

    /// 就绪、但型号未登记为 mock 的打印机：证明"设备能用"不等于"设备准用"。
    struct UnregisteredPrinter(MockPrinter);

    impl Printer for UnregisteredPrinter {
        fn model(&self) -> &'static str {
            "acme-thermal-900"
        }

        fn status(&self) -> DeviceStatus {
            self.0.status()
        }

        fn print(&self, voucher: &Voucher) -> Result<PrintReceipt, DeviceError> {
            self.0.print(voucher)
        }
    }

    /// 打印机就绪，但读卡器、密码键盘、高拍仪分别处于故障/未连接。
    fn partly_faulty_devices() -> Devices {
        Devices {
            printer: Box::new(MockPrinter::ready()),
            card_reader: Box::new(MockCardReader {
                state: DeviceStatus::Faulty("读头脏污".into()),
                pan: "6222021234567890123".into(),
            }),
            pinpad: Box::new(MockPinPad {
                state: DeviceStatus::NotConnected,
            }),
            scanner: Box::new(MockScanner {
                state: DeviceStatus::NotConnected,
            }),
        }
    }

    /// 断言错误是 NotReady，且种类与状态文案都精确匹配。
    fn assert_not_ready(err: &DeviceError, kind: &str, state: &str) {
        match err {
            DeviceError::NotReady { kind: k, state: s } => {
                assert_eq!(k, kind);
                assert_eq!(s, state);
            }
            other => panic!("期望 NotReady({kind}/{state})，得到 {other:?}"),
        }
    }

    // S-03：空需求放行；重复声明逐次校验，不 panic、不因重复而误报。
    #[test]
    fn s03_ensure_empty_and_duplicate_requirements() {
        let faulty = partly_faulty_devices();
        assert!(
            faulty.ensure(&[]).is_ok(),
            "空需求列表当前直接放行，不校验任何设备"
        );
        // 对照：同一套设备，一旦声明就会拒绝。
        assert!(faulty.ensure(&[DeviceKind::PinPad]).is_err());

        assert!(mock_devices()
            .ensure(&[
                DeviceKind::PinPad,
                DeviceKind::PinPad,
                DeviceKind::Printer,
                DeviceKind::Printer,
            ])
            .is_ok());
        let err = faulty
            .ensure(&[DeviceKind::Printer, DeviceKind::PinPad, DeviceKind::PinPad])
            .unwrap_err();
        assert!(
            matches!(&err, DeviceError::NotReady { kind, .. } if kind == "密码键盘"),
            "重复故障设备只报第一条命中的设备：{err:?}"
        );
    }

    // S-03：错误必须指出具体是哪一类设备、处于什么状态。
    #[test]
    fn s03_ensure_names_the_faulty_kind_and_state_for_each_device() {
        let devices = partly_faulty_devices();
        assert!(devices.ensure(&[DeviceKind::Printer]).is_ok());
        assert_eq!(
            devices.status_of(DeviceKind::CardReader),
            DeviceStatus::Faulty("读头脏污".into())
        );
        assert_eq!(
            devices.status_of(DeviceKind::Scanner),
            DeviceStatus::NotConnected
        );

        let err = devices.ensure(&[DeviceKind::CardReader]).unwrap_err();
        assert_not_ready(&err, "读卡器", "故障：读头脏污");
        let err = devices.ensure(&[DeviceKind::PinPad]).unwrap_err();
        assert_not_ready(&err, "密码键盘", "未连接");
        let err = devices.ensure(&[DeviceKind::Scanner]).unwrap_err();
        assert_not_ready(&err, "高拍仪", "未连接");
    }

    // S-03：DeviceKind 是封闭枚举，"未知设备种类"在编译期无法表示；
    // 能进入系统的唯一入口是序列化边界，未知名字必须被拒绝。
    #[test]
    fn s03_unknown_device_kind_is_rejected_at_deserialization_boundary() {
        for (json, expected) in [
            ("\"printer\"", DeviceKind::Printer),
            ("\"cardreader\"", DeviceKind::CardReader),
            ("\"pinpad\"", DeviceKind::PinPad),
            ("\"scanner\"", DeviceKind::Scanner),
        ] {
            assert_eq!(serde_json::from_str::<DeviceKind>(json).unwrap(), expected);
        }
        for json in ["\"toaster\"", "\"Printer\"", "\"pin-pad\"", "null", "0"] {
            assert!(
                serde_json::from_str::<DeviceKind>(json).is_err(),
                "未知设备种类必须被拒绝：{json}"
            );
        }
    }

    // S-03：预检失败必须发生在打印之前，不能出现"已出纸但签名失败"的半截状态。
    #[test]
    fn s03_failed_ensure_prevents_half_printed_transaction() {
        let printer = std::sync::Arc::new(MockPrinter::ready());
        let devices = Devices {
            printer: Box::new(SharedPrinter(std::sync::Arc::clone(&printer))),
            card_reader: Box::new(MockCardReader::with_pan("6222021234567890123")),
            pinpad: Box::new(MockPinPad::faulty("未连接")),
            scanner: Box::new(MockScanner::ready()),
        };

        // 业务函数：先预检、后打印，? 阻断任何后续副作用。
        let result = (|| -> Result<PrintReceipt, DeviceError> {
            devices.ensure(&[DeviceKind::Printer, DeviceKind::PinPad])?;
            devices.printer.print(&voucher())
        })();

        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            DeviceError::NotReady { kind, .. } if kind == "密码键盘"
        ));
        assert!(
            printer.printed().is_empty(),
            "预检失败后打印机上不得出现任何凭证"
        );
    }

    // S-02：同一个 printer 实例在就绪/故障之间切换，下一次调用立刻反映新状态。
    #[test]
    fn s02_printer_state_change_is_visible_on_next_call() {
        let mut printer = MockPrinter::ready();
        assert_eq!(printer.print(&voucher()).unwrap().job_id, "PRINT-1");
        assert_eq!(printer.status(), DeviceStatus::Ready);

        printer.state = Some(DeviceStatus::Faulty("缺纸".into()));
        let err = printer.print(&voucher()).unwrap_err();
        assert_not_ready(&err, "打印机", "故障：缺纸");
        assert_eq!(printer.printed_count(), 1, "故障时的打印不得写入已打印列表");

        printer.state = Some(DeviceStatus::NotConnected);
        let err = printer.print(&voucher()).unwrap_err();
        assert!(
            err.to_string().contains("未连接"),
            "未连接状态必须可见：{err}"
        );

        printer.state = None; // None 等价于默认就绪
        assert_eq!(
            printer.print(&voucher()).unwrap().job_id,
            "PRINT-2",
            "job_id 由已打印条数派生：故障期间的失败不占号"
        );
        assert_eq!(printer.printed_count(), 2);
    }

    // S-02/S-03：pinpad 与读卡器的状态切换同样在下一次调用生效。
    #[test]
    fn s02_pinpad_and_card_reader_state_changes_are_visible_on_next_call() {
        let mut pinpad = MockPinPad::faulty("未连接");
        assert!(pinpad.read_pin().is_err());
        pinpad.state = DeviceStatus::Ready;
        let block = pinpad.read_pin().unwrap();
        assert!(!block.is_empty());
        assert!(!format!("{block:?}").contains(block.expose()));

        pinpad.state = DeviceStatus::Faulty("按键无响应".into());
        let err = pinpad.read_pin().unwrap_err();
        assert!(err.to_string().contains("按键无响应"));

        let mut reader = MockCardReader::with_pan("6222021234567890123");
        assert!(reader.read().is_ok());
        reader.state = DeviceStatus::NotConnected;
        let err = reader.read().unwrap_err();
        assert_not_ready(&err, "读卡器", "未连接");
    }

    // S-01：连续打印的 job_id 递增，已打印列表按顺序累积。
    #[test]
    fn s01_consecutive_prints_increment_job_id_and_append_to_printed_list() {
        let printer = MockPrinter::ready();
        let ids: Vec<String> = (0..3)
            .map(|i| {
                printer
                    .print(&voucher().field("seq", &i.to_string()))
                    .unwrap()
                    .job_id
            })
            .collect();
        assert_eq!(ids, ["PRINT-1", "PRINT-2", "PRINT-3"]);

        // printed() 返回只读视图而不是副本：副本会让人误以为改得到内部状态。
        assert_eq!(printer.printed().len(), 3);
        assert_eq!(printer.printed()[2].fields["seq"], "2");

        // 作业号用独立计数器，清空记录后不得复用旧号——
        // 否则上层拿 job_id 当对账键就会撞号。
        printer.clear_printed();
        assert_eq!(printer.printed_count(), 0);
        assert_eq!(printer.print(&voucher()).unwrap().job_id, "PRINT-4");
    }

    // S-04：不足 10 位数字整体掩码；非数字字符（含全角数字）被静默剥离。
    #[test]
    fn s04_masking_below_ten_digits_and_non_digits() {
        for (pan, expected) in [
            ("", "****"),
            ("123456789", "****"),
            ("1234-5678-9", "****"),
            ("6222-0212-3456-7890", "622202******7890"),
            ("PAN-ABCD-EFGH", "****"),
            ("６２２２０２１２３４５６７８９０", "****"),
        ] {
            let card = CardData::new(pan, false);
            assert_eq!(card.pan_masked, expected, "pan={pan}");
            if !pan.is_empty() {
                let debug = format!("{card:?}");
                assert!(!debug.contains(pan), "Debug 泄露输入 {pan}：{debug}");
            }
        }
    }

    // S-04（当前行为）：恰好 10 位时取前 6 + 后 4，覆盖全部 10 位数字，
    // 只是被 ****** 分隔；对 10 位卡号这等于没有任何隐藏。
    #[test]
    fn s04_masking_boundary_below_13_digits_is_fully_masked() {
        // 10 位时旧的 6+4 掩码会把全部数字露出来——这条边界由测试钉死。
        for pan in ["6222020123", "622202012", "62220201234"] {
            let card = CardData::new(pan, true);
            assert_eq!(card.pan_masked, "****", "不足 13 位必须整卡掩码：{pan}");
        }
        // 13 位起才允许保留前 6 后 4。
        let card = CardData::new("6222021234567", true);
        assert_eq!(card.pan_masked, "622202******4567");
    }

    // S-04：空块/非空块的 expose、字节长度与 Debug 脱敏边界。
    #[test]
    fn s04_pin_block_boundaries_and_debug_redaction() {
        let empty = PinBlock::new("");
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
        assert_eq!(empty.expose(), "", "显式 expose 是唯一取明文的入口");
        assert_eq!(format!("{empty:?}"), "PinBlock(<redacted, 0 bytes>)");

        let raw = "SECRET-PIN-BLOCK";
        let block = PinBlock::new(raw);
        assert_eq!(block.len(), raw.len());
        assert!(!block.is_empty());
        assert_eq!(block.expose(), raw, "expose 只应在送主机时显式调用");
        let debug = format!("{block:?}");
        assert!(debug.contains("redacted"));
        assert!(debug.contains(&raw.len().to_string()));
        assert!(!debug.contains(raw), "Debug 不得包含 PIN 块内容：{debug}");

        // 多字节内容：len 报字节数（6），不是字符数；Debug 同样不泄露。
        let multibyte = PinBlock::new("密码");
        assert_eq!(multibyte.len(), 6, "len 当前按字节计数");
        assert_eq!(multibyte.expose(), "密码");
        assert!(!format!("{multibyte:?}").contains("密码"));
        assert!(format!("{multibyte:?}").contains("6 bytes"));
    }

    // ---- 白名单策略：默认拒绝，未登记型号一律不放行 ----

    const MOCK_MODELS: [&str; 4] = [
        MOCK_PRINTER_MODEL,
        MOCK_CARD_READER_MODEL,
        MOCK_PINPAD_MODEL,
        MOCK_SCANNER_MODEL,
    ];

    /// 断言错误是 NotWhitelisted，且种类与型号都精确匹配。
    fn assert_not_whitelisted(err: &DeviceError, kind: &str, model: &str) {
        match err {
            DeviceError::NotWhitelisted { kind: k, model: m } => {
                assert_eq!(k, kind);
                assert_eq!(m, model);
            }
            other => panic!("期望 NotWhitelisted({kind}/{model})，得到 {other:?}"),
        }
    }

    // 默认态：拒绝一切；每台设备被拒时错误都要点名型号。
    #[test]
    fn policy_deny_all_rejects_every_device_with_model_name() {
        let devices = mock_devices();
        let policy = DevicePolicy::deny_all();

        for model in MOCK_MODELS {
            assert!(!policy.permits(model), "deny_all 不得放行 {model}");
        }
        assert!(
            !DevicePolicy::allow(&[]).permits(MOCK_PRINTER_MODEL),
            "空白名单不是\"不限制\"，必须同样拒绝"
        );

        // 逐台验证：只把这一台排除在白名单外，错误必须点名它。
        for (model, label) in [
            (MOCK_PRINTER_MODEL, "打印机"),
            (MOCK_CARD_READER_MODEL, "读卡器"),
            (MOCK_PINPAD_MODEL, "密码键盘"),
            (MOCK_SCANNER_MODEL, "高拍仪"),
        ] {
            let allowlist: Vec<&str> = MOCK_MODELS
                .iter()
                .copied()
                .filter(|m| *m != model)
                .collect();
            let err = devices
                .ensure_allowed(&DevicePolicy::allow(&allowlist))
                .unwrap_err();
            assert_not_whitelisted(&err, label, model);
        }

        let text = devices.ensure_allowed(&policy).unwrap_err().to_string();
        assert!(
            text.contains("打印机") && text.contains(MOCK_PRINTER_MODEL),
            "错误必须带种类与型号：{text}"
        );
    }

    // 单型号白名单：只放行打印机，其余三类仍被拒。
    #[test]
    fn policy_allow_single_model_only_permits_that_kind() {
        let policy = DevicePolicy::allow(&[MOCK_PRINTER_MODEL]);
        assert!(policy.permits(MOCK_PRINTER_MODEL));
        for model in [
            MOCK_CARD_READER_MODEL,
            MOCK_PINPAD_MODEL,
            MOCK_SCANNER_MODEL,
        ] {
            assert!(!policy.permits(model), "只登记了打印机，不得放行 {model}");
        }
        assert!(!policy.permits("acme-thermal-900"));

        // 四类设备都就绪，但白名单只登记了打印机：第一台未登记的读卡器被拒。
        let err = mock_devices().ensure_allowed(&policy).unwrap_err();
        assert_not_whitelisted(&err, "读卡器", MOCK_CARD_READER_MODEL);
    }

    // 演示白名单：四类 mock 全部放行，且不放行任何真机型号。
    #[test]
    fn policy_allow_mocks_permits_all_four_kinds() {
        let policy = DevicePolicy::allow_mocks();
        for model in MOCK_MODELS {
            assert!(policy.permits(model), "allow_mocks 必须放行 {model}");
        }
        assert!(
            !policy.permits("acme-thermal-900"),
            "mock 白名单不得通配真机型号"
        );

        // 型号字面量是上游约定，钉死在这里，常量改动会立刻暴露。
        assert_eq!(MockPrinter::ready().model(), "mock-printer");
        assert_eq!(MockCardReader::with_pan("6222").model(), "mock-card-reader");
        assert_eq!(MockPinPad::ready().model(), "mock-pinpad");
        assert_eq!(MockScanner::ready().model(), "mock-scanner");

        assert!(mock_devices().ensure_allowed(&policy).is_ok());
    }

    // 白名单检查与就绪检查互不影响：两者都通过才允许产生副作用。
    #[test]
    fn policy_check_is_independent_of_readiness_check() {
        // 四类设备全部就绪、就绪检查通过，但型号一台都没登记。
        let ready = mock_devices();
        assert!(ready
            .ensure(&[
                DeviceKind::Printer,
                DeviceKind::CardReader,
                DeviceKind::PinPad,
                DeviceKind::Scanner
            ])
            .is_ok());
        let err = ready
            .ensure_allowed(&DevicePolicy::allow(&["acme-thermal-900"]))
            .unwrap_err();
        assert_not_whitelisted(&err, "打印机", MOCK_PRINTER_MODEL);

        // 反向：型号全部登记，但设备没就绪——白名单放行，就绪检查照旧拒绝。
        let faulty = partly_faulty_devices();
        assert!(faulty.ensure_allowed(&DevicePolicy::allow_mocks()).is_ok());
        let err = faulty.ensure(&[DeviceKind::CardReader]).unwrap_err();
        assert_not_ready(&err, "读卡器", "故障：读头脏污");

        // 混装现场：真机就绪但型号未登记，另三台是 mock。
        let mixed = Devices {
            printer: Box::new(UnregisteredPrinter(MockPrinter::ready())),
            ..mock_devices()
        };
        assert!(mixed
            .ensure(&[
                DeviceKind::Printer,
                DeviceKind::CardReader,
                DeviceKind::PinPad,
                DeviceKind::Scanner
            ])
            .is_ok());
        let err = mixed
            .ensure_allowed(&DevicePolicy::allow_mocks())
            .unwrap_err();
        assert_not_whitelisted(&err, "打印机", "acme-thermal-900");
    }
}
