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
const printed = ref<string | null>(null);
const pending = ref(0);
const exceptions = ref<{ key: string; kind: string }[]>([]);
const reversalNote = ref<string | null>(null);
const replayNote = ref<string | null>(null);

/** 金额一律换算成"分"再发给后端：浮点进交易是事故来源。 */
const amountCents = computed(() => {
  const n = Number.parseFloat(amountYuan.value);
  if (!Number.isFinite(n) || n <= 0) return null;
  return Math.round(n * 100);
});

const statusMeta: Record<TxStatus, { label: string; cls: string }> = {
  committed: { label: "交易成功", cls: "ok" },
  pending: { label: "已挂起，待重放", cls: "warn" },
  rejected: { label: "已拒绝", cls: "danger" },
  marked_for_reversal: { label: "状态不明 · 待冲正", cls: "warn" },
};

const meta = computed(() => (result.value ? statusMeta[result.value.status] : null));

async function refreshPending(): Promise<void> {
  pending.value = await api.txPendingCount();
  exceptions.value = await api.txExceptions();
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
  replayNote.value = await api.replayPending();
  await refreshPending();
}

async function submit(): Promise<void> {
  if (!amountCents.value || busy.value) return;
  busy.value = true;
  printed.value = null;
  try {
    result.value = await api.submitTransaction({
      txCode: props.node.code,
      // 客户端生成幂等键：断网时拿不到服务端 ID
      idempotencyKey: crypto.randomUUID(),
      amountCents: amountCents.value,
      elements: {
        account: account.value,
        note: note.value,
        menu: props.node.code,
      },
      offlineCapable: false,
    });
    await refreshPending();
  } catch (err) {
    result.value = {
      status: "rejected",
      reference: null,
      message: String(err),
      replayable: false,
    };
  } finally {
    busy.value = false;
  }
}

async function printVoucher(): Promise<void> {
  if (!result.value?.reference) return;
  const receipt = await api.printVoucher({
    template: `voucher-${props.node.code.split(".").pop()}`,
    txCode: props.node.code,
    reference: result.value.reference,
    fields: {
      amount: (amountCents.value! / 100).toFixed(2),
      account: account.value,
    },
  });
  printed.value = `作业号 ${receipt.job_id} · ${receipt.pages} 页`;
}

void refreshPending();
</script>

<template>
  <div class="card">
    <h2>交易要素</h2>
    <div class="tx-form">
      <label>
        <span>金额（元）</span>
        <input v-model="amountYuan" inputmode="decimal" placeholder="0.00" />
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
        <input v-model="note" placeholder="选填" />
      </label>
    </div>
    <div class="tx-actions">
      <button class="btn primary" :disabled="!amountCents || busy" @click="submit">
        {{ busy ? "提交中…" : "提交交易" }}
      </button>
      <span class="tx-hint">
        幂等键由客户端生成；重复提交不会重复入账。
        <template v-if="pending > 0"> · 当前挂起 {{ pending }} 笔</template>
      </span>
      <button v-if="pending > 0" class="btn" @click="replay">重放挂起交易</button>
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
      <span v-if="result.replayable" class="tx-hint">已进入待重放队列</span>
    </div>
  </div>
</template>
