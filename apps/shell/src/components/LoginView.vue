<script setup lang="ts">
import type { BootInfo } from "../generated";
import { ROLES } from "../api";

defineProps<{
  boot: BootInfo | null;
  busy: boolean;
  error: string;
}>();

const emit = defineEmits<{ (e: "select", roleId: string): void }>();

function sourceLabel(source?: string): string {
  if (source === "local") return "本地包";
  if (source === "network") return "服务端";
  return "—";
}
</script>

<template>
  <div class="login-wrap">
    <div class="login-card">
      <div class="login-head">
        <h1>TellerKit 柜面终端</h1>
        <p>菜单本地化 · 权限快照剪枝 · 启动零网络等待</p>
      </div>

      <div v-if="boot" class="boot-strip">
        <div class="cell">
          <div class="k">菜单版本</div>
          <div class="v">v{{ boot.load.menu_version }}</div>
        </div>
        <div class="cell">
          <div class="k">加载耗时</div>
          <div class="v" :class="boot.load.load_ms < 50 ? 'ok' : 'warn'">
            {{ boot.load.load_ms.toFixed(1) }}ms
          </div>
        </div>
        <div class="cell">
          <div class="k">数据来源</div>
          <div class="v">{{ sourceLabel(boot.load.source) }}</div>
        </div>
        <div class="cell">
          <div class="k">菜单节点</div>
          <div class="v">{{ boot.load.menu_nodes }}</div>
        </div>
      </div>

      <div v-if="error" class="error-box">{{ error }}</div>

      <div class="roles">
        <button
          v-for="role in ROLES"
          :key="role.id"
          class="role"
          :disabled="busy || !!error"
          @click="emit('select', role.id)"
        >
          <div class="r-label">{{ role.label }}</div>
          <div class="r-teller">{{ role.teller }}</div>
          <div class="r-hint">{{ role.hint }}</div>
          <div class="r-go">{{ busy ? "登录中…" : "进入 →" }}</div>
        </button>
      </div>

      <div class="login-foot">
        P0 演示环境：后端为 mock，未接入真实主机与柜面外设。菜单隐藏只是体验层，
        真实权限必须以服务端接口校验为准。
      </div>
    </div>
  </div>
</template>
