import { readFile, writeFile, mkdir, readdir, cp, rm } from "node:fs/promises";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import { createDesign } from "../src/design.mjs";
import {
  validateProfile,
  validateSubmission,
  deepLink,
} from "../../community/profile-schema.mjs";
import { LANGS, t, pathFor, copy } from "../src/copy.mjs";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(root, "..");
const out = resolve(root, "dist");
const json = async (p) => JSON.parse(await readFile(p, "utf8"));
const e = (v) =>
  String(v ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const external = (href, label) =>
  `<a href="${e(href)}" target="_blank" rel="noreferrer">${e(label)} <span aria-hidden="true">↗</span></a>`;
const size = (n) =>
  n >= 1024 ** 3
    ? `${(n / 1024 ** 3).toFixed(2)} GiB`
    : `${(n / 1024 ** 2).toFixed(0)} MiB`;
const catalog = await json(resolve(repo, "model-catalog.json"));
const releases = await json(resolve(root, "data/releases.json"));
if (catalog.schema !== 1 || !Array.isArray(catalog.models))
  throw Error("Invalid model catalog");
const modelIds = new Set();
for (const model of catalog.models) {
  if (
    !/^[a-z0-9][a-z0-9.-]{1,99}$/.test(model.id) ||
    modelIds.has(model.id) ||
    !["asr", "llm", "diarization"].includes(model.kind)
  )
    throw Error("Invalid or duplicate catalog model");
  modelIds.add(model.id);
  const source = new URL(model.source);
  if (
    source.protocol !== "https:" ||
    !["github.com", "huggingface.co", "k2-fsa.github.io"].includes(
      source.hostname,
    ) ||
    source.username ||
    source.password
  )
    throw Error("Untrusted model source link");
  if (
    !Number.isFinite(model.memory_mib) ||
    model.memory_mib <= 0 ||
    !Array.isArray(model.files) ||
    !model.files.length ||
    model.files.some((f) => !Number.isSafeInteger(f.bytes) || f.bytes < 1)
  )
    throw Error("Invalid model size metadata");
}
if (
  releases.schema !== 1 ||
  !/^v\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(releases.tag) ||
  releases.version !== releases.tag.slice(1) ||
  !Number.isFinite(Date.parse(releases.fetched_at)) ||
  !Number.isFinite(Date.parse(releases.published_at))
)
  throw Error("Invalid release snapshot");
const releaseNames = {
  mac_arm: `OmaScribe_${releases.version}_macOS-AppleSilicon.dmg`,
  mac_intel: `OmaScribe_${releases.version}_macOS-Intel.dmg`,
  windows: `OmaScribe_${releases.version}_Windows-x64.exe`,
  checksums: "SHA256SUMS.txt",
};
for (const [key, name] of Object.entries(releaseNames)) {
  const a = releases.assets?.[key];
  if (
    !a ||
    a.name !== name ||
    !Number.isSafeInteger(a.size) ||
    a.size < 1 ||
    a.url !==
      `https://github.com/EazyLee30/omascribe/releases/download/${releases.tag}/${name}`
  )
    throw Error("Invalid release asset snapshot");
}
if (
  releases.url !==
  `https://github.com/EazyLee30/omascribe/releases/tag/${releases.tag}`
)
  throw Error("Invalid release page URL");

const profiles = [];
for (const folder of ["profiles", "submissions"])
  for (const file of (await readdir(resolve(repo, "community", folder))).filter(
    (f) => f.endsWith(".json"),
  )) {
    const entry = validateSubmission(
      await json(resolve(repo, "community", folder, file)),
      catalog,
      folder === "profiles",
    );
    if (file !== `${entry.id}.json`)
      throw Error(`Submission filename must match ID: ${file}`);
    if (folder === "profiles") profiles.push(entry);
  }
const benchmarks = [];
let benchmarkFiles = [];
try {
  benchmarkFiles = (await readdir(resolve(repo, "docs/benchmarks"))).filter(
    (f) => f.endsWith(".json"),
  );
} catch (error) {
  if (error.code !== "ENOENT") throw error;
}
for (const file of benchmarkFiles) {
  const b = await json(resolve(repo, "docs/benchmarks", file));
  if (
    b.schema !== 1 ||
    !/^[a-z0-9-]{3,100}$/.test(b.id) ||
    !b.hardware ||
    !b.method ||
    !Array.isArray(b.runs) ||
    !Array.isArray(b.summary) ||
    !Array.isArray(b.limitations)
  )
    throw Error(`Invalid benchmark ${file}`);
  for (const run of b.runs) {
    if (
      !catalog.models.some((m) => m.id === run.model_id) ||
      !["cpu", "metal", "mlx"].includes(run.backend)
    )
      throw Error(`Unsupported benchmark model/backend: ${file}`);
    for (const key of [
      "generation_tps",
      "ttft_ms",
      "elapsed_ms",
      "peak_rss_mib",
    ])
      if (
        run[key] !== null &&
        run[key] !== undefined &&
        (!Number.isFinite(run[key]) || run[key] < 0)
      )
        throw Error(`Invalid benchmark metric ${key}: ${file}`);
    if (
      !Number.isInteger(run.generated_tokens) ||
      run.generated_tokens < 1 ||
      !/^[0-9a-f]{64}$/.test(run.model_sha256)
    )
      throw Error(`Incomplete benchmark measurement: ${file}`);
  }
  for (const summary of b.summary) {
    const measured = b.runs.filter(
      (r) => r.backend === summary.backend && r.model_id === summary.model_id,
    );
    if (!measured.length)
      throw Error(`Summary without raw measurements: ${file}`);
    const median = (values) => {
      const sorted = values.sort((a, b) => a - b);
      return sorted.length % 2
        ? sorted[(sorted.length - 1) / 2]
        : (sorted[sorted.length / 2 - 1] + sorted[sorted.length / 2]) / 2;
    };
    for (const [field, key] of [
      ["median_generation_tps", "generation_tps"],
      ["median_ttft_ms", "ttft_ms"],
    ]) {
      const expected = median(measured.map((r) => r[key]));
      if (
        !Number.isFinite(summary[field]) ||
        Math.abs(expected - summary[field]) > Math.max(0.11, expected * 0.001)
      )
        throw Error(`Summary does not match raw ${key}: ${file}`);
    }
  }
  benchmarks.push(b);
}
for (const p of profiles)
  for (const id of p.benchmark_ids)
    if (!benchmarks.some((b) => b.id === id))
      throw Error(`Missing benchmark ${id} for ${p.id}`);
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
await cp(resolve(root, "public/assets"), resolve(out, "assets"), {
  recursive: true,
});
for (const file of ["site.css", "_headers"])
  await cp(resolve(root, "public", file), resolve(out, file));
await cp(
  resolve(repo, "community/profile-schema.mjs"),
  resolve(out, "profile-schema.js"),
);
await mkdir(resolve(out, "data/profiles"), { recursive: true });
await mkdir(resolve(out, "data/benchmarks"), { recursive: true });
const publicCatalog = {
  schema: catalog.schema,
  models: catalog.models.map((m) => ({
    id: m.id,
    name: m.name,
    kind: m.kind,
    format: m.format || (m.kind === "llm" ? "gguf" : "onnx"),
    memory_mib: m.memory_mib,
    bytes: m.files.reduce((n, f) => n + f.bytes, 0),
    bundled: m.bundled,
    license: m.license,
    source: m.source,
  })),
};
await writeFile(
  resolve(out, "data/catalog.json"),
  JSON.stringify(publicCatalog, null, 2),
);
await writeFile(
  resolve(out, "data/profiles.json"),
  JSON.stringify(profiles, null, 2),
);
await writeFile(
  resolve(out, "data/releases.json"),
  JSON.stringify(releases, null, 2),
);
for (const p of profiles)
  await writeFile(
    resolve(out, `data/profiles/${p.id}.json`),
    JSON.stringify(p.profile, null, 2),
  );
for (const b of benchmarks)
  await writeFile(
    resolve(out, `data/benchmarks/${b.id}.json`),
    JSON.stringify(b, null, 2),
  );
function note(lang) {
  return `<p class="notice">${t(lang, "配置、说话人与加速功能对应当前开发版。下载页标明已发布安装包的版本；旧版可能不支持配置链接。", "設定、說話人與加速功能對應目前開發版。下載頁標明已發布安裝包的版本；舊版可能不支援設定連結。", "Configuration sharing, speaker tools and acceleration describe the current development build. See Downloads for released installer versions; older versions may not support app links.")}</p>`;
}
const { button, header, footer, home, downloadChooser } = createDesign({
  e,
  t,
  pathFor,
  copy,
  LANGS,
  releases,
  size,
  external,
});
function heading(lang, eyebrow, title, description) {
  return `<div class="page-heading"><p class="eyebrow">${e(eyebrow)}</p><h1>${e(title)}</h1><p class="lede">${e(description)}</p></div>`;
}
function download(lang) {
  return `${heading(lang, "DOWNLOAD", copy.titles[lang][1], t(lang, "安装包包含默认离线模型。选择与你的处理器匹配的版本。", "安裝包包含預設離線模型。選擇與處理器相符的版本。", "Installers include the default offline models. Choose the build matching your processor."))}<div class="release-bar"><span class="badge">${e(releases.tag)} · ${releases.prerelease ? t(lang, "预发布", "預發布", "Prerelease") : t(lang, "正式版", "正式版", "Stable")}</span><span>${t(lang, "发布于", "發布於", "Published")} ${e(releases.published_at.slice(0, 10))}</span>${external(releases.url, t(lang, "更新日志", "更新紀錄", "Release notes"))}</div>${downloadChooser(lang)}<section class="text-section"><h2>${t(lang, "版本与安装说明", "版本與安裝說明", "Version and installation notes")}</h2><p>${t(lang, "此处显示的是 GitHub 上已经发布且三个安装包齐全的版本。新配置广场、说话人设置与 Apple Silicon 加速需要包含这些功能的新版或源码构建，不能通过下载旧版自动获得。", "此處顯示 GitHub 上已發布且三個安裝包齊全的版本。新設定廣場、說話人設定及 Apple Silicon 加速需要含這些功能的新版或原始碼建置，下載舊版不會自動取得。", "This page lists a published GitHub release with all three installers present. The new configuration gallery, speaker settings and Apple Silicon acceleration require a build containing those features; downloading an older release does not add them.")}</p><p>${t(lang, "当前预览包使用 macOS 临时签名，尚未公证；Windows 尚无发行者签名。安装前请阅读原生测试版说明。", "目前預覽包使用 macOS 臨時簽章，尚未公證；Windows 尚無發行者簽章。安裝前請閱讀原生測試版說明。", "Current preview packages use ad-hoc signing on macOS and are not notarized; Windows packages have no publisher signature. Read the native testing guide before installation.")}</p><div class="actions">${external(releases.assets.checksums.url, "SHA-256")}${external("https://github.com/EazyLee30/omascribe/blob/main/docs/NATIVE_TESTING.md", t(lang, "安装与验证说明", "安裝與驗證說明", "Installation and verification"))}</div><p class="small muted">${t(lang, "版本数据核对时间", "版本資料核對時間", "Release metadata checked")} ${e(releases.fetched_at.slice(0, 10))} · GitHub Releases API</p></section>`;
}
function modelCards(lang) {
  return publicCatalog.models
    .map(
      (m) =>
        `<article class="model-card"><div class="model-title">${m.id.startsWith("qwen") ? '<img class="model-logo" src="/assets/brands/qwen.png" alt="Qwen">' : m.id.startsWith("zipformer") ? '<img class="model-logo sherpa-logo" src="/assets/brands/sherpa.png" alt="k2 / sherpa-onnx">' : '<span class="model-monogram" aria-hidden="true">S</span>'}<div><span class="eyebrow">${{ asr: t(lang, "语音转写", "語音轉寫", "TRANSCRIPTION"), llm: t(lang, "纪要生成", "紀要生成", "MINUTES"), diarization: t(lang, "说话人区分", "說話人區分", "SPEAKERS") }[m.kind]}</span><h3>${e(m.name)}</h3></div></div><dl class="specs"><div><dt>${t(lang, "文件大小", "檔案大小", "Model files")}</dt><dd>${size(m.bytes)}</dd></div><div><dt>${t(lang, "运行内存估算", "執行記憶體估算", "Estimated runtime memory")}</dt><dd>${(m.memory_mib / 1024).toFixed(1)} GiB</dd></div><div><dt>${t(lang, "格式", "格式", "Format")}</dt><dd>${e(m.format.toUpperCase())}</dd></div><div><dt>${t(lang, "许可", "授權", "License")}</dt><dd>${e(m.license)}</dd></div></dl><div class="card-bottom"><span class="small muted">${m.bundled ? t(lang, "完整安装包默认包含", "完整安裝包預設包含", "Included in full installers") : t(lang, "可选下载", "選用下載", "Optional download")}</span>${external(m.source, t(lang, "模型来源", "模型來源", "Source"))}</div></article>`,
    )
    .join("");
}
function performance(lang) {
  if (!benchmarks.length)
    return `<div class="empty"><h3>${t(lang, "暂无可发布的实测数据", "尚無可發布的實測資料", "No published measurements yet")}</h3><p>${t(lang, "将按具体设备与测试条件展示 tokens/s、首 token 延迟和内存。模型大小或芯片名称不能代替实测。", "將依具體裝置及測試條件顯示 tokens/s、首 token 延遲與記憶體。模型大小或晶片名稱不能取代實測。", "Results will report tokens/s, first-token latency and memory for a specified device and test. Model size or chip branding is not a measurement.")}</p></div>`;
  return benchmarks
    .map(
      (b) =>
        `<article class="benchmark"><div class="section-top"><div><span class="badge">${t(lang, "本机实测", "本機實測", "Measured on device")}</span><h3>${e(b.hardware.chip)} · ${e(b.hardware.memory_gib)} GB</h3><p class="small muted">${e(b.hardware.os)} · ${e(b.hardware.arch)} · ${e(b.measured_at.slice(0, 10))}</p></div><a href="/data/benchmarks/${e(b.id)}.json" download>${t(lang, "下载完整测量记录", "下載完整測量紀錄", "Download full measurements")} ↓</a></div><div class="table-scroll"><table><thead><tr><th>${t(lang, "后端 / 模型", "後端 / 模型", "Backend / model")}</th><th>${t(lang, "生成速度", "生成速度", "Generation")}<small>tokens/s</small></th><th>${t(lang, "首 token", "首 token", "First token")}<small>ms</small></th><th>${t(lang, "进程内存峰值", "程序記憶體峰值", "Peak process RSS")}<small>MiB</small></th><th>${t(lang, "测量次数", "測量次數", "Runs")}</th></tr></thead><tbody>${b.summary.map((s) => `<tr><td><strong>${e(s.backend.toUpperCase())}</strong><small>${e(s.model_id)}</small></td><td class="metric">${s.median_generation_tps.toFixed(1)}</td><td>${s.median_ttft_ms.toFixed(0)}</td><td>${s.max_peak_rss_mib == null ? "—" : s.max_peak_rss_mib.toFixed(0)}</td><td>${b.runs.filter((r) => r.backend === s.backend && r.model_id === s.model_id).length}</td></tr>`).join("")}</tbody></table></div><p class="small">${t(lang, "固定测试输入；速度和延迟为多次测量的中位数。模型热启动，提示冷缓存。RSS 是采样到的进程内存，不代表 GPU 总内存或整机功耗。", "固定測試輸入；速度與延遲為多次測量中位數。模型熱啟動，提示冷快取。RSS 是取樣程序記憶體，不代表 GPU 總記憶體或整機耗電。", "Fixed test input; speed and latency are medians across runs. Model warm, prompt cache cold. RSS is sampled process memory, not total GPU memory or system power.")}</p><wa-details summary="${t(lang, "测试条件与限制", "測試條件與限制", "Method and limitations")}"><pre>${e(JSON.stringify({ method: b.method, parameters: b.runs.map((r) => ({ backend: r.backend, model_id: r.model_id, context_size: r.context_size, threads: r.threads, batch_size: r.batch_size, ubatch_size: r.ubatch_size, generated_tokens: r.generated_tokens, quantization: r.quantization })).filter((r, i, a) => a.findIndex((q) => JSON.stringify(q) === JSON.stringify(r)) === i), limitations: b.limitations }, null, 2))}</pre></wa-details></article>`,
    )
    .join("");
}
function models(lang) {
  return `${heading(lang, "MODELS / PERFORMANCE", copy.titles[lang][2], t(lang, "按你的设备和使用场景选模型。文件大小、运行内存估算与实测速度分开显示。", "依裝置及使用情境選模型。檔案大小、執行記憶體估算與實測速度分開顯示。", "Choose models for your computer and workload. File size, estimated memory and measured speed are shown separately."))}${note(lang)}<div class="section-tabs"><a href="#hardware">${t(lang, "设备组合", "裝置組合", "Configurations")}</a><a href="#measurements">${t(lang, "实测性能", "實測效能", "Measurements")}</a><a href="#catalog">${t(lang, "模型目录", "模型目錄", "Model catalog")}</a></div><section id="hardware"><div class="section-top"><h2>${t(lang, "从一组合适的配置开始", "從一組合適的設定開始", "Start with a suitable configuration")}</h2><a href="${pathFor(lang, "community")}">${t(lang, "查看与使用配置", "查看及使用設定", "Preview and use configurations")} →</a></div><div class="table-scroll"><table><thead><tr><th>${t(lang, "设备", "裝置", "Computer")}</th><th>${t(lang, "转写 + 写作", "轉寫 + 寫作", "ASR + writer")}</th><th>${t(lang, "资源策略", "資源策略", "Resource mode")}</th><th>${t(lang, "性质", "性質", "Evidence")}</th></tr></thead><tbody>${profiles
    .filter((p) => p.origin === "official")
    .map(
      (p) =>
        `<tr><td><a href="${pathFor(lang, "community")}#${e(p.id)}">${e(p.title[lang])}</a></td><td><small>${e(p.profile.preferences.asr_model)}</small>${e(p.profile.preferences.llm_model)}</td><td>${e(p.profile.preferences.resource_mode)} · ${e(p.profile.preferences.context_size)} tokens</td><td>${p.benchmark_ids.length ? t(lang, "有测量记录", "有測量紀錄", "Measurement linked") : t(lang, "建议起点 · 未测", "建議起點 · 未測", "Starting point · unmeasured")}</td></tr>`,
    )
    .join(
      "",
    )}</tbody></table></div><p class="small muted">${t(lang, "先看当前可用内存，再看总内存。会议期间优先保证录音与转写；更大模型适合会后主动整理。", "先看目前可用記憶體，再看總記憶體。開會時優先確保錄音與轉寫；較大模型適合會後主動整理。", "Check available memory as well as installed memory. Prioritize recording and transcription during meetings; larger writers are better suited to after-meeting work.")}</p></section><section class="backend-grid"><article><img src="/assets/brands/mlx.svg" alt="MLX" class="backend-logo"><h3>Apple Silicon</h3><p>${t(lang, "Metal 可加速现有 GGUF 模型；MLX 使用独立的 MLX 权重和运行时。选择后端不会把模型推理变成云服务。", "Metal 可加速現有 GGUF 模型；MLX 使用獨立的 MLX 權重與執行環境。選擇後端不會把模型推理變成雲端服務。", "Metal accelerates existing GGUF models. MLX uses separate MLX weights and runtime. Neither backend sends inference to a cloud service.")}</p><p class="small muted">${t(lang, "MLX 为实验功能，需另行安装兼容运行时；仅支持 Apple Silicon，不能直接加载 GGUF。", "MLX 為實驗功能，需另行安裝相容執行環境；僅支援 Apple Silicon，不能直接載入 GGUF。", "MLX is experimental and needs a separately installed compatible runtime on Apple Silicon; it does not directly load GGUF.")}</p></article><article><span class="backend-word">CPU</span><h3>Windows · Linux · Intel Mac</h3><p>${t(lang, "保留通用 CPU 后端。省资源、日常、质量三档控制线程、上下文和任务预算；预算是调度依据，不是操作系统硬内存上限。", "保留通用 CPU 後端。省資源、日常、品質三檔控制執行緒、上下文及任務預算；預算是排程依據，並非系統硬記憶體上限。", "The CPU backend remains available. Economy, balanced and quality modes control threads, context and job budgets. A budget is a scheduling check, not an OS-enforced memory cap.")}</p></article></section><section id="measurements"><div class="section-top"><h2>${t(lang, "实测性能", "實測效能", "Measured performance")}</h2><span class="small muted">${t(lang, "不以估算代替测量", "不以估算代替測量", "Measurements, not estimates")}</span></div>${performance(lang)}</section><section id="catalog"><div class="section-top"><h2>${t(lang, "可用模型", "可用模型", "Available models")}</h2><span class="small muted">${t(lang, "参数自动同步自应用模型目录", "參數自動同步自應用模型目錄", "Generated from the app model catalog")}</span></div><div class="models-grid">${modelCards(lang)}</div><p class="small muted">${t(lang, "下载前检查文件大小与 SHA-256。首次下载联网，之后本地推理无需网络。运行内存是估算，还需给其他应用与系统留空间。", "下載前檢查檔案大小與 SHA-256。首次下載連網，之後本機推理不需網路。執行記憶體為估算，仍須預留其他應用與系統空間。", "Downloads are checked against pinned sizes and SHA-256. Downloading is online; subsequent inference is offline. Memory figures are estimates and exclude headroom for other apps and the OS.")}</p></section>`;
}
function profileCard(p, lang) {
  const v = p.profile.preferences;
  return `<article class="profile-card" id="${e(p.id)}" data-platform="${e(p.platform)}" data-memory="${p.memory_gib}" data-origin="${p.origin}"><div class="card-top"><span class="badge ${p.origin === "community" ? "community-badge" : ""}">${p.origin === "official" ? t(lang, "官方建议", "官方建議", "Maintainer suggestion") : t(lang, "社区分享", "社群分享", "Community")}</span><span class="small muted">${e(p.author)}</span></div><h2>${e(p.title[lang])}</h2><p>${e(p.description[lang])}</p><div class="model-pair"><span>ASR <strong>${e(v.asr_model || "zipformer-small")}</strong></span><span>LLM <strong>${e(v.llm_model || "qwen3-1.7b")}</strong></span></div><dl class="profile-specs"><div><dt>${t(lang, "后端", "後端", "Backend")}</dt><dd>${e(v.inference_backend || "auto")}</dd></div><div><dt>${t(lang, "线程", "執行緒", "Threads")}</dt><dd>${v.threads || 1}</dd></div><div><dt>${t(lang, "上下文", "上下文", "Context")}</dt><dd>${v.context_size || 2048}</dd></div><div><dt>${t(lang, "内存预算", "記憶體預算", "Memory budget")}</dt><dd>${v.memory_budget_mib ? `${v.memory_budget_mib} MiB` : "auto"}</dd></div></dl><p class="small muted">${p.benchmark_ids.length ? t(lang, "附实测记录", "附實測紀錄", "Measurement included") : t(lang, "性能未测量；建议组合不等于性能承诺。", "效能未測量；建議組合不代表效能承諾。", "Unmeasured; a suggested combination is not a performance guarantee.")}</p><div class="actions"><wa-button appearance="outlined" class="preview-profile" data-id="${e(p.id)}">${t(lang, "预览配置", "預覽設定", "Preview")}</wa-button><a href="/data/profiles/${e(p.id)}.json" download>${t(lang, "下载 JSON", "下載 JSON", "Download JSON")} ↓</a></div></article>`;
}
function community(lang) {
  return `${heading(lang, "CONFIGURATIONS", copy.titles[lang][3], t(lang, "按设备浏览组合，查看参数后再使用。分享的是配置文件，不是会议数据。", "依裝置瀏覽組合，查看參數後再使用。分享的是設定檔，並非會議資料。", "Browse configurations by computer and inspect settings before using them. Share configuration files, not meeting data."))}${note(lang)}<div class="filters"><wa-input id="profile-search" label="${t(lang, "搜索配置", "搜尋設定", "Search configurations")}" placeholder="${t(lang, "名称或模型", "名稱或模型", "Name or model")}" with-clear></wa-input><wa-select label="${t(lang, "设备", "裝置", "Computer")}" id="platform-filter" value="all"><wa-option value="all">${t(lang, "全部", "全部", "All")}</wa-option><wa-option value="apple-silicon">Apple Silicon</wa-option><wa-option value="mac-intel">Intel Mac</wa-option><wa-option value="windows">Windows</wa-option><wa-option value="linux">Linux</wa-option></wa-select><wa-select label="${t(lang, "你的内存", "你的記憶體", "Your memory")}" id="memory-filter" value="all"><wa-option value="all">${t(lang, "不限", "不限", "Any")}</wa-option>${[8, 16, 24, 32, 64].map((n) => `<wa-option value="${n}">${n} GB</wa-option>`).join("")}</wa-select><wa-select label="${t(lang, "来源", "來源", "Source")}" id="origin-filter" value="all"><wa-option value="all">${t(lang, "全部", "全部", "All")}</wa-option><wa-option value="official">${t(lang, "官方建议", "官方建議", "Maintainer")}</wa-option><wa-option value="community">${t(lang, "社区分享", "社群分享", "Community")}</wa-option></wa-select><a href="#share">${t(lang, "分享我的配置", "分享我的設定", "Share a configuration")} ↗</a></div><div class="profile-grid">${profiles.map((p) => profileCard(p, lang)).join("")}</div><p id="no-results" hidden class="empty">${t(lang, "暂时没有符合条件的配置。可以调整筛选或提交自己的测试结果。", "暫無符合條件的設定。可調整篩選或提交自己的測試結果。", "No matching configurations yet. Adjust the filters or contribute your own results.")}</p><section id="share" class="share-section"><div><p class="eyebrow">${t(lang, "文件投稿 · 审查后发布", "檔案投稿 · 審查後發布", "FILE SUBMISSION · REVIEWED BEFORE PUBLICATION")}</p><h2>${t(lang, "分享你用得好的组合", "分享你用得好的組合", "Share a configuration that works for you")}</h2><ol><li>${t(lang, "在 OmaScribe 设置中导出配置，再在这里选择文件。文件只在浏览器本地读取。", "在 OmaScribe 設定中匯出設定，再於此選取檔案。檔案只在瀏覽器本機讀取。", "Export a configuration from OmaScribe settings and select it here. The file is read locally in your browser.")}</li><li>${t(lang, "核对公开名称、设备与参数，下载投稿文件。不会自动上传。", "核對公開名稱、裝置及參數，下載投稿檔案。不會自動上傳。", "Review the public name, hardware and settings, then download the submission file. Nothing is uploaded automatically.")}</li><li>${t(lang, "前往 GitHub 上传文件，选择新分支并提交 PR；也可自行附到 Issue。维护者审查后发布到广场。", "前往 GitHub 上傳檔案，選擇新分支並提交 PR；亦可自行附於 Issue。維護者審查後發布至廣場。", "Upload the file on GitHub, choose a new branch and submit a pull request, or attach it to an issue yourself. Maintainers review it before publication.")}</li></ol><p class="small muted">${t(lang, "不接收录音、转写、笔记、密钥或私有路径。公开字段请使用你愿意分享的名称。", "不接收錄音、轉寫、筆記、金鑰或私人路徑。公開欄位請使用你願意分享的名稱。", "Do not include recordings, transcripts, notes, keys or private paths. Use only names you intend to make public.")}</p></div><form id="share-form"><label>${t(lang, "选择配置 JSON（最多 64 KiB）", "選取設定 JSON（最多 64 KiB）", "Choose a configuration JSON (max 64 KiB)")}<input type="file" id="profile-file" accept=".json,application/json" required></label><wa-input label="${t(lang, "公开作者名称", "公開作者名稱", "Public author name")}" id="share-author" maxlength="60" autocomplete="off" required></wa-input><div class="form-row"><wa-select label="${t(lang, "设备类型", "裝置類型", "Computer type")}" id="share-platform" value="apple-silicon"><wa-option value="apple-silicon">Apple Silicon</wa-option><wa-option value="mac-intel">Intel Mac</wa-option><wa-option value="windows">Windows</wa-option><wa-option value="linux">Linux</wa-option><wa-option value="cross-platform">${t(lang, "跨平台", "跨平台", "Cross-platform")}</wa-option></wa-select><wa-input label="${t(lang, "内存 GB", "記憶體 GB", "Memory GB")}" id="share-memory" type="number" min="4" max="512" value="16" required></wa-input></div><wa-textarea label="${t(lang, "简短说明（公开）", "簡短說明（公開）", "Short description (public)")}" id="share-description" maxlength="300" rows="3" required></wa-textarea><div id="file-preview" class="file-preview" hidden><strong id="file-name"></strong><pre id="file-json"></pre></div><p id="share-status" role="status" aria-live="polite"></p><wa-button type="submit" id="prepare-submission" disabled>${t(lang, "下载投稿文件", "下載投稿檔案", "Download submission file")} ↓</wa-button><div class="submission-next" id="submission-next" hidden><p>${t(lang, "文件已准备好，尚未投稿或发布。下一步会打开 GitHub，你可以在那里上传并提交。", "檔案已備妥，尚未投稿或發布。下一步會開啟 GitHub，可在該處上傳及提交。", "Your file is ready. It has not been submitted or published. The next step opens GitHub, where you can upload and submit it.")}</p>${external("https://github.com/EazyLee30/omascribe/upload/main/community/submissions", t(lang, "去 GitHub 提交 PR", "到 GitHub 提交 PR", "Continue on GitHub"))}</div></form></section><wa-dialog id="profile-dialog"><span slot="label" id="preview-title"></span><p>${t(lang, "仅包含设置，不含会议内容。打开应用后还需预览并确认，缺少的模型不会自动下载。", "只含設定，不含會議內容。開啟應用後仍須預覽及確認，缺少的模型不會自動下載。", "Contains settings only, no meeting content. The app will ask you to review and confirm. Missing models are not downloaded automatically.")}</p><pre id="preview-json"></pre><p id="preview-status" role="status"></p><div class="actions"><a class="button" id="open-app">${t(lang, "在 OmaScribe 中预览", "在 OmaScribe 中預覽", "Preview in OmaScribe")} ↗</a><a id="download-profile" download>${t(lang, "下载 JSON 文件", "下載 JSON 檔案", "Download JSON file")} ↓</a></div><p class="small muted">${t(lang, "需要支持配置链接的新版应用。若无法打开，请下载文件后在应用设置中导入。", "需要支援設定連結的新版應用。若無法開啟，請下載檔案後在應用設定中匯入。", "Requires a build supporting configuration links. If the app does not open, download the file and import it in settings.")}</p></wa-dialog>`;
}
function privacy(lang) {
  const rows = [
    [
      t(
        lang,
        "录音、音频导入、转写",
        "錄音、音訊匯入、轉寫",
        "Recording, audio import, transcription",
      ),
      t(lang, "离线", "離線", "Offline"),
      t(
        lang,
        "音频在本机读取和处理，不上传。",
        "音訊在本機讀取及處理，不上傳。",
        "Audio is read and processed locally, without upload.",
      ),
    ],
    [
      t(
        lang,
        "AI 纪要、说话人分析",
        "AI 紀要、說話人分析",
        "AI minutes, speaker analysis",
      ),
      t(lang, "离线", "離線", "Offline"),
      t(
        lang,
        "使用本地模型；原文、提示和结果不发往云端。",
        "使用本機模型；原文、提示及結果不傳送雲端。",
        "Local models process source text, prompts and results. None are sent to a cloud model.",
      ),
    ],
    [
      t(
        lang,
        "笔记、知识库、记忆、导出",
        "筆記、知識庫、記憶、匯出",
        "Notes, knowledge, memory, export",
      ),
      t(lang, "离线", "離線", "Offline"),
      t(
        lang,
        "数据保存在电脑上，由你选择文件去向。",
        "資料保存在電腦，由你選擇檔案去向。",
        "Data stays on your computer. You choose where exported files go.",
      ),
    ],
    [
      t(
        lang,
        "浏览官网与配置广场",
        "瀏覽官網與設定廣場",
        "Website and configuration gallery",
      ),
      t(lang, "联网", "連網", "Online"),
      t(
        lang,
        "浏览器访问 Cloudflare 托管网页，会发送常规网络请求信息；不读取应用中的会议。",
        "瀏覽器存取 Cloudflare 託管網頁，會傳送一般網路請求資訊；不讀取應用中的會議。",
        "The browser requests pages hosted by Cloudflare and sends normal request metadata. It cannot read meetings in the app.",
      ),
    ],
    [
      t(
        lang,
        "设备检测与内置推荐",
        "裝置偵測與內建建議",
        "Device checks and built-in recommendations",
      ),
      t(lang, "离线", "離線", "Offline"),
      t(
        lang,
        "应用在本机检测硬件并匹配内置模型组合，不上传设备信息。在线广场另需主动联网。",
        "應用在本機偵測硬體並比對內建模型組合，不上傳裝置資訊。線上廣場另需主動連網。",
        "The app checks hardware and matches bundled model suggestions locally without uploading device details. The online gallery is a separate opt-in feature.",
      ),
    ],
    [
      t(
        lang,
        "下载模型与安装包",
        "下載模型與安裝包",
        "Model and installer downloads",
      ),
      t(lang, "主动联网", "主動連網", "Online by request"),
      t(
        lang,
        "连接 GitHub 或 Hugging Face 等目录中列明的来源。下载模型后可断网使用。",
        "連接 GitHub 或 Hugging Face 等目錄列明來源。下載模型後可離線使用。",
        "Downloads contact the listed sources, including GitHub and Hugging Face. Models run offline after download.",
      ),
    ],
    [
      t(
        lang,
        "导出 / 导入配置文件",
        "匯出 / 匯入設定檔",
        "Exporting / importing configuration files",
      ),
      t(lang, "本地", "本機", "Local"),
      t(
        lang,
        "文件包含模型 ID、资源与外观设置，不包含会议数据、网络授权或模型权重。",
        "檔案包含模型 ID、資源及外觀設定，不含會議資料、網路授權或模型權重。",
        "Files contain model IDs, resource and appearance settings. They exclude meeting data, network permissions and weights.",
      ),
    ],
    [
      t(lang, "向广场投稿", "向廣場投稿", "Contributing to the gallery"),
      t(lang, "自行提交", "自行提交", "You submit"),
      t(
        lang,
        "本网页只本地校验与生成投稿文件。你前往 GitHub 上传后，内容会成为公开投稿。",
        "本網頁只在本機驗證及產生投稿檔案。你到 GitHub 上傳後，內容才成為公開投稿。",
        "This page validates and prepares the file locally. Only your upload on GitHub makes it a public submission.",
      ),
    ],
  ];
  return `${heading(lang, "PRIVACY / NETWORK", copy.titles[lang][4], t(lang, "本地处理与在线服务分开说明。你可以不使用配置广场，继续离线录音、转写和整理。", "本機處理與線上服務分開說明。你可不使用設定廣場，繼續離線錄音、轉寫及整理。", "Local processing and online services have separate boundaries. You can skip the gallery and continue recording, transcribing and writing offline."))}<div class="table-scroll privacy-table"><table><thead><tr><th>${t(lang, "功能", "功能", "Feature")}</th><th>${t(lang, "网络", "網路", "Network")}</th><th>${t(lang, "处理的数据", "處理的資料", "Data boundary")}</th></tr></thead><tbody>${rows.map((r, i) => `<tr><td><strong>${r[0]}</strong></td><td><span class="badge ${[3, 5, 7].includes(i) ? "online" : ""}">${r[1]}</span></td><td>${r[2]}</td></tr>`).join("")}</tbody></table></div><div class="two-columns privacy-notes"><section><h2>${t(lang, "配置不能替你授权", "設定不能代你授權", "A configuration cannot grant permission")}</h2><p>${t(lang, "从网页打开配置只进入预览。应用验证支持的模型 ID、参数范围与文件大小，拒绝命令、自定义下载地址、路径和未知字段。配置不会开始录音、上传内容或自动下载。", "從網頁開啟設定只進入預覽。應用驗證支援的模型 ID、參數範圍及檔案大小，拒絕命令、自訂下載位址、路徑與未知欄位。設定不會開始錄音、上傳內容或自動下載。", "Opening a configuration from the site only opens a preview. The app validates model IDs, parameter ranges and file size, rejecting commands, custom download URLs, paths and unknown fields. It cannot start recording, upload content or trigger automatic downloads.")}</p></section><section><h2>${t(lang, "本地不等于加密", "本機不等於加密", "Local storage is not encryption")}</h2><p>${t(lang, "OmaScribe 当前不对本地录音和笔记进行应用层加密。请使用系统账户保护与磁盘加密。导出文件由你自行保管。", "OmaScribe 目前不對本機錄音及筆記進行應用層加密。請使用系統帳號保護與磁碟加密。匯出檔案由你自行保管。", "OmaScribe does not currently encrypt local recordings or notes at the application level. Use OS account protection and disk encryption. You are responsible for exported files.")}</p></section><section><h2>${t(lang, "网站访问", "網站存取", "Website requests")}</h2><p>${t(lang, "网站不加载访客分析、外部字体或第三方运行脚本。模型标识和页面资源由本域名提供。托管与下载服务仍会处理提供服务必需的 IP、请求时间等网络元数据。", "網站不載入訪客分析、外部字型或第三方執行腳本。模型標識與頁面資源由本網域提供。託管及下載服務仍會處理提供服務所需的 IP、請求時間等網路中繼資料。", "The site has no visitor analytics, external fonts or third-party runtime scripts. Logos and page assets are served from this domain. Hosting and download services still process network metadata such as IP addresses and request times.")}</p></section><section><h2>${t(lang, "版本与源码", "版本與原始碼", "Versions and source")}</h2><p>${t(lang, "官网可随 GitHub 代码与发布信息更新。桌面应用不会因网页更新自动替换程序或模型。下载页明确标注实际安装包版本。", "官網可隨 GitHub 程式碼及發布資訊更新。桌面應用不會因網頁更新自動替換程式或模型。下載頁清楚標明實際安裝包版本。", "The site can update with GitHub code and release metadata. A website update does not automatically replace the desktop app or its models. Downloads show the actual installer version.")}</p>${external("https://github.com/EazyLee30/omascribe/blob/main/SECURITY.md", t(lang, "阅读安全说明", "閱讀安全說明", "Read the security documentation"))}</section></div>`;
}
// Pin the library and materialize its bundled system icons so even fetch(data:) is unnecessary.
// Nothing in the live site depends on a font or icon CDN.
const chunks = resolve(root, "node_modules/@awesome.me/webawesome/dist/chunks");
let iconModule;
for (const file of await readdir(chunks)) {
  if (
    file.endsWith(".js") &&
    (await readFile(join(chunks, file), "utf8")).includes(
      "// src/components/icon/library.system.ts",
    )
  ) {
    iconModule = await import(pathToFileURL(join(chunks, file)).href);
    break;
  }
}
if (!iconModule?.icons)
  throw Error(
    "Web Awesome system icon source changed; review the pinned package",
  );
for (const [variant, icons] of Object.entries(iconModule.icons)) {
  await mkdir(resolve(out, "assets/ui-icons", variant), { recursive: true });
  for (const [name, svg] of Object.entries(icons))
    await writeFile(
      resolve(out, "assets/ui-icons", variant, `${name}.svg`),
      svg,
    );
}
await build({
  entryPoints: {
    site: resolve(root, "src/client.js"),
    components: resolve(root, "src/components.css"),
  },
  bundle: true,
  minify: true,
  format: "esm",
  target: ["es2022"],
  outdir: out,
  loader: { ".woff2": "file", ".woff": "file" },
  assetNames: "assets/fonts/[name]-[hash]",
  logLevel: "warning",
});
const renderers = { "": home, download, models, community, privacy };
for (const lang of LANGS)
  for (const page of copy.pages) {
    const i = copy.pages.indexOf(page),
      title = copy.titles[lang][i],
      route = pathFor(lang, page);
    const canonical = `https://omascribe.eazylee.si${route}`;
    const html = `<!doctype html><html lang="${{ zh: "zh-CN", tw: "zh-TW", en: "en" }[lang]}" data-language="${lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="theme-color" content="#ffffff"><title>${e(title)} — OmaScribe</title><meta name="description" content="${e(title)}. ${t(lang, "开源本地会议笔记，模型配置与实测性能。", "開源本機會議筆記，模型設定與實測效能。", "Open-source local meeting notes, model configurations and measured performance.")}"><link rel="canonical" href="${canonical}">${LANGS.map((l) => `<link rel="alternate" hreflang="${{ zh: "zh-CN", tw: "zh-TW", en: "en" }[l]}" href="https://omascribe.eazylee.si${pathFor(l, page)}">`).join("")}<meta property="og:type" content="website"><meta property="og:title" content="${e(title)} — OmaScribe"><meta property="og:url" content="${canonical}"><meta property="og:image" content="https://omascribe.eazylee.si/assets/social.png"><link rel="icon" href="/assets/art/edit-notes.svg" type="image/svg+xml"><link rel="stylesheet" href="/components.css"><link rel="stylesheet" href="/site.css"><script type="module" src="/site.js"></script></head><body><a class="skip-link" href="#main">${t(lang, "跳到正文", "跳至正文", "Skip to content")}</a><div class="site-shell">${header(lang, page)}<main id="main" class="page-${page || "home"}">${renderers[page](lang)}</main>${footer(lang)}</div></body></html>`;
    const dir = resolve(out, route.slice(1));
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, "index.html"), html);
  }
await writeFile(
  resolve(out, "sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${LANGS.flatMap((l) => copy.pages.map((p) => `<url><loc>https://omascribe.eazylee.si${pathFor(l, p)}</loc></url>`)).join("")}</urlset>`,
);
await writeFile(
  resolve(out, "robots.txt"),
  "User-agent: *\nAllow: /\nSitemap: https://omascribe.eazylee.si/sitemap.xml\n",
);
await writeFile(
  resolve(out, "404.html"),
  '<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Page not found — OmaScribe</title><link rel="stylesheet" href="/components.css"><link rel="stylesheet" href="/site.css"><main class="site-shell page-heading"><h1>404</h1><p>This page does not exist. / 此页面不存在。</p><a class="button" href="/">OmaScribe →</a></main></html>',
);
console.log(
  `Built 15 pages, ${profiles.length} reviewed configurations, ${publicCatalog.models.length} models, ${benchmarks.length} measured benchmark datasets; release ${releases.tag}`,
);
