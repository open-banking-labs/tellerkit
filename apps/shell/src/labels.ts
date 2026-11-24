/**
 * 按钮级权限码的展示名。
 *
 * 权限码本身是稳定的英文标识（服务端授予、业务包声明都用它），
 * 界面上给人看的是中文。缺失时回退到 code 末段，绝不隐藏错误。
 */
const ACTION_LABELS: Record<string, string> = {
  "action.deposit.print": "打印凭证",
  "action.withdraw.print": "打印凭证",
  "action.withdraw.reversal": "发起冲正",
  "action.transfer.print": "打印凭证",
  "action.customer.print": "打印开户资料",
  "action.loan.approve": "贷款发放授权",
  "action.reversal.approve": "冲正授权",
  "action.report.export": "导出报表",
};

export function actionLabel(code: string): string {
  return ACTION_LABELS[code] ?? code.split(".").slice(-1)[0];
}

/** 菜单分组/页面的补充说明（演示用，真实文案走 title_key + i18n）。 */
const MENU_NOTES: Record<string, string> = {
  "account.deposit": "现金存入，支持卡/折/无卡",
  "account.withdraw": "单笔 5 万以上需主管授权",
  "account.transfer": "行内实时到账",
  "customer.query": "按证件号 / 卡号 / 客户号检索",
  "customer.open": "开户需完成联网核查",
  "approval.queue": "待处理的授权与冲正申请",
  "approval.reversal": "异常账务的冲正处理",
};

export function menuNote(code: string): string | null {
  return MENU_NOTES[code] ?? null;
}
