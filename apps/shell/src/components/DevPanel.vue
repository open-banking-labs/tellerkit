<script setup lang="ts">
import type { BootInfo, DevState, LoginResult, UpdateReport } from "../generated";

defineProps<{
  boot: BootInfo | null;
  session: LoginResult | null;
  dev: DevState | null;
  update: UpdateReport | null;
  busy: boolean;
}>();

const emit = defineEmits<{
  (e: "check-update"): void;
  (e: "toggle-offline"): void;
  (e: "toggle-tampered"): void;
  (e: "bump-version"): void;
  (e: "corrupt"): void;
  (e: "reload"): void;
  (e: "reset"): void;
  (e: "logout"): void;
}>();
</script>

<template>
  <aside class="rail">
    <section>
      <h3>运行状态</h3>
      <div class="kv">
        <span>菜单版本</span><span>v{{ boot?.load.menu_version ?? "—" }}</span>
      </div>
      <div class="kv">
        <span>本地版本</span>
        <span>{{ dev?.local_versions.length ? dev.local_versions.map((v) => "v" + v).join(" / ") : "无" }}</span>
      </div>
      <div class="kv">
        <span>服务端版本</span><span>v{{ dev?.server_version ?? "—" }}</span>
      </div>
      <div class="kv">
        <span>加载耗时</span><span>{{ boot ? boot.load.load_ms.toFixed(1) + "ms" : "—" }}</span>
      </div>
      <div class="kv">
        <span>剪枝耗时</span><span>{{ session ? session.filter_ms.toFixed(3) + "ms" : "—" }}</span>
      </div>
      <div class="kv">
        <span>包大小</span><span>{{ boot ? boot.load.pack_bytes + " B" : "—" }}</span>
      </div>
      <div class="kv">
        <span>权限版本</span><span>{{ session?.snapshot.perm_version ?? "—" }}</span>
      </div>
    </section>

    <section>
      <h3>P0 验收</h3>
      <div class="row">
        <button class="btn" :disabled="busy" @click="emit('check-update')">版本探测</button>
        <button class="btn" :disabled="busy" @click="emit('bump-version')">服务端 +1</button>
      </div>
      <div class="row">
        <button class="btn danger" :disabled="busy" @click="emit('corrupt')">篡改本地包</button>
        <button class="btn" :disabled="busy" @click="emit('reload')">重新加载</button>
      </div>
      <div class="row">
        <button class="btn wide" :disabled="busy" @click="emit('reset')">删除本地包（模拟首启）</button>
      </div>
      <p class="hint">
        篡改本地包 → 点「重新加载」应验签失败并自动回退服务端；删除本地包 →
        重新执行启动流程，观察是否必须联网一次。
      </p>
    </section>

    <section>
      <h3>网络与下发</h3>
      <label class="switch">
        <span>模拟断网</span>
        <input type="checkbox" :checked="dev?.offline" @change="emit('toggle-offline')" />
      </label>
      <label class="switch" style="margin-top: 8px">
        <span>服务端下发坏包</span>
        <input type="checkbox" :checked="dev?.tampered" @change="emit('toggle-tampered')" />
      </label>
      <p class="hint">
        断网 + 已有本地包 = 正常启动；断网 + 无本地包 = 明确失败。勾选「坏包」后点「版本探测」可验证下载侧验签。
      </p>
    </section>

    <section>
      <h3>会话</h3>
      <div class="row">
        <button class="btn wide" @click="emit('logout')">登出并清除权限快照</button>
      </div>
      <p class="hint">
        登出会立刻丢弃内存中的权限快照；换账号后不应看到上一个账号的菜单。
      </p>
      <div v-if="update" class="kv" style="margin-top: 8px">
        <span>最近探测</span><span>{{ update.note }}</span>
      </div>
    </section>
  </aside>
</template>
