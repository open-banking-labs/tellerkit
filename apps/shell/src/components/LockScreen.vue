<script setup lang="ts">
import { ref } from "vue";

/**
 * 锁屏。遮住全部内容，只留重新登录入口。
 *
 * 刻意不用"输入密码解锁"的假流程：当前没有接真实 IAM，
 * 一个本地校验的密码框会让人误以为有身份认证。这里直接要求重新登录，
 * 语义上等价于"新会话"——真实的生物识别/密码校验在 P3.5 接 IAM 时替换。
 */
defineProps<{ reason: string | null; error: string | null }>();
const emit = defineEmits<{ (e: "reauth"): void }>();

const busy = ref(false);

async function handle(): Promise<void> {
  busy.value = true;
  try {
    emit("reauth");
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="lockscreen">
    <div class="lockscreen-card">
      <h1>会话已锁定</h1>
      <p class="lockscreen-reason">{{ reason ?? "需要重新验证身份" }}</p>
      <p class="lockscreen-hint">
        为保护客户信息，柜员离开或窗口失焦时会自动锁屏。请重新登录继续办理业务。
      </p>
      <p v-if="error" class="error-box" role="alert">{{ error }}</p>
      <button class="btn primary" :disabled="busy" @click="handle">
        {{ busy ? "正在返回登录…" : "重新登录" }}
      </button>
    </div>
  </div>
</template>
