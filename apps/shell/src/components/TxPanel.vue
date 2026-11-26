<script setup lang="ts">
import { computed, ref } from "vue";
import { api } from "../api";
import type { MenuView, TxResult, TxStatus } from "../generated";

const props = defineProps<{ node: MenuView }>();

const amountYuan = ref("");
const account = ref("");
const note = ref("");
const busy = ref(false);
const result = ref<TxResult | null>(null);
/** 命令失败（会话/IPC/审计故障）与业务拒绝是两回事，绝不写进 result。 */
const commandError = ref<string | null>(null);
const dataError = ref<string | null>(null);
const printError = ref<string | null>(null);
const printed = ref<string | null>(null);
const submittedCents = ref<number | null>(null);
const pending = ref(0);
const exceptions = ref<{ key: string; kind: string }[]>([]);
const reversalNote = ref<string | null>(null);
const replayNote = ref<string | null>(null);
const amountTouched = ref(false);
/** 幂等键绑定填单会话：一次填单内的重试复用同一把键。 */
let idempotencyKey = crypto.randomUUID();

/** 金额按字符串逐位换分，全程不经过浮点；超两位或非法输入返回 null。 */
function parseAmountToCents(raw: string): number | null {
  const text = raw.trim().replace(/[\s,，￥¥]/g, "");
  if (!/^\d+(\.\d{1,2})?$/.test(text)) return null;
  const [yuan, fraction = ""] = text.split(".");
  const cents = BigInt(yuan) * 100n + BigInt(fraction.padEnd(2, "0"));
  if (cents <= 0n || cents > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  return Number(cents);
}

const amountCents = computed(() => parseAmountToCents(amountYuan.value));
const amountError = computed(() => {
  const text = amountYuan.value.trim().replace(/[\s,，￥¥]/g, "");
  if (!text) return amountTouched.value ? "请输入金额" : null;
  if (amountCents.value !== null) return null;
  if (/\.\d{3,}$/.test(text)) return "金额最多两位小数，请修改后提交";
  return "金额必须是大于 0 的数字，最多两位小数";
});

/** 分转展示串也用整数运算，避免 toFixed 的浮点误差。 */
function yuanFromCents(cents: number): string {
  return `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;
}

const statusMeta: Record<TxStatus, { label: string; cls: string }> = {
  committed: { label: "交易成功", cls: "ok" },
  pending: { label: "已挂起，待重放", cls: "warn" },
  rejected: { label: "已拒绝", cls: "danger" },
  marked_for_reversal: { label: "状态不明 · 待冲正", cls: "warn" },
};

const meta = computed(() => (result.value ? statusMeta[result.value.status] : null));

async function refreshPending(): Promise<void> {
  try {
    const [count, list] = await Promise.all([api.txPendingCount(), api.txExceptions()]);
    pending.value = count;
    exceptions.value = list;
    dataError.value = null;
  } catch (err) {
    dataError.value = `挂起/异常交易查询失败：${String(err)}`;
  }
}

/** 冲正必须有人授权，且要主机回执；这里只负责发起。 */
async function requestReversal(key: string): Promise<void> {
  reversalNote.value = null;
  try {
    const reference = await api.requestReversal(key, "柜员确认状态不明，申请冲正");
    reversalNote.value = `主机已受理冲正：${reference}`;
  } catch (err) {
    reversalNote.value = `冲正未完成：${String(err)}`;
  }
  await refreshPending();
}

async function replay(): Promise<void> {
  replayNote.value = null;
  try {
    replayNote.value = `重放完成：${await api.replayPending()}`;
  } catch (err) {
    replayNote.value = `重放失败：${String(err)}`;
  }
  await refreshPending();
}

async function submit(): Promise<void> {
  amountTouched.value = true;
  const cents = amountCents.value;
  if (cents === null || busy.value) return;
  busy.value = true;
  printed.value = null;
  printError.value = null;
  const key = idempotencyKey;
  try {
    result.value = await api.submitTransaction({
      txCode: props.node.code,
      // 客户端生成幂等键：断网时拿不到服务端 ID；重试必须复用
      idempotencyKey: key,
      amountCents: cents,
      elements: {
        account: account.value,
        note: note.value,
        menu: props.node.code,
      },
      offlineCapable: false,
    });
    submittedCents.value = cents;
    commandError.value = null;
    // 只有业务明确拒绝才可以换键重试；状态不明一律保留原键。
    if (result.value.status === "rejected") idempotencyKey = crypto.randomUUID();
    await refreshPending();
  } catch (err) {
    // 命令失败不等于业务拒绝：保留上次已知结果，绝不伪造 rejected。
    commandError.value = `命令失败/状态不明，请查账，勿用新键重试（幂等键 ${key.slice(0, 12)}…：${String(err)}）`;
  } finally {
    busy.value = false;
  }
}

/** 柜员明确要办新一笔时，才换键并清空填单。 */
function newForm(): void {
  amountYuan.value = "";
  account.value = "";
  note.value = "";
  result.value = null;
  submittedCents.value = null;
  commandError.value = null;
  printError.value = null;
  printed.value = null;
  replayNote.value = null;
  amountTouched.value = false;
  idempotencyKey = crypto.randomUUID();
}

async function printVoucher(): Promise<void> {
  if (!result.value?.reference) return;
  printError.value = null;
  try {
    const receipt = await api.printVoucher({
      template: `voucher-${props.node.code.split(".").pop()}`,
      txCode: props.node.code,
      reference: result.value.reference,
      fields: {
        amount: yuanFromCents(submittedCents.value ?? amountCents.value ?? 0),
        account: account.value,
      },
    });
    printed.value = `作业号 ${receipt.job_id} · ${receipt.pages} 页`;
  } catch (err) {
    printError.value = `打印凭证失败：${String(err)}`;
  }
}

void refreshPending();
</script>

<template>
  <div class="card">
    <h2>交易要素</h2>
    <p v-if="dataError" class="tx-error" role="alert">{{ dataError }}</p>
    <div v-if="commandError" class="error-box" role="alert">
      {{ commandError }}
    </div>
    <div class="tx-form">
      <label>
        <span>金额（元）</span>
        <input
          v-model="amountYuan"
          inputmode="decimal"
          placeholder="0.00"
          :aria-invalid="!!amountError"
          @blur="amountTouched = true"
        />
      </label>
      <label>
        <span>账号 / 卡号</span>
        <input
          v-model="account"
          placeholder="6222…"
          data-sensitive
          data-sensitive-label="账号/卡号"
        />
      </label>
      <label class="wide">
        <span>备注</span>
        <input v-model="note" placeholder="选填" data-sensitive data-sensitive-label="备注" />
      </label>
    </div>
    <p v-if="amountError" class="tx-error" role="alert">{{ amountError }}</p>
    <div class="tx-actions">
      <button class="btn primary" :disabled="!amountCents || busy" @click="submit">
        {{ busy ? "提交中…" : "提交交易" }}
      </button>
      <span class="tx-hint">
        幂等键由客户端生成；一次填单内重试复用同一把键。
        <template v-if="pending > 0"> · 当前挂起 {{ pending }} 笔</template>
      </span>
      <button v-if="pending > 0" class="btn" @click="replay">重放挂起交易</button>
      <button v-if="result || commandError" class="btn" @click="newForm">新交易（换幂等键）</button>
    </div>
  </div>

  <p v-if="replayNote" class="tx-hint" style="margin-top: 8px">{{ replayNote }}</p>

  <div v-if="exceptions.length" class="tx-exceptions">
    <p class="tx-hint">
      <strong>{{ exceptions.length }}</strong> 笔交易状态不明，需查账确认后冲正：
    </p>
    <div v-for="item in exceptions" :key="item.key" class="tx-exception-row">
      <code>{{ item.key.slice(0, 12) }}…</code>
      <span class="tx-hint">{{ item.kind === "marked_for_reversal" ? "待冲正" : "待推进" }}</span>
      <button class="btn" :disabled="busy" @click="requestReversal(item.key)">申请冲正</button>
    </div>
    <p v-if="reversalNote" class="tx-hint">{{ reversalNote }}</p>
  </div>

  <div v-if="result && meta" class="card">
    <h2>交易结果</h2>
    <div class="tx-result">
      <span class="tag" :class="meta.cls">{{ meta.label }}</span>
      <span class="mono tx-ref">{{ result.reference ?? "无流水号" }}</span>
    </div>
    <p class="tx-msg">{{ result.message }}</p>
    <div class="tx-actions">
      <button
        v-if="result.status === 'committed'"
        class="btn"
        :disabled="!!printed"
        @click="printVoucher"
      >
        打印凭证
      </button>
      <span v-if="printed" class="tx-hint mono">{{ printed }}</span>
      <span v-if="printError" class="tx-error" role="alert">{{ printError }}</span>
      <span v-if="result.replayable" class="tx-hint">已进入待重放队列</span>
    </div>
  </div>
</template>
