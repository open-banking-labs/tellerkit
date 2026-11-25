<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api } from "../api";
import type { Reconciliation } from "../generated";

/**
 * 日终对账。只读本地账本，以核心系统为准——这里解决"关账前先自查一遍"。
 */
const emit = defineEmits<{ (e: "close"): void }>();

const data = ref<Reconciliation | null>(null);
const error = ref<string | null>(null);
const loading = ref(true);

const LABEL: Record<string, string> = {
  committed: "已入账",
  pending: "挂起待重放",
  rejected: "已拒绝",
  marked_for_reversal: "状态不明·待冲正",
};

function yuan(cents: number): string {
  return (cents / 100).toFixed(2);
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    data.value = await api.txReconciliation();
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="reconcile-overlay" @click.self="emit('close')">
    <div class="reconcile-card">
      <header class="reconcile-head">
        <h2>日终对账</h2>
        <button class="btn" @click="load">刷新</button>
        <button class="btn" @click="emit('close')">关闭</button>
      </header>

      <p v-if="error" class="tx-msg" style="color: var(--danger)">{{ error }}</p>
      <p v-else-if="loading" class="tx-hint">正在汇总本地账本…</p>

      <template v-else-if="data">
        <p class="tx-hint">
          统计起点：<code>{{ data.since }}</code>（本地时间当日 00:00，秒级）
        </p>

        <table class="reconcile-table">
          <thead>
            <tr>
              <th>状态</th>
              <th>笔数</th>
              <th>金额（元）</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in data.summary" :key="row.status">
              <td>{{ LABEL[row.status] ?? row.status }}</td>
              <td class="num">{{ row.count }}</td>
              <td class="num">{{ yuan(row.amount_cents) }}</td>
            </tr>
            <tr v-if="data.summary.length === 0">
              <td colspan="3" class="tx-hint">当日还没有交易记录</td>
            </tr>
          </tbody>
        </table>

        <h3 class="reconcile-sub">最近 {{ data.recent.length }} 笔</h3>
        <table class="reconcile-table">
          <thead>
            <tr>
              <th>幂等键</th>
              <th>交易码</th>
              <th>金额（元）</th>
              <th>状态</th>
              <th>流水号</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="tx in data.recent" :key="tx.key">
              <td><code>{{ tx.key.slice(0, 12) }}…</code></td>
              <td>{{ tx.tx_code }}</td>
              <td class="num">{{ yuan(tx.amount_cents) }}</td>
              <td>{{ LABEL[tx.status] ?? tx.status }}</td>
              <td><code>{{ tx.reference ?? "—" }}</code></td>
            </tr>
            <tr v-if="data.recent.length === 0">
              <td colspan="5" class="tx-hint">没有可显示的明细</td>
            </tr>
          </tbody>
        </table>

        <p class="tx-hint">
          说明：这里是<strong>本地留痕</strong>，用于关账前自查；账务一致性以核心系统为准。
        </p>
      </template>
    </div>
  </div>
</template>
