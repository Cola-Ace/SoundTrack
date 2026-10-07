<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type Route = "game" | "media" | "voice";

interface ApplicationSummary {
  id: string;
  name: string;
  executablePath: string | null;
  fileName: string | null;
  windowTitle: string | null;
  windowClass: string | null;
  pids: number[];
  sessionCount: number;
  route: Route;
  online: boolean;
  automatic: boolean;
  excluded: boolean;
  conflict: string | null;
  captureError: string | null;
  peak: number;
}

interface RouteSummary {
  route: Route;
  gain: number;
  muted: boolean;
  peak: number;
  clipping: boolean;
}

interface AppSnapshot {
  captureAvailable: boolean;
  captureError: string | null;
  fileNameFallbackEnabled: boolean;
  applications: ApplicationSummary[];
  routes: RouteSummary[];
  configPath: string;
}

interface RouteMeter {
  route: Route;
  peak: number;
  clipping: boolean;
}

const routeOrder: Route[] = ["game", "media", "voice"];
const routeMeta: Record<Route, { title: string; subtitle: string; glyph: string }> = {
  game: { title: "Game", subtitle: "游戏与默认应用", glyph: "G" },
  media: { title: "Media", subtitle: "音乐、视频与浏览器", glyph: "M" },
  voice: { title: "Voice", subtitle: "语音与通话应用", glyph: "V" },
};

const snapshot = ref<AppSnapshot | null>(null);
const meters = reactive<Record<Route, number>>({ game: 0, media: 0, voice: 0 });
const clipping = reactive<Record<Route, boolean>>({ game: false, media: false, voice: false });
const isRefreshing = ref(false);
const isUpdatingNameFallback = ref(false);
const showAddDialog = ref(false);
const selectedRoute = ref<Route>("game");
const fileNameFallbackEnabled = ref(false);
const searchQuery = ref("");
const actionError = ref("");
const excludedApplications = computed(
  () => snapshot.value?.applications.filter(application => application.excluded && matchesSearch(application)) ?? [],
);
const visibleCount = computed(
  () => snapshot.value?.applications.filter(application => !application.excluded).length ?? 0,
);

let unlistenState: UnlistenFn | undefined;
let unlistenMeters: UnlistenFn | undefined;
const controlTimers: Partial<Record<Route, ReturnType<typeof setTimeout>>> = {};

function applySnapshot(next: AppSnapshot) {
  snapshot.value = next;
  fileNameFallbackEnabled.value = next.fileNameFallbackEnabled;
  for (const route of routeOrder) {
    const value = next.routes.find(item => item.route === route);
    if (value) {
      meters[route] = value.peak;
      clipping[route] = value.clipping;
    }
  }
}

function applicationsFor(route: Route) {
  return snapshot.value?.applications.filter(application =>
    !application.excluded && application.route === route && matchesSearch(application),
  ) ?? [];
}

function matchesSearch(application: ApplicationSummary) {
  const query = searchQuery.value.trim().toLocaleLowerCase();
  if (!query) return true;
  return [
    application.name,
    application.fileName,
    application.windowTitle,
    application.windowClass,
    application.executablePath,
  ].some(value => value?.toLocaleLowerCase().includes(query));
}

function routeControls(route: Route) {
  return snapshot.value?.routes.find(item => item.route === route) ?? {
    route,
    gain: 0.5,
    muted: false,
    peak: 0,
    clipping: false,
  };
}

function formatGain(gain: number) {
  return `${Math.round(gain * 100)}%`;
}

function formatLevel(peak: number) {
  if (peak <= 0.00001) return "−∞ dB";
  return `${(20 * Math.log10(peak)).toFixed(1)} dB`;
}

function levelPercent(peak: number) {
  return `${Math.min(100, Math.max(0, peak * 100))}%`;
}

async function refresh() {
  isRefreshing.value = true;
  actionError.value = "";
  try {
    applySnapshot(await invoke<AppSnapshot>("refresh_app_state"));
  } catch (error) {
    actionError.value = String(error);
  } finally {
    isRefreshing.value = false;
  }
}

async function moveApplication(application: ApplicationSummary, route: Route) {
  actionError.value = "";
  try {
    applySnapshot(await invoke<AppSnapshot>("move_application", {
      applicationId: application.id,
      route,
    }));
  } catch (error) {
    actionError.value = String(error);
  }
}

async function toggleExcluded(application: ApplicationSummary) {
  actionError.value = "";
  try {
    applySnapshot(await invoke<AppSnapshot>("set_application_excluded", {
      applicationId: application.id,
      excluded: !application.excluded,
    }));
  } catch (error) {
    actionError.value = String(error);
  }
}

async function removeRule(application: ApplicationSummary) {
  actionError.value = "";
  try {
    applySnapshot(await invoke<AppSnapshot>("remove_application_rule", {
      applicationId: application.id,
    }));
  } catch (error) {
    actionError.value = String(error);
  }
}

function changeGain(route: Route, event: Event) {
  const controls = routeControls(route);
  const gainPercent = Number((event.target as HTMLInputElement).value);
  const current = snapshot.value;
  if (current) {
    current.routes = current.routes.map(item => item.route === route
      ? { ...item, gain: gainPercent / 100 }
      : item);
  }
  scheduleRouteControls(route, gainPercent, controls.muted);
}

async function changeFileNameFallback(event: Event) {
  isUpdatingNameFallback.value = true;
  actionError.value = "";
  try {
    applySnapshot(await invoke<AppSnapshot>("set_file_name_fallback_enabled", {
      enabled: (event.target as HTMLInputElement).checked,
    }));
  } catch (error) {
    actionError.value = String(error);
  } finally {
    isUpdatingNameFallback.value = false;
  }
}

function toggleMute(route: Route) {
  const controls = routeControls(route);
  const muted = !controls.muted;
  if (snapshot.value) {
    snapshot.value.routes = snapshot.value.routes.map(item => item.route === route
      ? { ...item, muted }
      : item);
  }
  scheduleRouteControls(route, Math.round(controls.gain * 100), muted);
}

function scheduleRouteControls(route: Route, gainPercent: number, muted: boolean) {
  clearTimeout(controlTimers[route]);
  controlTimers[route] = setTimeout(async () => {
    try {
      applySnapshot(await invoke<AppSnapshot>("set_route_controls", {
        route,
        gainPercent,
        muted,
      }));
    } catch (error) {
      actionError.value = String(error);
    }
  }, 140);
}

async function addOfflineApplication() {
  actionError.value = "";
  try {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Windows 应用程序", extensions: ["exe"] }],
    });
    if (!selected || Array.isArray(selected)) return;
    applySnapshot(await invoke<AppSnapshot>("add_offline_application", {
      path: selected,
      route: selectedRoute.value,
    }));
    showAddDialog.value = false;
  } catch (error) {
    actionError.value = String(error);
  }
}

function handleRouteChange(application: ApplicationSummary, event: Event) {
  void moveApplication(application, (event.target as HTMLSelectElement).value as Route);
}

onMounted(async () => {
  try {
    applySnapshot(await invoke<AppSnapshot>("get_app_state"));
    unlistenState = await listen<AppSnapshot>("soundtrack-state", event => applySnapshot(event.payload));
    unlistenMeters = await listen<RouteMeter[]>("route-levels", event => {
      for (const meter of event.payload) {
        meters[meter.route] = meter.peak;
        clipping[meter.route] = meter.clipping;
      }
    });
    await refresh();
  } catch (error) {
    actionError.value = String(error);
  }
});

onUnmounted(() => {
  unlistenState?.();
  unlistenMeters?.();
  for (const timer of Object.values(controlTimers)) clearTimeout(timer);
});
</script>

<template>
  <main class="app-shell min-h-screen px-5 pb-8 pt-5 text-slate-100 sm:px-8">
    <div class="mx-auto max-w-[1440px]">
      <header class="topbar flex items-center justify-between gap-5">
        <div class="flex min-w-0 items-center gap-3">
          <div class="brand-mark" aria-hidden="true">
            <span></span><span></span><span></span>
          </div>
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <h1 class="brand-title">SoundTrack</h1>
              <span class="version-pill">DESKTOP</span>
            </div>
            <p class="brand-caption">应用音频路由台</p>
          </div>
        </div>

        <div class="header-actions flex items-center gap-3">
          <div class="capture-chip" :class="snapshot?.captureAvailable ? 'is-live' : 'is-limited'">
            <span class="status-light"></span>
            <span>{{ snapshot?.captureAvailable ? "音频捕获已就绪" : (snapshot?.captureError ?? "正在连接音频服务") }}</span>
          </div>
          <button class="icon-button" type="button" title="刷新应用列表" :disabled="isRefreshing" @click="refresh">
            <span :class="{ spinning: isRefreshing }">↻</span>
          </button>
          <button class="add-button" type="button" @click="showAddDialog = true">
            <span class="add-glyph">＋</span>
            <span>添加应用</span>
          </button>
        </div>
      </header>

      <section class="workspace-heading mt-9 flex flex-wrap items-end justify-between gap-4">
        <div>
          <div class="eyebrow">ROUTING OVERVIEW</div>
          <h2>你的声音，分轨掌控</h2>
          <p>活动中的播放应用会自动进入 Game，可将应用固定到 Media 或 Voice。</p>
        </div>
        <div class="summary-count">
          <span class="summary-number">{{ visibleCount.toString().padStart(2, "0") }}</span>
          <span>个应用正在管理</span>
        </div>
      </section>

      <div v-if="actionError && !showAddDialog" class="inline-error mt-5" role="alert">
        <span>!</span>{{ actionError }}
        <button type="button" aria-label="关闭错误" @click="actionError = ''">×</button>
      </div>

      <div v-if="snapshot?.captureError" class="notice-banner mt-5">
        <span class="notice-icon">!</span>
        <div>
          <strong>音频捕获尚未就绪</strong>
          <p>{{ snapshot.captureError }}。应用列表仍会持续扫描；确认 Windows 版本和音频服务后可重试。</p>
        </div>
        <button type="button" class="quiet-button" :disabled="isRefreshing" @click="refresh">重试</button>
      </div>

      <div class="output-note mt-5">
        <span class="output-note-icon">i</span>
        <span>桌面端会把 Game、Media、Voice 三路音频提供给配套 OBS 插件；在 OBS 中添加“SoundTrack 路由音频”源即可接收。</span>
      </div>

      <label class="search-field mt-5" for="application-search">
        <span class="search-glyph">⌕</span>
        <input
          id="application-search"
          v-model="searchQuery"
          type="search"
          placeholder="搜索应用名、窗口标题或路径"
          autocomplete="off"
        />
        <button v-if="searchQuery" type="button" aria-label="清除搜索" @click="searchQuery = ''">×</button>
      </label>
      <label class="fallback-toggle mt-3">
        <input
          type="checkbox"
          :checked="fileNameFallbackEnabled"
          :disabled="isUpdatingNameFallback"
          @change="changeFileNameFallback"
        />
        <span>允许文件名回退匹配</span>
        <small>会匹配其他目录下的同名程序</small>
      </label>

      <section class="route-grid mt-6 grid grid-cols-1 gap-4 lg:grid-cols-3">
        <article v-for="route in routeOrder" :key="route" class="route-card" :class="`route-${route}`">
          <div class="route-card-head flex items-start justify-between gap-4">
            <div class="flex items-center gap-3">
              <div class="route-icon">{{ routeMeta[route].glyph }}</div>
              <div>
                <h3>{{ routeMeta[route].title }}</h3>
                <p>{{ routeMeta[route].subtitle }}</p>
              </div>
            </div>
            <span class="route-count">{{ applicationsFor(route).length.toString().padStart(2, "0") }}</span>
          </div>

          <div class="meter-row mt-6 flex items-center gap-3">
            <div class="meter-track" :class="{ muted: routeControls(route).muted, clipped: clipping[route] }">
              <div class="meter-fill" :style="{ width: levelPercent(meters[route]) }"></div>
            </div>
            <span class="meter-value" :class="{ clipping: clipping[route] }">{{ clipping[route] ? "过载" : formatLevel(meters[route]) }}</span>
          </div>

          <div class="volume-row mt-5 flex items-center gap-3">
            <button class="mute-button" type="button" :aria-label="routeControls(route).muted ? '取消静音' : '静音'" @click="toggleMute(route)">
              {{ routeControls(route).muted ? "静" : "♫" }}
            </button>
            <input
              class="volume-slider"
              type="range"
              min="0"
              max="100"
              step="1"
              :value="Math.round(routeControls(route).gain * 100)"
              :aria-label="`${routeMeta[route].title} 音量`"
              @input="changeGain(route, $event)"
            />
            <span class="volume-value">{{ formatGain(routeControls(route).gain) }}</span>
          </div>

          <div class="app-list mt-5">
            <div v-if="applicationsFor(route).length === 0" class="empty-list">
              <span class="empty-wave">〰</span>
              <span>{{ searchQuery ? "没有匹配的应用" : "等待应用播放声音" }}</span>
            </div>
            <div v-for="application in applicationsFor(route)" :key="application.id" class="application-row">
              <div class="application-icon" :class="{ offline: !application.online }">
                {{ application.name.slice(0, 1).toUpperCase() }}
              </div>
              <div class="application-info">
                <div class="application-title-line">
                  <strong :title="application.name">{{ application.name }}</strong>
                  <span v-if="application.automatic" class="auto-tag">自动</span>
                  <span v-else class="rule-tag">规则</span>
                </div>
                <div class="application-subtitle" :title="application.executablePath ?? application.windowTitle ?? ''">
                  {{ application.online ? (application.windowTitle || application.fileName || `${application.pids.length} 个进程`) : "离线规则" }}
                </div>
                <div v-if="application.conflict" class="app-warning">{{ application.conflict }}</div>
                <div v-else-if="application.captureError" class="app-warning">{{ application.captureError }}</div>
              </div>
              <div class="application-actions">
                <select
                  class="route-select"
                  :value="application.route"
                  :disabled="Boolean(application.captureError)"
                  :aria-label="`移动 ${application.name} 到路由`"
                  @change="handleRouteChange(application, $event)"
                >
                  <option value="game">Game</option>
                  <option value="media">Media</option>
                  <option value="voice">Voice</option>
                </select>
                <button class="row-icon-button" type="button" :title="application.excluded ? '恢复捕获' : '排除此应用'" @click="toggleExcluded(application)">
                  {{ application.excluded ? "↶" : "⊘" }}
                </button>
                <button v-if="!application.automatic" class="row-icon-button" type="button" title="恢复默认 Game" @click="removeRule(application)">
                  ×
                </button>
              </div>
            </div>
          </div>

          <footer class="route-card-footer">
            <span>{{ applicationsFor(route).reduce((sum, application) => sum + application.sessionCount, 0) }} 个音频会话</span>
            <span>{{ routeControls(route).muted ? "已静音" : "路由峰值电平监测" }}</span>
          </footer>
        </article>
      </section>

      <section v-if="excludedApplications.length" class="excluded-section mt-6">
        <div class="flex items-center justify-between gap-3">
          <div>
            <div class="eyebrow">EXCLUDED</div>
            <h3>已排除应用 <span>{{ excludedApplications.length }}</span></h3>
          </div>
          <span class="excluded-hint">不会进入任何路由</span>
        </div>
        <div class="excluded-list mt-3">
          <div v-for="application in excludedApplications" :key="application.id" class="excluded-item">
            <div class="application-icon muted-icon">{{ application.name.slice(0, 1).toUpperCase() }}</div>
            <div class="application-info">
              <strong>{{ application.name }}</strong>
              <span>{{ application.online ? "在线" : "离线规则" }}</span>
            </div>
            <button class="quiet-button" type="button" @click="toggleExcluded(application)">恢复捕获</button>
            <button class="row-icon-button" type="button" title="删除规则" @click="removeRule(application)">×</button>
          </div>
        </div>
      </section>

      <footer class="app-footer mt-7 flex flex-wrap items-center justify-between gap-3">
        <div class="footer-status">
          <span class="footer-dot"></span>
          <span>原声播放不受影响</span>
          <span class="footer-separator">·</span>
          <span>增益仅作用于 SoundTrack 混音</span>
        </div>
        <div class="footer-note">OBS 输出需要配套 SoundTrack 插件</div>
      </footer>
    </div>

    <div v-if="showAddDialog" class="dialog-backdrop" @click.self="showAddDialog = false">
      <section class="add-dialog" role="dialog" aria-modal="true" aria-labelledby="add-dialog-title">
        <button class="dialog-close" type="button" aria-label="关闭" @click="showAddDialog = false">×</button>
        <div class="eyebrow">ADD APPLICATION RULE</div>
        <h2 id="add-dialog-title">添加离线应用</h2>
        <p class="dialog-copy">选择应用程序文件，即使应用没有运行，也可以先设置路由规则。</p>
        <label class="field-label" for="route-choice">默认路由</label>
        <select id="route-choice" v-model="selectedRoute" class="dialog-select">
          <option value="game">Game</option>
          <option value="media">Media</option>
          <option value="voice">Voice</option>
        </select>
        <button class="file-picker-button" type="button" @click="addOfflineApplication">
          <span class="file-picker-glyph">＋</span>
          <span><strong>选择 .exe 文件</strong><small>保存完整路径规则，应用启动后自动生效</small></span>
          <span class="file-picker-arrow">→</span>
        </button>
        <p v-if="actionError" class="dialog-error" role="alert">{{ actionError }}</p>
        <p class="dialog-footnote">配置保存在 {{ snapshot?.configPath ?? "%APPDATA%\\SoundTrack\\config.json" }}</p>
      </section>
    </div>
  </main>
</template>
