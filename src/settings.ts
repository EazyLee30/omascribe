import { getViewSettings, patchViewSettings } from "./views-client";
import { translationChoices } from "./translation-ui";
import { confirmDelete } from "./interactions";
import { mountUpdates } from "./updates";
import { accelerationBody, type ComponentCatalog } from "./acceleration-ui";
import { invoke } from "@tauri-apps/api/core";
import { t, locale, type Locale } from "./i18n";
import type { Diagnostic } from "./types";
import {
  getPreferences,
  savePreferences,
  defaultAppearance,
  type Preferences,
  type ModelStatus,
  type Hardware,
  type Profile,
  type NetworkPolicy,
} from "./preferences-client";
import { esc, tr, bytes, glyph, type PageHandle } from "./workspace-ui";
interface Options {
  toast: (s: string, error?: boolean) => void;
  back: () => Promise<void>;
  language: (l: Locale) => Promise<void>;
  display: () => Promise<void>;
  device: (id: string) => void;
  section?: string;
  profile?: Profile;
}
const sections = () => [
  ["general", tr("常规", "General", "一般")],
  ["appearance", tr("外观", "Appearance", "外觀")],
  ["models", tr("模型与性能", "Models & performance", "模型與效能")],
  ["transcription", tr("转写与纪要", "Transcription & minutes", "轉寫與紀要")],
  ["profiles", tr("配置分享", "Configuration sharing", "設定分享")],
  ["privacy", tr("隐私与联网", "Privacy & network", "隱私與連網")],
  ["updates", tr("软件更新", "Software updates", "軟體更新")],
  ["shortcuts", tr("键盘快捷键", "Keyboard shortcuts", "鍵盤快捷鍵")],
  ["storage", tr("存储空间", "Storage", "儲存空間")],
];
export async function mountSettings(
  root: HTMLElement,
  options: Options,
): Promise<PageHandle> {
  let prefs = await getPreferences(),
    active = options.section || "appearance",
    alive = true,
    queue = Promise.resolve();
  let models: ModelStatus[] = [],
    hardware: Hardware | null = null,
    profile: Profile | null = options.profile || null;
  let acceleration: ComponentCatalog = {
    state: "catalog_required",
    components: [],
  };
  const readNetwork = async (): Promise<NetworkPolicy> => {
    try {
      return await invoke<NetworkPolicy>("get_network_policy");
    } catch (e) {
      options.toast(String(e), true);
      return {
        allow_model_downloads: false,
        allow_online_recommendations: false,
        allow_update_checks: false,
      };
    }
  };
  let updateCleanup: (() => void) | undefined;
  let network = await readNetwork();
  const diagnostics = await invoke<Diagnostic>("diagnostics");
  root.classList.add("workspace-page");
  root.innerHTML = `<div class="settings-layout"><aside class="settings-nav"><button class="page-back" data-settings="back">${glyph("back")}${tr("返回工作区", "Back to workspace", "返回工作區")}</button><h1>${t("settings")}</h1><label class="page-search">${glyph("search")}<input id="settings-search" placeholder="${tr("搜索设置", "Find a setting", "搜尋設定")}" aria-label="${tr("搜索设置", "Find a setting", "搜尋設定")}"></label><small class="nav-caption">${tr("此设备", "THIS DEVICE", "此裝置")}</small><nav>${sections()
    .map(
      ([id, label]) =>
        `<button data-section="${id}">${glyph(id)}${label}</button>`,
    )
    .join(
      "",
    )}</nav><p class="settings-local">${tr("本地运行 · 数据留在这台电脑", "Runs locally · Your data stays here", "本機執行 · 資料留在這台電腦")}<br>OmaScribe ${esc(diagnostics.version)}</p></aside><section class="settings-scroll"><div class="settings-content" id="settings-content"></div></section></div>`;
  const content = root.querySelector<HTMLElement>("#settings-content")!;
  const checked = (value: boolean) => (value ? "checked" : "");
  const row = (title: string, control: string, help = "") =>
    `<div class="settings-row"><div><label>${title}</label>${help ? `<p>${help}</p>` : ""}</div><div class="settings-control">${control}</div></div>`;
  const toggle = (key: keyof Preferences, label: string, help: string) =>
    row(
      label,
      `<input type="checkbox" role="switch" aria-label="${esc(label)}" data-pref="${key}" ${checked(Boolean(prefs[key]))}>`,
      help,
    );
  const choice = (
    key: keyof Preferences,
    values: [number | string, string][],
    label: string,
  ) =>
    `<select data-pref="${key}" aria-label="${esc(label)}">${values.map(([v, s]) => `<option value="${v}" ${prefs[key] === v ? "selected" : ""}>${s}</option>`).join("")}</select>`;
  function save(mutator: (p: Preferences) => void) {
    queue = queue
      .then(async () => {
        const next = structuredClone(prefs);
        mutator(next);
        prefs = await savePreferences(next);
      })
      .catch((e) => {
        options.toast(String(e), true);
        if (alive) void render();
      });
    return queue;
  }
  function header(title: string, sub: string) {
    return `<header class="page-heading"><span class="eyebrow">OMASCRIBE / ${t("settings")}</span><h1>${title}</h1><p>${sub}</p></header>`;
  }
  function modelCards() {
    return models
      .map(
        ({
          model: m,
          installed,
          installed_bytes,
          selected,
          managed,
          download: d,
        }) => {
          const working = d && ["downloading", "verifying"].includes(d.state),
            size = m.files.reduce((n, f) => n + f.bytes, 0);
          const group =
            m.kind === "asr"
              ? tr("转写", "Transcription", "轉寫")
              : m.kind === "llm"
                ? tr("纪要", "Minutes", "紀要")
                : tr("说话人分离", "Speaker separation", "說話人分離");
          const logo = m.id.startsWith("qwen3-")
            ? '<img src="/brands/qwen.png" alt="Qwen" width="24" height="24">'
            : m.kind === "asr"
              ? '<img src="/brands/sherpa.png" alt="k2 / sherpa-onnx" width="24" height="24">'
              : glyph("speakers");
          return `<article class="model-card" data-model="${m.id}"><div class="model-icon">${logo}</div><div class="model-info"><div class="model-title"><h3>${esc(m.name)}</h3>${m.bundled ? `<span class="badge">${tr("随完整安装包内置", "Bundled in full installer", "隨完整安裝包內建")}</span>` : ""}${selected ? `<span class="badge selected">${tr("已选择", "Selected", "已選擇")}</span>` : ""}</div><p>${group} · ${esc(m.format.toUpperCase())} · ${installed ? tr("存储占用", "Disk usage", "儲存佔用") : tr("文件大小", "File size", "檔案大小")} ${bytes(installed ? installed_bytes : size)} · ${m.license}</p><small>${tr("预估运行内存", "Estimated runtime memory", "預估執行記憶體")} ${(m.memory_mib / 1024).toFixed(1)} GiB · ${installed ? tr("已安装", "Installed", "已安裝") : tr("未安装", "Not installed", "未安裝")}</small>${working ? `<progress max="${d.total}" value="${d.downloaded}"></progress><small>${d.state === "verifying" ? tr("正在校验文件", "Verifying files", "正在驗證檔案") : `${bytes(d.downloaded)} / ${bytes(d.total)}`}</small>` : ""}${d?.error ? `<p class="inline-error">${tr("下载连接中断，请检查网络后重试。模型只有通过完整校验才会安装。", "Download interrupted. Check your connection and retry. Models are installed only after verification.", "下載中斷，請檢查網路後重試；模型通過驗證才會安裝。")}</p><details class="model-error-detail"><summary>${tr("技术详情", "Technical details", "技術詳情")}</summary><code>${esc(d.error)}</code></details>` : ""}</div><div class="model-actions">${working ? `<button class="outline" data-model-action="cancel_download" data-id="${m.id}">${tr("取消", "Cancel")}</button>` : !installed ? `<button class="outline" data-model-action="download_model" data-id="${m.id}">${glyph("download")}${tr("下载", "Download", "下載")}</button>` : ""}${m.kind !== "diarization" && !selected ? `<button class="outline" data-model-action="select" data-id="${m.id}" data-kind="${m.kind}">${tr("选择", "Select", "選擇")}</button>` : ""}${managed && !selected && !working ? `<button class="subtle" data-model-action="delete_model" data-id="${m.id}" aria-label="${tr("删除", "Delete", "刪除")} ${esc(m.name)}">${glyph("trash")}${tr("删除", "Delete", "刪除")}</button>` : ""}</div></article>`;
        },
      )
      .join("");
  }
  function modelBody() {
    const h = hardware!;
    const selected = models.filter((m) => m.selected);
    const modes = {
      eco: tr("节能", "Eco", "節能"),
      balanced: tr("均衡", "Balanced"),
      quality: tr("质量优先", "Quality", "品質優先"),
    };
    const backendLabels: Record<string, string> = {
      auto: tr(
        "自动选择本机后端",
        "Automatic local backend",
        "自動選擇本機後端",
      ),
      cpu: "CPU",
      metal: "Metal · Apple GPU",
      mlx: tr("MLX · 实验", "MLX · experimental", "MLX · 實驗"),
    };
    return `${header(tr("模型与性能", "Models & performance", "模型與效能"), tr("选择模型和资源档位；所有推理都在这台电脑完成。", "Choose models and a resource mode. All inference runs on this computer.", "選擇模型及資源檔位；所有推理皆在這台電腦完成。"))}
    <div class="hardware-card"><div class="hardware-title">${glyph("models")}<div><strong>${esc(h.cpu || h.arch)}</strong><p>${esc(h.os)} · ${h.arch}${h.apple_silicon ? " · Apple Silicon" : ""}</p></div><button class="subtle" data-settings="refresh-hardware">${tr("重新检测", "Detect again", "重新偵測")}</button></div>
    <div class="hardware-metrics"><div><strong>${(h.total_memory_mib / 1024).toFixed(0)} <small>GiB</small></strong><span>${tr("总内存", "Total memory", "總記憶體")}</span></div><div><strong>${h.available_memory_mib === null ? "—" : (h.available_memory_mib / 1024).toFixed(1)} <small>GiB</small></strong><span>${tr("当前可用", "Available now", "目前可用")}</span></div><div><strong>${h.cores}</strong><span>${tr("逻辑核心", "Logical cores", "邏輯核心")}</span></div><div><strong>${bytes(h.free_disk_bytes)}</strong><span>${tr("可用磁盘", "Free disk", "可用磁碟")}</span></div></div>
    <p class="setting-help">${tr("硬件检测在本机完成，不上传设备信息。推荐依据容量与运行时支持，速度需实测。", "Hardware detection is local and sends no device information. Recommendations reflect capacity and installed runtimes; speed requires measurement.", "硬體偵測在本機完成，不上傳裝置資訊；推薦依容量及執行環境，速度需實測。")}</p><p class="setting-help">${tr("本机可用后端", "Available local backends", "本機可用後端")}: ${h.backends.map((b) => esc(backendLabels[b] || b)).join(" / ")}</p></div>
    <div id="acceleration-components">${accelerationBody(h, acceleration)}</div>
    <h2 class="settings-subtitle">${tr("模型与资源搭配", "Model and resource combinations", "模型及資源搭配")}</h2><div class="preset-grid">${h.resource_presets.map(({ id, preferences: p }) => `<button data-preset="${id}" class="preset-card ${prefs.resource_mode === id ? "recommended" : ""}"><span>${modes[id as keyof typeof modes] || esc(id)}</span><strong>${esc(models.find((m) => m.model.id === p.llm_model)?.model.name || p.llm_model)}</strong><p>${esc(p.asr_model)}<br>${p.threads} ${tr("线程", "threads", "執行緒")} · ${p.context_size} tokens</p></button>`).join("")}</div>
    <div class="settings-group">${row(
      tr("推理后端", "Inference backend", "推理後端"),
      `<select data-pref="inference_backend" aria-label="${tr("推理后端", "Inference backend", "推理後端")}">${Object.entries(
        backendLabels,
      )
        .map(
          ([v, label]) =>
            `<option value="${v}" ${prefs.inference_backend === v ? "selected" : ""} ${v !== "auto" && !h.backends.includes(v) ? "disabled" : ""}>${label}${v !== "auto" && !h.backends.includes(v) ? ` · ${tr("不可用", "unavailable", "無法使用")}` : ""}</option>`,
        )
        .join("")}</select>`,
      tr(
        "Apple Silicon 可使用 Metal 加速现有 GGUF；MLX 需要独立模型和本地运行时。不会因缺少后端而调用云服务。",
        "Apple Silicon can accelerate GGUF with Metal. MLX needs its own weights and local runtime. Missing backends never trigger cloud inference.",
        "Apple Silicon 可用 Metal 加速 GGUF；MLX 需獨立模型及本機執行環境，不會改用雲端。",
      ),
    )}
    ${row(tr("资源策略", "Resource policy", "資源策略"), choice("resource_mode", Object.entries(modes), tr("资源策略", "Resource policy")), tr("节能限制生成线程与批量，批次结束释放模型。录音与转写优先。", "Eco limits generation threads and batches and unloads the model after the batch. Recording and transcription take priority.", "節能限制生成執行緒及批次，批次結束釋放模型；錄音轉寫優先。"))}
    ${row(
      tr("模型空闲保留", "Keep model ready", "模型閒置保留"),
      choice(
        "llm_keep_alive_seconds",
        [
          [0, tr("批次结束释放", "Unload after batch", "批次結束釋放")],
          [10, tr("10 秒", "10 seconds", "10 秒")],
          [30, tr("30 秒", "30 seconds", "30 秒")],
        ],
        tr("模型空闲保留", "Keep model ready"),
      ),
      tr(
        "减少连续翻译和纪要的重复启动，但会暂时保留模型内存。节能模式始终在批次结束后释放，内存紧张时立即退出。",
        "Reduces repeated model starts for translation and minutes, using memory while idle. Eco always unloads after the batch; memory pressure releases it immediately.",
        "減少翻譯及紀要重複啟動，但暫時保留模型記憶體；節能模式在批次結束後釋放，記憶體緊張時立即退出。",
      ),
    )}
    ${row(
      tr("推理线程", "Inference threads", "推理執行緒"),
      choice(
        "threads",
        [1, 2, 4, 8, 16].map((n) => [n, String(n)]),
        tr("推理线程", "Inference threads"),
      ),
    )}
    ${row(
      tr("上下文长度", "Context size", "上下文長度"),
      choice(
        "context_size",
        [2048, 4096, 8192].map((n) => [n, String(n)]),
        "Context size",
      ),
    )}
    ${row(
      tr("生成内存预算", "Writer memory budget", "生成記憶體預算"),
      choice(
        "memory_budget_mib",
        [0, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 16384, 32768].map((n) => [
          n,
          n
            ? `${n / 1024} GiB`
            : tr("按档位自动分配", "Automatic by mode", "依檔位自動分配"),
        ]),
        tr("生成内存预算", "Writer memory budget"),
      ),
      tr(
        "用于任务准入和压力暂停，不是操作系统硬上限。更长上下文和更大模型需要更多内存。",
        "Used to admit or pause jobs under pressure, not an OS-enforced memory limit. Larger models and contexts need more memory.",
        "用於任務准入及壓力暫停，並非系統硬上限；更大模型及上下文需更多記憶體。",
      ),
    )}</div>
    <div class="settings-section-title"><h2>${tr("本地模型", "Local models", "本機模型")}</h2><button class="outline" data-settings="download-selected">${tr("下载所选组合", "Download selected pair", "下載所選組合")}</button></div>
    <p class="setting-help">${tr("下载会连接模型托管站点，只获取文件，不发送录音或笔记。下载完成后断网也可使用。", "Downloads contact model hosts to fetch files; recordings and notes are never sent. Installed models work offline.", "下載會連接模型託管站點取得檔案，不傳送錄音或筆記；完成後可離線使用。")} <button class="text-button" data-section="privacy">${tr("查看联网权限", "Network permissions", "檢視連網權限")}</button></p>
    ${!diagnostics.worker_ready || !diagnostics.server_ready ? `<p class="inline-error">${tr("本地推理程序尚未就绪，请使用完整安装包或开发环境的 bin/setup。模型下载不安装运行程序。", "Local runtimes are missing. Use a full installer or bin/setup in development. Model downloads do not install runtime programs.", "本機推理程式尚未就緒，請使用完整安裝包或 bin/setup。")}</p>` : ""}
    <div id="model-list">${modelCards()}</div><p class="setting-help">${tr("所选组合", "Selected pair", "所選組合")}: ${selected.map((m) => esc(m.model.name)).join(" + ")}</p>`;
  }
  async function render() {
    updateCleanup?.();
    updateCleanup = undefined;
    const section = active;
    root.querySelectorAll<HTMLElement>("[data-section]").forEach((b) => {
      b.classList.toggle("selected", b.dataset.section === active);
      b.setAttribute(
        "aria-current",
        b.dataset.section === active ? "page" : "false",
      );
    });
    if (section === "appearance") {
      const a = prefs.appearance;
      content.innerHTML = `${header(tr("外观", "Appearance", "外觀"), tr("让工作区更适合你的阅读习惯。", "Make your workspace comfortable to read.", "讓工作區更符合你的閱讀習慣。"))}<h2 class="settings-subtitle">${tr("视觉风格", "Visual style", "視覺風格")}</h2><div class="settings-group">${row(tr("模式", "Mode"), `<div class="mode-choices">${["system", "light", "dark"].map((mode, i) => `<button class="mode-choice ${a.mode === mode ? "selected" : ""}" data-mode="${mode}" aria-pressed="${a.mode === mode}"><span class="mode-preview preview-${mode}"><i></i><b></b><em></em></span><span>${[tr("跟随系统", "System", "跟隨系統"), tr("浅色", "Light", "淺色"), tr("深色", "Dark")][i]}</span></button>`).join("")}</div>`)}</div><div class="settings-group">${row(
        tr("主题", "Theme", "主題"),
        `<select data-appearance="theme" aria-label="${tr("主题", "Theme", "主題")}">${[
          ["forest", tr("森林", "Forest")],
          ["neutral", tr("简洁", "Neutral", "簡潔")],
          ["slate", tr("岩蓝", "Slate", "岩藍")],
        ]
          .map(
            ([v, s]) =>
              `<option value="${v}" ${a.theme === v ? "selected" : ""}>${s}</option>`,
          )
          .join("")}</select>`,
      )}${row(tr("强调色", "Accent", "強調色"), `<div class="color-control"><input type="color" data-appearance="accent" value="${a.accent}" aria-label="${tr("强调色", "Accent", "強調色")}"><code>${a.accent.toUpperCase()}</code></div>`)}${[
        "background",
        "foreground",
      ]
        .map((key, i) =>
          row(
            i ? tr("前景", "Foreground") : tr("背景", "Background"),
            `<div class="color-control"><input type="color" data-appearance="${key}" value="${
              a[key as "background"] ||
              getComputedStyle(document.documentElement)
                .getPropertyValue(i ? "--text" : "--bg")
                .trim()
            }" aria-label="${i ? tr("前景", "Foreground") : tr("背景", "Background")}"><code>${a[key as "background"] || tr("自动", "Auto", "自動")}</code><button class="text-button" data-reset-color="${key}">${tr("重置", "Reset")}</button></div>`,
          ),
        )
        .join("")}${row(
        tr("字体", "Font", "字體"),
        `<select data-appearance="font" aria-label="${tr("字体", "Font", "字體")}">${[
          ["system", tr("系统", "System", "系統")],
          ["serif", tr("衬线", "Serif", "襯線")],
          ["mono", tr("等宽", "Monospace", "等寬")],
        ]
          .map(
            ([v, s]) =>
              `<option value="${v}" ${a.font === v ? "selected" : ""}>${s}</option>`,
          )
          .join("")}</select>`,
      )}</div><details class="settings-advanced"><summary>${tr("高级", "Advanced", "進階")}</summary><div class="settings-group">${row(t("fontSize"), `<input type="number" min="11" max="20" data-appearance="font_size" value="${a.font_size}" aria-label="${t("fontSize")}">`)}</div></details><button class="text-button reset-settings" data-settings="reset-appearance">${tr("重置外观为默认值", "Reset appearance to defaults", "重設為預設外觀")}</button>`;
    } else if (section === "models") {
      const values = await Promise.all([
        invoke<ModelStatus[]>("model_status"),
        invoke<Hardware>("hardware_profile"),
      ]);
      if (!alive || section !== active) return;
      [models, hardware] = values;
      acceleration = await invoke<ComponentCatalog>("acceleration_components");
      if (!alive || section !== active) return;
      content.innerHTML = modelBody();
    } else if (section === "general") {
      const devices =
        await invoke<{ id: string; name: string }[]>("list_devices");
      if (!alive || section !== active) return;
      const views = await getViewSettings();
      content.innerHTML = `${header(tr("常规", "General", "一般"), tr("语言、输入设备和工作区行为。", "Language, input device, and workspace behavior.", "語言、輸入裝置與工作區行為。"))}<div class="settings-group">${row(t("language"), `<select id="settings-language"><option value="en">English</option><option value="zh-CN">简体中文</option><option value="zh-TW">繁體中文</option></select>`)}${row(t("device"), `<select id="settings-device">${devices.map((d) => `<option value="${esc(d.id)}">${esc(d.id === "auto" ? t("defaultDevice") : d.name)}</option>`).join("")}</select>`)}</div><div class="settings-group">${row(t("displaySettings"), `<button class="outline" data-settings="display">${tr("调整", "Configure", "調整")}</button>`, tr("悬浮转写、屏幕字幕与面板布局。", "Floating transcript, screen captions, and panel layout.", "浮動轉寫、螢幕字幕與面板配置。"))}</div><div class="settings-group">${row(tr("桌面录音小助手", "Desktop recording companion", "桌面錄音小助手"), `<input id="settings-companion" type="checkbox" role="switch" ${views.companion_enabled ? "checked" : ""}>`, tr("显示在整个桌面；点击展开后再开始录音。可拖动位置。", "Appears on the desktop. Click to open controls, then start recording. Drag to reposition.", "顯示於整個桌面；點擊展開後再開始錄音，可拖曳位置。"))}${row(tr("小助手动效", "Companion animation", "助手動效"), `<input id="settings-companion-animation" type="checkbox" role="switch" ${views.companion_animation ? "checked" : ""}>`, tr("待机、眨眼和记录动作；遵循系统减少动态效果设置。", "Idle, blink and writing poses. Respects the system reduced-motion preference.", "待機、眨眼及記錄動作；遵循系統減少動態效果設定。"))}</div><div class="privacy-card">${glyph("storage")}<h2>${tr("保存在这台电脑", "Saved on this device", "儲存在這台電腦")}</h2><p>${tr("录音、转写、笔记、记忆和模型均保存在本地。导出配置不会包含会议、笔记或记忆内容。", "Audio, transcripts, notes, memory, and models are stored locally. Shared configurations do not contain meetings, notes, or memory.", "錄音、轉寫、筆記、記憶與模型皆保存在本機。分享設定不含個人內容。")}</p></div>`;
      (root.querySelector("#settings-language") as HTMLSelectElement).value =
        locale;
      (root.querySelector("#settings-device") as HTMLSelectElement).value =
        localStorage.getItem("omascribe-device") || "auto";
    } else if (section === "transcription") {
      content.innerHTML = `${header(tr("转写与纪要", "Transcription & minutes", "轉寫與紀要"), tr("新会议和音频导入的默认行为。已有会议可单独调整。", "Defaults for new meetings and imported audio. Existing meetings have their own controls.", "新會議及音訊匯入的預設行為，現有會議可個別調整。"))}<div class="settings-group">${row(
        tr("实时转写与字幕", "Live transcript & captions", "即時轉寫及字幕"),
        choice(
          "translation_mode",
          translationChoices().map(([value, label]) => [value, label]),
          tr("实时翻译模式", "Live translation mode", "即時翻譯模式"),
        ),
        tr(
          "原文：关闭翻译；双语：原文与译文对照；中译英 / 英译中：显示对应译文。使用本地模型，先显示原文，译文稍后补上；不改变音频、原文或纪要语言。",
          "Original disables translation. Bilingual shows both. ZH → EN / EN → ZH shows the target language. Local translation follows the original with a delay; it never changes audio, source text, or the minutes language.",
          "原文關閉翻譯；雙語並列原文與譯文；中譯英／英譯中顯示對應譯文。本機模型稍後補上譯文，不改動原文或紀要語言。",
        ),
      )}${row(
        tr("纪要语言", "Minutes language", "紀要語言"),
        choice(
          "minutes_language",
          [
            [
              "auto",
              tr(
                "自动（含中文时用中文）",
                "Auto (Chinese for mixed speech)",
                "自動（含中文時用中文）",
              ),
            ],
            ["zh", "中文"],
            ["en", "English"],
          ],
          tr("纪要语言", "Minutes language", "紀要語言"),
        ),
        tr(
          "影响后续整理；已由你修改并保护的段落保持原样。界面语言在「常规」中设置。",
          "Applies to subsequent writing. Your protected edits remain intact. Set the interface language in General.",
          "影響後續整理，已修改保護的段落保留。介面語言於一般設定。",
        ),
      )}${toggle("auto_notes", tr("自动整理纪要", "Write minutes automatically", "自動整理紀要"), tr("随着转写更新，自动生成带出处的会议纪要。", "Compose source-linked minutes as transcription progresses.", "隨轉寫更新，自動生成附出處的會議紀要。"))}${toggle("diarization", tr("区分说话人", "Separate speakers", "區分說話人"), tr("录音和转写结束后，使用本地声纹模型分析。可在会议中改名和修正归属。", "Analyze voices locally after recording and transcription finish. Rename people and correct assignments in each meeting.", "錄音轉寫結束後以本機聲紋模型分析，可改名及修正歸屬。"))}${row(
        tr("预计说话人数", "Expected speakers", "預計說話人數"),
        choice(
          "speaker_count",
          Array.from({ length: 17 }, (_, n) => [
            n,
            n === 0 ? tr("自动检测", "Automatic", "自動偵測") : String(n),
          ]),
          tr("预计说话人数", "Expected speakers"),
        ),
        tr(
          "人数已知时可以指定；单次分析支持最长 2 小时。多人重叠或不确定片段保留为未分配。",
          "Specify a count if known. Analysis supports recordings up to 2 hours. Ambiguous or overlapping passages remain unassigned.",
          "已知時可指定人數；單次最長 2 小時，重疊及不確定片段保留未分配。",
        ),
      )}</div><button class="outline" data-section="models">${tr("管理说话人模型", "Manage speaker models", "管理說話人模型")}</button><h2 class="settings-subtitle">${tr("辅助背景", "Supporting context", "輔助背景")}</h2><div class="settings-group">${toggle("use_knowledge", tr("使用知识库", "Use knowledge base", "使用知識庫"), tr("只检索你明确加入知识库的笔记。用于理解背景，不作为会议事实来源。", "Retrieve notes you explicitly add to the knowledge base as context, never as meeting evidence.", "僅檢索明確加入知識庫的筆記作為背景。"))}${toggle("use_memory", tr("使用记忆", "Use memory", "使用記憶"), tr("使用你维护的表达偏好与工作背景，不自动保存新记忆。", "Use your saved writing preferences and work context. New memories are never saved automatically.", "使用你維護的寫作偏好及背景，不自動新增記憶。"))}</div><p class="setting-help">${tr("已启用的专有名词帮助校正拼写并生成纪要。原识别版本保留，你手动修改的文字不会被覆盖。", "Enabled terms guide spelling corrections and minutes. Original recognition revisions are retained; your manual edits remain protected.", "啟用的專有名詞協助校正拼寫及生成紀要；原識別版本保留，手動修改不被覆蓋。")}</p>`;
    } else if (section === "profiles") {
      content.innerHTML = `${header(tr("配置分享", "Configuration sharing", "設定分享"), tr("把适合某台电脑的模型搭配与外观保存为文件。", "Save a device-specific model pair and appearance as a portable file.", "將適合某台電腦的模型搭配及外觀存為檔案。"))}<div class="settings-group">${row(tr("配置名称", "Configuration name", "設定名稱"), `<input id="profile-name" maxlength="160" value="${tr("我的离线配置", "My offline configuration", "我的離線設定")}">`)}${row(tr("导出当前配置", "Export current configuration", "匯出目前設定"), `<button class="outline" data-settings="export-profile">${glyph("upload")}${tr("导出分享文件", "Export share file", "匯出分享檔案")}</button>`, tr("包含模型 ID、推理参数和外观；不包含模型权重或个人内容。", "Includes model IDs, inference settings, and appearance. No model weights or personal content.", "包含模型 ID、推理參數及外觀，不含模型權重或個人內容。"))}${row(tr("使用别人的配置", "Use a shared configuration", "使用別人的設定"), `<button class="outline" data-settings="import-profile">${glyph("download")}${tr("导入配置文件", "Import configuration", "匯入設定檔")}</button>`, tr("先预览，再应用；缺少的模型可以随后下载。", "Preview before applying, then download any missing models.", "預覽後套用，再下載缺少的模型。"))}</div><button class="outline" data-settings="open-community">${tr("打开官网配置广场", "Open configuration gallery", "開啟官網設定廣場")} ↗</button><p class="setting-help">${tr("在浏览器中查看和投稿；本地导入无需联网。外部配置只供预览，不会自动应用或下载模型。", "Browse and contribute in your browser. Local imports work offline. External profiles are previewed, never automatically applied or downloaded.", "透過瀏覽器查看及投稿，本機匯入不需連網；外部設定僅供預覽，不自動套用或下載。")}</p>${profile ? `<div class="profile-preview"><span class="badge">${tr("待应用配置", "Configuration preview", "待套用設定")}</span><h2>${esc(profile.name)}</h2><p>${esc(profile.hardware_hint)}</p><dl><dt>${tr("转写", "Transcription", "轉寫")}</dt><dd>${esc(profile.preferences.asr_model)}</dd><dt>LLM</dt><dd>${esc(profile.preferences.llm_model)}</dd><dt>${tr("线程 / 上下文", "Threads / context", "執行緒 / 上下文")}</dt><dd>${profile.preferences.threads} / ${profile.preferences.context_size}</dd><dt>${tr("后端 / 资源策略", "Backend / resource policy", "後端 / 資源策略")}</dt><dd>${esc(profile.preferences.inference_backend)} / ${esc(profile.preferences.resource_mode)}</dd><dt>${tr("生成内存预算", "Writer memory budget", "生成記憶體預算")}</dt><dd>${profile.preferences.memory_budget_mib ? `${profile.preferences.memory_budget_mib / 1024} GiB` : tr("自动", "Automatic", "自動")}</dd><dt>${tr("外观 / 自动纪要", "Appearance / automatic minutes", "外觀 / 自動紀要")}</dt><dd>${esc(profile.preferences.appearance.theme)} · ${esc(profile.preferences.appearance.mode)} · ${profile.preferences.appearance.font_size}px / ${profile.preferences.auto_notes ? tr("启用", "On", "啟用") : tr("关闭", "Off", "關閉")}</dd><dt>${tr("说话人 / 知识库 / 记忆", "Speakers / knowledge / memory", "說話人 / 知識庫 / 記憶")}</dt><dd>${[profile.preferences.diarization, profile.preferences.use_knowledge, profile.preferences.use_memory].map((v) => (v ? tr("启用", "On", "啟用") : tr("关闭", "Off", "關閉"))).join(" / ")}</dd></dl><div class="button-row"><button class="primary" data-settings="apply-profile">${tr("应用此配置", "Apply configuration", "套用此設定")}</button><button class="subtle" data-settings="dismiss-profile">${tr("取消", "Cancel")}</button></div></div>` : ""}`;
    } else if (section === "updates") {
      updateCleanup = await mountUpdates(
        content,
        diagnostics.version,
        options.toast,
      );
    } else if (section === "privacy") {
      network = await readNetwork();
      if (!alive || section !== active) return;
      const offline = `<span class="badge">${tr("仅本机 · 不可开启上传", "Local only · no upload option", "僅本機 · 無上傳選項")}</span>`;
      const control = (key: keyof NetworkPolicy, label: string) =>
        `<input type="checkbox" role="switch" data-network="${key}" aria-label="${esc(label)}" ${network[key] ? "checked" : ""}>`;
      content.innerHTML = `${header(tr("隐私与联网", "Privacy & network", "隱私與連網"), tr("内容处理始终离线。可选联网功能单独授权，并且不会随分享配置开启。", "Content processing stays offline. Optional network features have separate device permissions that shared profiles cannot enable.", "內容處理始終離線；可選連網功能獨立授權，不隨分享設定開啟。"))}<div class="settings-group">${row(tr("录音与音频导入", "Recording & audio import", "錄音及音訊匯入"), offline)}${row(tr("转写与说话人分析", "Transcription & speaker analysis", "轉寫及說話人分析"), offline)}${row(tr("AI 纪要、知识库与记忆", "AI minutes, knowledge & memory", "AI 紀要、知識庫及記憶"), offline, tr("仅使用已安装模型。没有云端模型、遥测或内容上传接口。本机进程间通信不连接互联网。", "Uses installed models only. No cloud inference, telemetry, or content-upload endpoint. Local inter-process communication does not use the internet.", "僅使用已安裝模型，無雲端推理、遙測或內容上傳；本機程序通訊不連接網際網路。"))}</div><h2 class="settings-subtitle">${tr("可选联网功能", "Optional network features", "可選連網功能")}</h2><div class="settings-group">${row(tr("软件更新", "Software updates", "軟體更新"), `<button class="outline" data-section="updates">${tr("管理", "Manage")}</button>`, tr("单独授权，手动检查和下载 GitHub 上的签名安装包。", "Separate permission for manual checks and signed GitHub installers.", "獨立授權，手動檢查及下載 GitHub 上的簽章安裝包。"))}${row(tr("允许手动下载模型与组件", "Allow model and component downloads", "允許手動下載模型及元件"), control("allow_model_downloads", tr("允许手动下载模型与组件", "Allow model and component downloads")), tr("只在你点击下载后连接 Hugging Face / GitHub 获取已固定版本的文件并核对哈希。关闭会取消进行中的下载。", "Only your Download action fetches pinned files from Hugging Face / GitHub and verifies hashes. Disabling cancels active downloads.", "僅點擊下載後從 Hugging Face / GitHub 取得固定檔案並核對雜湊；關閉會取消下載。"))}${row(tr("允许打开在线配置广场", "Allow opening the online gallery", "允許開啟線上設定廣場"), control("allow_online_recommendations", tr("允许打开在线配置广场", "Allow opening the online gallery")), tr("由默认浏览器访问 OmaScribe 官网和 Cloudflare。网站可看到网络地址；应用不附带硬件、录音、笔记或配置。投稿需在网页中另行操作。", "Your browser visits the OmaScribe site on Cloudflare, which receives normal web connection metadata. The app sends no hardware, recordings, notes or configuration. Publishing is a separate website action.", "由瀏覽器存取 Cloudflare 上的官網，網站可見網路位址；應用不附帶硬體、錄音、筆記或設定，投稿另行操作。"))}</div><p class="setting-help">${tr("本地文件目前不由应用加密；请使用系统磁盘加密。配置文件只含设置，个人资料备份则包含正文。", "Local files are not app-encrypted; use system disk encryption. Configuration files contain settings only; private-library backups contain your content.", "本機檔案目前未由應用加密，請使用系統磁碟加密。設定檔不含正文，個人資料備份則包含正文。")}</p>`;
    } else if (section === "shortcuts") {
      const mod = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl";
      const keys = [
        ["⇧ R", "shortcutRecord"],
        ["⇧ P", "pauseResume"],
        ["N", "shortcutNew"],
        ["F", "shortcutSearch"],
        ["B / I", "formatInline"],
        ["⌥ 1 / 2 / 3", "heading"],
        ["⇧ 8 / 7 / 9", "lists"],
        ["Z / ⇧ Z", "undoRedo"],
      ];
      content.innerHTML = `${header(tr("键盘快捷键", "Keyboard shortcuts", "鍵盤快捷鍵"), tr("工作区指令，以及笔记编辑器的格式快捷键。", "Workspace commands and formatting shortcuts for the note editor.", "工作區指令及筆記編輯快捷鍵。"))}<div class="settings-group shortcuts">${keys.map(([key, label]) => row(t(label as import("./i18n").Key), `<kbd>${mod} ${mod === "Ctrl" ? key.replaceAll("⇧", "Shift").replaceAll("⌥", "Alt") : key}</kbd>`)).join("")}${[
        ["T", tr("新建会议标签", "New meeting tab", "新增會議分頁")],
        ["W", tr("关闭当前标签", "Close current tab", "關閉目前分頁")],
        ["Tab", tr("切换会议标签", "Switch meeting tabs", "切換會議分頁")],
      ]
        .map(([key, label]) => row(label, `<kbd>${mod} ${key}</kbd>`))
        .join(
          "",
        )}${row(tr("插入内容或格式", "Insert content or formatting", "插入內容或格式"), "<kbd>/</kbd>")}</div>`;
    } else if (section === "storage") {
      const s = await invoke<Record<string, number | string>>("storage_usage");
      if (!alive || section !== active) return;
      const groups = [
        [
          "recordings",
          tr("录音与会议文件", "Audio & meeting files", "錄音與會議檔案"),
        ],
        ["database", tr("笔记与转写", "Notes & transcripts", "筆記與轉寫")],
        ["models", tr("下载的模型", "Downloaded models", "下載的模型")],
        ["bundled_models", tr("内置模型", "Bundled models", "內建模型")],
        ["downloads", tr("下载缓存", "Downloads in progress", "下載快取")],
        ["exports", tr("导出文件", "Exports", "匯出檔案")],
      ];
      const deleted =
        await invoke<import("./types").Meeting[]>("deleted_meetings");
      const total = groups.reduce((n, [key]) => n + Number(s[key]), 0);
      content.innerHTML = `${header(tr("存储空间", "Storage", "儲存空間"), tr("管理这台电脑上的本地数据。", "Manage data stored on this device.", "管理這台電腦的本機資料。"))}<div class="settings-section-title"><h2>${tr("此设备已用", "Used on this device", "此裝置已用")} ${bytes(total)}</h2><button class="outline" data-settings="refresh-storage">${tr("刷新", "Refresh", "重新整理")}</button></div><div class="storage-bar">${groups.map(([key], i) => `<span style="width:${total ? (Number(s[key]) / total) * 100 : 0}%;background:var(--chart-${i})"></span>`).join("")}</div><div class="storage-legend">${groups.map(([key, label], i) => `<span><i style="background:var(--chart-${i})"></i>${label}<strong>${bytes(Number(s[key]))}</strong></span>`).join("")}</div><p class="setting-help">${esc(s.root)}<br>${tr("磁盘剩余", "Free disk space", "磁碟剩餘")}: ${bytes(Number(s.free))}<br>${tr("已统计内置模型与下载模型的实际文件大小；内置模型存放于应用安装目录。", "Includes actual file sizes of bundled and downloaded models. Bundled weights are stored in the application installation.", "已統計內建及下載模型的實際檔案大小；內建模型存放於應用安裝目錄。")}</p><div class="settings-group">${row(tr("本地数据目录", "Local data folder", "本機資料目錄"), `<button class="outline" data-settings="open-data">${tr("打开", "Open", "開啟")}</button>`)}${row(tr("模型空间", "Model storage", "模型空間"), `<button class="outline" data-section="models">${tr("管理", "Manage")}</button>`)}${row(tr("个人资料备份", "Private library export", "個人資料備份"), `<button class="outline" data-settings="export-library">${tr("导出", "Export", "匯出")}</button>`, tr("单独导出笔记、术语和记忆。此文件包含个人内容，请勿当作配置分享。", "Export notes, terms, and memory separately. This file contains personal content and is not a shareable configuration.", "此檔案含筆記、術語及記憶等個人內容，請勿作為設定分享。"))}</div><h2 class="settings-subtitle">${tr("已删除的会议", "Deleted meetings", "已刪除的會議")}</h2><p class="setting-help">${tr("录音和笔记保留在本机，可以恢复。", "Audio and notes remain on this device and can be restored.", "錄音及筆記保留於本機，可還原。")}</p><div class="settings-group">${deleted.length ? deleted.map((m) => row(esc(m.title), `<button class="outline" data-settings="restore-meeting" data-id="${m.id}">${tr("恢复", "Restore", "還原")}</button>`)).join("") : `<p class="setting-help">${tr("没有已删除的会议", "No deleted meetings", "沒有已刪除的會議")}</p>`}</div>`;
    }
  }
  async function allowDownload() {
    network = await readNetwork();
    if (network.allow_model_downloads) return true;
    active = "privacy";
    await render();
    options.toast(
      tr(
        "请先开启手动模型与组件下载权限，再选择下载。",
        "Enable model and component downloads here, then choose Download.",
        "請先開啟手動模型及元件下載權限，再選擇下載。",
      ),
    );
    return false;
  }
  async function action(event: Event) {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "button",
    );
    if (!button) return;
    if (button.dataset.section) {
      await queue;
      active = button.dataset.section;
      await render();
      root.querySelector(".settings-scroll")!.scrollTop = 0;
      return;
    }
    if (button.dataset.componentAction) {
      button.disabled = true;
      try {
        const kind = button.dataset.componentAction;
        if (kind === "check") {
          if (!(await allowDownload())) return;
          acceleration = await invoke<ComponentCatalog>(
            "check_acceleration_components",
          );
          const region = root.querySelector("#acceleration-components");
          if (region && hardware)
            region.innerHTML = accelerationBody(hardware, acceleration);
        } else if (kind === "download") {
          if (!(await allowDownload())) return;
          await invoke("download_acceleration_component", {
            id: button.dataset.id,
          });
        } else if (kind === "cancel") {
          await invoke("cancel_acceleration_download", {
            id: button.dataset.id,
          });
        } else if (kind === "test") {
          await invoke("verify_acceleration", { id: button.dataset.id });
        } else if (kind === "disable") {
          await invoke("disable_acceleration");
        } else if (kind === "cancel-test") {
          await invoke("cancel_notes");
        }
        await pollModels();
      } catch (e) {
        options.toast(
          tr(
            "暂时无法取得或安装组件。CPU 仍可使用，请稍后重试。",
            "Components could not be fetched or installed. CPU remains available; try again later.",
            "暫時無法取得或安裝元件，CPU 仍可使用，請稍後再試。",
          ),
          true,
        );
      } finally {
        if (button.isConnected) button.disabled = false;
      }
      return;
    }
    if (button.dataset.mode) {
      await save((p) => (p.appearance.mode = button.dataset.mode!));
      return render();
    }
    if (button.dataset.resetColor) {
      await save(
        (p) =>
          (p.appearance[
            button.dataset.resetColor as "background" | "foreground"
          ] = ""),
      );
      return render();
    }
    if (button.dataset.preset) {
      const selected = hardware?.resource_presets.find(
        (p) => p.id === button.dataset.preset,
      )?.preferences;
      if (!selected) return;
      await save((p) => {
        for (const key of [
          "asr_model",
          "llm_model",
          "threads",
          "context_size",
          "inference_backend",
          "resource_mode",
          "memory_budget_mib",
        ] as const)
          (p as unknown as Record<string, unknown>)[key] = selected[key];
      });
      return render();
    }
    if (button.dataset.modelAction) {
      const id = button.dataset.id!;
      button.disabled = true;
      try {
        if (
          button.dataset.modelAction === "delete_model" &&
          !(await confirmDelete(
            tr(
              "删除下载的模型？",
              "Delete downloaded model?",
              "刪除下載的模型？",
            ),
            tr(
              "模型文件将从磁盘移除，需要时可以重新下载。",
              "Remove these model files from disk. You can download them again.",
              "從磁碟移除此模型，可隨時重新下載。",
            ),
          ))
        )
          return;
        if (button.dataset.modelAction === "select")
          await save((p) => {
            p[button.dataset.kind === "asr" ? "asr_model" : "llm_model"] = id;
            if (button.dataset.kind === "llm") {
              const format = models.find((m) => m.model.id === id)?.model
                .format;
              if (format === "mlx" || p.inference_backend === "mlx")
                p.inference_backend = "auto";
            }
          });
        else {
          if (
            button.dataset.modelAction === "download_model" &&
            !(await allowDownload())
          )
            return;
          await invoke(button.dataset.modelAction, { id });
        }
        await pollModels();
      } finally {
        button.disabled = false;
      }
      return;
    }
    switch (button.dataset.settings) {
      case "restore-meeting":
        await invoke("set_meeting_deleted", {
          id: button.dataset.id,
          deleted: false,
        });
        await render();
        return;
      case "open-community":
        network = await readNetwork();
        if (!network.allow_online_recommendations) {
          active = "privacy";
          await render();
          options.toast(
            tr(
              "请先允许打开在线配置广场。",
              "Allow opening the online gallery first.",
              "請先允許開啟線上設定廣場。",
            ),
          );
        } else await invoke("open_configuration_gallery");
        break;
      case "back":
        await options.back();
        break;
      case "display":
        await options.display();
        break;
      case "reset-appearance":
        await save((p) => (p.appearance = { ...defaultAppearance }));
        await render();
        break;
      case "refresh-hardware":
      case "refresh-storage":
        await render();
        break;
      case "download-selected":
        if (!(await allowDownload())) return;
        for (const id of [
          prefs.asr_model,
          prefs.llm_model,
          ...(prefs.diarization ? ["speaker-diarization"] : []),
        ]) {
          const m = models.find((m) => m.model.id === id);
          if (
            !m?.installed &&
            !["downloading", "verifying"].includes(m?.download?.state || "")
          )
            await invoke("download_model", { id });
        }
        await pollModels();
        break;
      case "export-profile": {
        await queue;
        const path = await invoke<string | null>("export_profile", {
          name: (root.querySelector("#profile-name") as HTMLInputElement).value,
        });
        if (path)
          options.toast(`${tr("已导出", "Exported", "已匯出")}: ${path}`);
        break;
      }
      case "import-profile":
        profile = await invoke<Profile | null>("import_profile");
        await render();
        break;
      case "dismiss-profile":
        profile = null;
        await render();
        break;
      case "apply-profile":
        if (profile) {
          await queue;
          prefs = await savePreferences(profile.preferences);
          profile = null;
          active = "models";
          await render();
          options.toast(
            tr(
              "配置已应用，可下载缺少的模型。",
              "Configuration applied. Download any missing models.",
              "設定已套用，可下載缺少的模型。",
            ),
          );
        }
        break;
      case "open-data":
        await invoke("open_data_folder");
        break;
      case "export-library": {
        const path = await invoke<string | null>("export_library");
        if (path)
          options.toast(`${tr("已导出", "Exported", "已匯出")}: ${path}`);
        break;
      }
    }
  }
  async function change(event: Event) {
    const input = event.target as HTMLInputElement;
    if (input.id === "settings-language") {
      await queue;
      await options.language(input.value as Locale);
      return;
    }
    if (
      input.id === "settings-companion" ||
      input.id === "settings-companion-animation"
    ) {
      await patchViewSettings(
        input.id === "settings-companion"
          ? { companion_enabled: input.checked }
          : { companion_animation: input.checked },
      );
      return;
    }
    if (input.id === "settings-device") {
      options.device(input.value);
      return;
    }
    if (input.dataset.network) {
      const key = input.dataset.network as keyof NetworkPolicy;
      try {
        network = await invoke<NetworkPolicy>("set_network_policy", {
          policy: { ...network, [key]: input.checked },
        });
      } finally {
        await render();
      }
      return;
    }
    if (input.dataset.pref) {
      const key = input.dataset.pref as keyof Preferences;
      await save((p) => {
        (p as unknown as Record<string, unknown>)[key] =
          input.type === "checkbox"
            ? input.checked
            : ["inference_backend", "resource_mode"].includes(key)
              ? input.value
              : Number(input.value);
      });
    }
    if (input.dataset.appearance) {
      const key = input.dataset.appearance as keyof Preferences["appearance"];
      if (!input.checkValidity()) return;
      await save((p) => {
        (p.appearance as unknown as Record<string, unknown>)[key] =
          key === "font_size" ? Number(input.value) : input.value;
      });
      await render();
    }
  }
  async function pollModels() {
    if (!alive || active !== "models") return;
    const next = await invoke<ModelStatus[]>("model_status");
    if (!alive || active !== "models") return;
    if (JSON.stringify(next) !== JSON.stringify(models)) {
      models = next;
      const list = root.querySelector("#model-list");
      if (list) list.innerHTML = modelCards();
    }
    const nextAcceleration = await invoke<ComponentCatalog>(
      "acceleration_components",
    );
    if (!alive || active !== "models") return;
    const region = root.querySelector("#acceleration-components");
    if (
      region &&
      hardware &&
      JSON.stringify(nextAcceleration) !== JSON.stringify(acceleration)
    ) {
      acceleration = nextAcceleration;
      region.innerHTML = accelerationBody(hardware, acceleration);
    }
  }
  const click = (e: Event) =>
    void action(e).catch((e) => options.toast(String(e), true));
  const changed = (e: Event) =>
    void change(e).catch((e) => options.toast(String(e), true));
  root.addEventListener("click", click);
  root.addEventListener("change", changed);
  root.querySelector("#settings-search")!.addEventListener("input", (event) => {
    const q = (event.target as HTMLInputElement).value.toLowerCase();
    root
      .querySelectorAll<HTMLButtonElement>(".settings-nav [data-section]")
      .forEach((b) => (b.hidden = !b.textContent!.toLowerCase().includes(q)));
  });
  await render();
  const timer = setInterval(
    () => void pollModels().catch((e) => options.toast(String(e), true)),
    1000,
  );
  return {
    async flush() {
      await queue;
    },
    async close() {
      updateCleanup?.();
      alive = false;
      clearInterval(timer);
      await queue;
      root.removeEventListener("click", click);
      root.removeEventListener("change", changed);
      root.classList.remove("workspace-page");
    },
  };
}
