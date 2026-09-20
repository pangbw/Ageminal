<script setup lang="ts">
// ─────────────────────────────────────────────────────────────────────────────
// #48 探针：Windows 上**点击桌面通知**能否回到应用
//
// 背景：官方 `tauri-plugin-notification` 在 Windows 上**没有任何「点击通知」回调的文档**
//       （Actions API 仅移动端）。本探针同时试两条路径并记录各自是否收到回调：
//         A. Rust 侧发通知（`tauri_plugin_notification`）—— 与 G2 #32「插件只从 Rust 调」一致
//         B. Web `Notification` API（插件 JS 侧其实就是它）
//
// ⚠️ 两个前提，缺一结论无效：
//   1) 必须用 **NSIS 安装后的 exe**（官方注明 Windows 通知仅对「已安装的应用」正常）
//   2) **Web API 路径必须先授权**（未授权时浏览器会静默不投递 → onclick 永远不会来）
//   3) 「监听注册」需要 capability 里有通知权限（否则会看到 registerListener not allowed by ACL）
// ─────────────────────────────────────────────────────────────────────────────
import { onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, onAction, onNotificationReceived, requestPermission } from "@tauri-apps/plugin-notification";
import { setProbeResult } from "../probeStore";

const events = ref<string[]>([]);
const permission = ref<string>("(未检查)");
const listenerOk = ref<boolean | null>(null);
const isTauri = "__TAURI_INTERNALS__" in window;
const checklist = [
  "① 确认顶部构建标记 = H4（不是 H3 或更早）",
  "② 「请求权限」直到显示 granted（Web API 路径的前提）",
  "③ 确认日志里出现『已注册 onAction / onNotificationReceived』（没有 = capability 仍旧）",
  "④ 「发送（Rust 路径）」→ **点那条通知** → 看是否有回调",
  "⑤ 「发送（Web API）」→ **点那条通知** → 看 onclick 是否触发",
  "⑥ 记录：点击后应用是否被激活/聚焦",
];

let listeners: Array<{ unregister: () => unknown }> = [];
let t0 = 0;

function log(msg: string): void {
  const t = Math.round(performance.now() - t0);
  events.value.push(`[t=${t}ms] ${msg}`);
  sync();
}

function sync(): void {
  setProbeResult("notify", {
    isTauri,
    permission: permission.value,
    listenerRegistered: listenerOk.value,
    jsNotificationPermission: typeof Notification !== "undefined" ? Notification.permission : "(no Notification API)",
    events: events.value,
    note: "Windows 上通知仅对『已安装的应用』正常；Web API 路径必须先授权",
    questions: [
      "A. Rust 路径发出后，点击通知是否触发任何 JS 回调？",
      "B. Web Notification 路径的 onclick 是否触发？",
      "C. onAction(onActionPerformed) 在 Windows 是否触发？",
      "D. 点击是否能激活/聚焦应用（默认行为）？",
    ],
  });
}

async function checkPermission(): Promise<boolean> {
  try {
    const granted = await isPermissionGranted();
    permission.value = granted ? "granted" : "not-granted";
    log(`isPermissionGranted() = ${granted}；Notification.permission = ${typeof Notification !== "undefined" ? Notification.permission : "n/a"}`);
    return granted;
  } catch (e) {
    permission.value = `error: ${String(e)}`;
    log(`isPermissionGranted 失败：${String(e)}`);
    return false;
  }
}

async function askPermission(): Promise<void> {
  try {
    const p = await requestPermission();
    permission.value = String(p);
    log(`requestPermission() = ${String(p)}`);
  } catch (e) {
    log(`requestPermission 失败：${String(e)}`);
  }
}

async function sendViaRust(): Promise<void> {
  try {
    await invoke("send_test_notification", {
      title: "Ageminal 通知测试（Rust 路径）",
      body: "请点击这条通知，然后看下方是否出现任何回调",
    });
    log("已发送（Rust 路径）→ 现在去点它");
  } catch (e) {
    log(`Rust 发送失败：${String(e)}`);
  }
}

function sendViaWeb(): void {
  if (typeof Notification === "undefined") {
    log("❌ 本环境没有 Web Notification API");
    return;
  }
  if (Notification.permission !== "granted") {
    log(`⚠️ Web API 未授权（Notification.permission=${Notification.permission}）→ 不会投递，先点「请求权限」。`);
    return;
  }
  try {
    const n = new Notification("Ageminal 通知测试（Web API）", {
      body: "请点击这条通知，观察 onclick 是否触发",
      tag: `agm-web-${Date.now()}`,
    });
    n.onclick = () => log("✅ web Notification.onclick 触发");
    n.onclose = () => log("web Notification.onclose 触发");
    n.onerror = () => log("web Notification.onerror 触发");
    n.onshow = () => log("web Notification.onshow 触发");
    log("已发送（Web API）→ 现在去点它");
  } catch (e) {
    log(`Web 发送失败：${String(e)}`);
  }
}

onMounted(async () => {
  t0 = performance.now();
  log(`Tauri=${isTauri}`);
  if (!isTauri) {
    log("非 Tauri 环境，仅记录；请用**安装后**的 exe 运行");
    return;
  }
  // 自动尝试注册监听：失败会在日志里显式暴露（多半是 capability 没同步）
  try {
    listeners.push(await onAction((n) => log(`✅ onAction 触发：${JSON.stringify(n)}`)));
    listeners.push(await onNotificationReceived((n) => log(`onNotificationReceived：${JSON.stringify(n)}`)));
    listenerOk.value = true;
    log("✅ 已注册 onAction / onNotificationReceived");
  } catch (e) {
    listenerOk.value = false;
    log(`❌ 注册监听失败：${String(e)}`);
    log("   → ⚠️ 这**不是配置问题**：官方插件在 **desktop** 只注册 notify / request_permission / is_permission_granted 三个命令（已查源码），**没有 register_listener**；Tauri 把「未知命令」统一报成 ACL 拒绝。结论：Windows 上拿不到 onAction / onNotificationReceived。");
  }
  const granted = await checkPermission();
  if (!granted) {
    log("权限未授予 → 自动请求一次…");
    await askPermission();
  }
  log("就绪。按左侧清单 ①→⑥ 走一遍，最后点顶部「导出报告」。");
  sync();
});

onBeforeUnmount(() => {
  for (const l of listeners) {
    try {
      l.unregister();
    } catch {
      /* ignore */
    }
  }
  listeners = [];
});
</script>

<template>
  <section class="probe">
    <header class="probe-head">
      <strong>#48 · 通知点击探针</strong>
      <span class="muted">Windows toast 激活 · 两条路径对照</span>
      <span class="spacer" />
      <span class="muted">权限：{{ permission }}</span>
      <span v-if="listenerOk === true" class="ok">监听已注册</span>
      <span v-else-if="listenerOk === false" class="bad">监听注册失败（见日志）</span>
    </header>

    <div class="toolbar-ish">
      <button @click="checkPermission">检查权限</button>
      <button @click="askPermission">请求权限</button>
      <button @click="sendViaRust">发送（Rust 路径）</button>
      <button @click="sendViaWeb">发送（Web API）</button>
      <button @click="events = []">清空日志</button>
    </div>

    <div class="warn">
      ⚠️ 必须用 **NSIS 安装后的 exe** 跑（官方注明 Windows 通知仅对「已安装的应用」正常）；
      且 **Web API 路径必须先授权**，否则浏览器静默不投递、`onclick` 永远不会来。
    </div>

    <div class="cols">
      <div class="card">
        <h3>操作清单</h3>
        <ol class="cl">
          <li v-for="(c, i) in checklist" :key="i">{{ c }}</li>
        </ol>
      </div>
      <div class="card">
        <h3>回调日志</h3>
        <pre class="logbox">{{ events.join("\n") }}</pre>
      </div>
    </div>
  </section>
</template>

<style scoped>
.probe { padding: 8px 10px; }
.probe-head { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; }
.toolbar-ish { display: flex; gap: 8px; margin-bottom: 8px; }
.warn { border: 1px solid #6b4a00; background: #1d1600; color: #e5c07b; border-radius: 6px; padding: 8px 10px; margin-bottom: 8px; font-size: 12px; }
.cols { display: flex; gap: 10px; }
.card { flex: 1; min-width: 0; }
.cl { margin: 0; padding-left: 18px; font-size: 12px; line-height: 1.7; color: #b0b0b0; }
.logbox { max-height: 48vh; overflow: auto; margin: 0; font-size: 11px; line-height: 1.6; white-space: pre-wrap; color: #b0b0b0; }
.ok { color: #a5e075; font-size: 12px; }
.bad { color: #ff616e; font-size: 12px; }
</style>
