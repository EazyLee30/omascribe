// The illustration pack is visual direction, not a source of release or model claims.
export function createDesign({
  e,
  t,
  pathFor,
  copy,
  LANGS,
  releases,
  size,
  external,
}) {
  const art = (name, cls = "", eager = false) =>
    `<img class="${cls}" src="/assets/art/${name}.webp" alt="" ${eager ? 'fetchpriority="high" srcset="/assets/art/01-hero-mecha-small.webp 600w, /assets/art/01-hero-mecha.webp 1145w" sizes="(max-width: 600px) calc(100vw - 40px), (max-width: 1440px) 62vw, 820px"' : 'loading="lazy"'} decoding="async">`;
  const icon = (name) =>
    `<img src="/assets/art/${name}.svg" width="40" height="40" alt="">`;
  const platformIcon = (name) => `<img class="platform-icon" src="/assets/platforms/${name}.svg" width="24" height="24" alt="" aria-hidden="true">`;
  const platformRow = (lang) => `<div class="platform-links" aria-label="${t(lang, "支持的平台", "支援的平台", "Platforms")}">${[["apple","macOS"],["windows","Windows"],["linux","Linux"]].map(([name,label]) => `<a href="${pathFor(lang,"download")}#${name === "apple" ? "mac" : name}">${platformIcon(name)}<span>${label}</span></a>`).join("")}</div>`;
  const link = (href, label, secondary = false) =>
    `<wa-button href="${e(href)}" variant="neutral" appearance="${secondary ? "outlined" : "filled"}">${e(label)} <span slot="end" aria-hidden="true">→</span></wa-button>`;
  const button = (lang, page, label, secondary = false) =>
    link(pathFor(lang, page), label, secondary);
  const nav = (lang, page) =>
    copy.pages
      .map(
        (p, i) =>
          `<a href="${pathFor(lang, p)}" ${p === page ? 'aria-current="page"' : ""}>${copy.nav[lang][i]}</a>`,
      )
      .join("");
  function header(lang, page) {
    return `<header class="site-header"><a class="brand" href="${pathFor(lang)}">OmaScribe</a><span class="brand-caption">LOCAL / OPEN SOURCE</span><nav aria-label="${t(lang, "主导航", "主導覽", "Main navigation")}">${nav(lang, page)}</nav><div class="header-tools"><wa-select id="language" size="s" value="${pathFor(lang, page)}" label="Language / 语言 / 語言">${LANGS.map((l) => `<wa-option value="${pathFor(l, page)}">${{ zh: "简体中文", tw: "繁體中文", en: "English" }[l]}</wa-option>`).join("")}</wa-select><wa-button id="menu-toggle" class="menu-toggle" size="s" appearance="outlined" aria-label="${t(lang, "打开导航", "開啟導覽", "Open navigation")}"><wa-icon library="system" name="bars"></wa-icon><span class="sr-only">${t(lang, "打开导航", "開啟導覽", "Open navigation")}</span></wa-button></div></header><wa-drawer id="mobile-nav" label="${t(lang, "导航", "導覽", "Navigation")}" placement="end"><nav aria-label="${t(lang, "移动导航", "行動導覽", "Mobile navigation")}">${nav(lang, page)}</nav>${external("https://github.com/EazyLee30/omascribe", "GitHub")}</wa-drawer>`;
  }
  function footer(lang) {
    return `<footer class="site-footer"><a class="brand" href="${pathFor(lang)}">OmaScribe</a><p>${t(lang, "本地录音。可编辑的笔记。", "本機錄音。可編輯的筆記。", "Local conversations. Editable notes.")}<br>© 2026 · MIT</p><div>${external("https://github.com/EazyLee30/omascribe", "GitHub")}<a href="${pathFor(lang, "privacy")}">${t(lang, "联网与隐私", "連網與隱私", "Network & privacy")}</a><a href="/assets/NOTICE.md">${t(lang, "素材与组件", "素材與元件", "Asset & component credits")}</a><a href="#main">${t(lang, "返回顶部", "返回頂部", "Back to top")} ↑</a></div></footer>`;
  }
  function demo(lang) {
    const rows = [
      [
        "00:08",
        t(
          lang,
          "下周的项目课先让学生拍一张观察照片，再讨论各组的发现。",
          "下週的專題課先讓學生拍一張觀察照片，再討論各組的發現。",
          "For next week’s project, students can take an observation photo, then discuss what each group found.",
        ),
      ],
      [
        "00:21",
        "Let’s keep the first activity short, then leave time for discussion.",
      ],
      [
        "00:34",
        t(
          lang,
          "我会准备学习单，评价标准我们下次一起确认。",
          "我會準備學習單，評量標準我們下次一起確認。",
          "I’ll prepare the worksheet. We can agree on the assessment criteria next time.",
        ),
      ],
      [
        "00:49",
        t(
          lang,
          "先记录每组的观察依据，不急着给结论。",
          "先記錄每組的觀察依據，不急著下結論。",
          "Record each group’s observations before jumping to conclusions.",
        ),
      ],
    ];
    const notes = t(
      lang,
      "讨论摘要\n用观察照片引入项目课，留足小组讨论时间，记录各组的观察依据。\n\n行动项\n- 准备学习单（00:34）。\n- 下次讨论确认评价标准（00:34）。\n\n待确认\n学习单负责人、首个活动时长、评价标准的具体内容。",
      "討論摘要\n用觀察照片引入專題課，留足小組討論時間，記錄各組的觀察依據。\n\n行動項\n- 準備學習單（00:34）。\n- 下次討論確認評量標準（00:34）。\n\n待確認\n學習單負責人、首個活動時長、評量標準的具體內容。",
      "Summary\nStart with observation photos, leave time for group discussion, and record each group’s evidence.\n\nActions\n- Prepare the worksheet (00:34).\n- Agree on assessment criteria next time (00:34).\n\nTo confirm\nWorksheet owner, first activity duration, and the assessment criteria.",
    );
    return `<section id="workspace" class="demo-window" aria-label="${t(lang, "工作区示例", "工作區範例", "Workspace example")}"><div class="demo-title"><strong>OmaScribe</strong><span>DEMO / 00:49</span><span aria-hidden="true">○ &nbsp; □</span></div><div class="demo-meta">${icon("record-waveform")}<span>${t(lang, "项目课备课讨论", "專題課備課討論", "Project lesson planning")}</span><span class="sample-label">${t(lang, "编写的示例 · 无录音", "編寫的範例 · 無錄音", "Prepared sample · no recording")}</span></div><wa-tab-group id="demo-tabs" active="transcript"><wa-tab panel="transcript" active>${t(lang, "转写原文", "轉寫原文", "Transcript")}</wa-tab><wa-tab panel="cues">${t(lang, "我的关键词", "我的關鍵詞", "My cues")}</wa-tab><wa-tab panel="notes">${t(lang, "可编辑纪要", "可編輯紀要", "Notes draft")}</wa-tab><wa-tab-panel name="transcript" active><div class="transcript-rows">${rows.map(([time, text]) => `<div class="transcript-row" id="demo-${time.replace(":", "")}" tabindex="-1"><time>${time}</time><p>${e(text)}</p></div>`).join("")}</div></wa-tab-panel><wa-tab-panel name="cues"><div class="cue-list">${t(
      lang,
      "照片导入|留足讨论时间|学习单|评价标准待确认|观察依据",
      "照片導入|留足討論時間|學習單|評量標準待確認|觀察依據",
      "Observation photos|Time for discussion|Worksheet|Assessment criteria to confirm|Evidence",
    )
      .split("|")
      .map((c) => `<span>${icon("add-context-tag")}${e(c)}</span>`)
      .join(
        "",
      )}</div><p class="small demo-hint">${t(lang, "会议进行时即可记下重点，作为整理纪要的线索。", "會議進行時即可記下重點，作為整理紀要的線索。", "Add cues during a conversation to guide the notes.")}</p></wa-tab-panel><wa-tab-panel name="notes"><wa-textarea id="demo-notes" label="${t(lang, "编辑示例纪要（仅当前页面）", "編輯範例紀要（僅目前頁面）", "Edit demo notes (this page only)")}" value="${e(notes)}" rows="7" resize="vertical"></wa-textarea><div class="demo-actions"><wa-button size="s" appearance="outlined" id="demo-source">${t(lang, "查看来源", "查看來源", "View source")} 00:34</wa-button><wa-button size="s" id="demo-export">${t(lang, "导出 Markdown", "匯出 Markdown", "Export Markdown")} ↓</wa-button><wa-button size="s" appearance="plain" id="demo-reset">${t(lang, "重置", "重設", "Reset")}</wa-button></div><p id="demo-status" role="status" class="small"></p></wa-tab-panel></wa-tab-group><div class="demo-bottom"><span>● ${t(lang, "仅在当前浏览器演示", "僅於目前瀏覽器示範", "Demo runs in this browser")}</span><a href="${pathFor(lang, "download")}">${t(lang, "下载桌面应用", "下載桌面應用", "Get the desktop app")} ↗</a></div></section><wa-dialog id="reset-demo" label="${t(lang, "重置示例纪要？", "重設範例紀要？", "Reset demo notes?")}"><p>${t(lang, "这会替换你在此页面编辑的内容。需要保留的话，请先导出 Markdown。", "這會取代你在此頁面編輯的內容。如需保留，請先匯出 Markdown。", "This replaces your edits on this page. Export Markdown first if you want to keep them.")}</p><wa-button slot="footer" id="cancel-reset" appearance="outlined">${t(lang, "保留修改", "保留修改", "Keep edits")}</wa-button><wa-button slot="footer" id="confirm-reset">${t(lang, "重置示例", "重設範例", "Reset sample")}</wa-button></wa-dialog>`;
  }
  function home(lang) {
    const steps = [
      [
        "record-waveform",
        t(lang, "录下对话", "錄下對話", "Record"),
        t(
          lang,
          "录制麦克风，或导入已有音频。",
          "錄製麥克風，或匯入現有音訊。",
          "Record your microphone or import existing audio.",
        ),
      ],
      [
        "transcribe-document",
        t(lang, "本地转写", "本機轉寫", "Transcribe"),
        t(
          lang,
          "中英双语转写，保留原文和时间戳。",
          "中英雙語轉寫，保留原文與時間戳記。",
          "Chinese and English transcripts, with timestamps.",
        ),
      ],
      [
        "add-context-tag",
        t(lang, "记下重点", "記下重點", "Add context"),
        t(
          lang,
          "边听边写关键词，标记你关心的事。",
          "邊聽邊寫關鍵詞，標記你在意的事。",
          "Add your cues and context while you listen.",
        ),
      ],
      [
        "edit-notes",
        t(lang, "整理笔记", "整理筆記", "Edit notes"),
        t(
          lang,
          "生成可修订的纪要，按来源核对。",
          "產生可修訂的紀要，依來源核對。",
          "Get editable notes and check the source.",
        ),
      ],
    ];
    const useCases = [
      [
        "03-classroom",
        t(lang, "课堂与讲座", "課堂與講座", "Classroom notes"),
        t(
          lang,
          "留下原文，用关键词整理知识点。",
          "留下原文，用關鍵詞整理知識點。",
          "Keep the transcript. Use your cues to structure the ideas.",
        ),
        "cpu-8gb-compact",
      ],
      [
        "04-meeting-room",
        t(lang, "日常会议", "日常會議", "Everyday meetings"),
        t(
          lang,
          "选择转写与纪要模型，为会议预留资源。",
          "選擇轉寫與紀要模型，為會議預留資源。",
          "Pair a speech model with a writer, and leave room for your meeting.",
        ),
        "apple-16gb-balanced",
      ],
      [
        "05-quick-capture",
        t(lang, "随手记录", "隨手記錄", "Quick capture"),
        t(
          lang,
          "从省资源组合开始，保留日常想法。",
          "從省資源組合開始，保留日常想法。",
          "Start with an economical configuration for everyday ideas.",
        ),
        "apple-8gb-eco",
      ],
    ];
    return `<section class="home-hero"><div class="hero-copy"><p class="eyebrow">LOCAL-FIRST / OPEN-SOURCE NOTEBOOK</p><h1>${t(lang, "你的对话。<br>你的电脑。", "你的對話。<br>你的電腦。", "Your words.<br>Your machine.")}</h1><p class="lede">${t(lang, "录音、中英转写、关键词和可编辑纪要。<br>把对话整理成笔记，在自己的电脑上完成。", "錄音、中英轉寫、關鍵詞與可編輯紀要。<br>把對話整理成筆記，在自己的電腦上完成。", "A local-first notebook for recording conversations, Chinese and English transcription, personal cues, and editable notes.")}</p><div class="actions">${button(lang, "download", t(lang, "下载 OmaScribe", "下載 OmaScribe", "Download the app"))}${link("#workspace", t(lang, "体验工作区", "體驗工作區", "Explore the workspace"), true)}</div>${platformRow(lang)}<p class="eyebrow hero-caption">${t(lang, "本地处理 × 开源免费 × 无需账号", "本機處理 × 開源免費 × 無需帳號", "PRIVATE × OPEN SOURCE × YOUR WORKFLOW")}</p></div>${art("01-hero-mecha", "hero-art", true)}<p class="art-annotation">CAPTURE<br>TRANSCRIBE<br>ADD CONTEXT<br>MAKE IT YOURS</p>${demo(lang)}</section><section id="workflow" class="workflow band"><div><p class="eyebrow">01 / PRODUCT</p><h2>${t(lang, "从对话，<br>到你的笔记。", "從對話，<br>到你的筆記。", "Turn conversations<br>into your notes.")}</h2><p>${t(lang, "转写保留事实，关键词提供关注点。每一步都由你掌握。", "轉寫保留事實，關鍵詞提供關注點。每一步都由你掌握。", "Transcripts keep the facts. Your cues keep the focus. All on your desktop.")}</p></div><div class="workflow-steps">${steps.map(([img, title, desc], i) => `<article>${icon(img)}<span class="step-number">0${i + 1}</span><h3>${title}</h3><p>${desc}</p></article>`).join("")}</div></section><section class="download-feature band"><div><p class="eyebrow">02 / DOWNLOAD</p><h2>${t(lang, "安装，开始记录。", "安裝，開始記錄。", "Download OmaScribe.")}</h2><p>${releases.assets.linux_appimage ? t(lang, "macOS、Windows 和 Linux 安装包，包含默认离线模型。", "macOS、Windows 與 Linux 安裝包，包含預設離線模型。", "Installers for macOS, Windows and Linux, with the default offline models included.") : t(lang, "macOS 和 Windows 安装包包含默认离线模型。Linux 可从源码构建。", "macOS 與 Windows 安裝包包含預設離線模型。Linux 可由原始碼建置。", "macOS and Windows installers include offline models. Linux is available from source.")}</p><div class="release-strip"><span class="badge">${e(releases.tag)} / ${releases.prerelease ? "PREVIEW" : "STABLE"}</span><span>macOS · Windows · Linux</span></div>${button(lang, "download", t(lang, "选择你的平台", "選擇你的平台", "Choose your platform"))}</div>${art("02-download-recorder", "download-art")}</section><section class="community-feature band"><div class="section-top"><div><p class="eyebrow">03 / COMMUNITY</p><h2>${t(lang, "找到适合你的组合。", "找到適合你的組合。", "Share your setup.")}</h2></div>${button(lang, "community", t(lang, "浏览配置广场", "瀏覽設定廣場", "Explore configurations"), true)}</div><p class="section-intro">${t(lang, "按电脑配置选择模型和资源策略。预览参数，下载配置，也可以分享自己的组合。", "依電腦配備選擇模型與資源策略。預覽參數、下載設定，也能分享自己的組合。", "Match models and resource settings to your computer. Preview the parameters, download a configuration, or share your own.")}</p><div class="use-case-grid">${useCases.map(([img, title, desc, id]) => `<article>${art(img, "scene-art")}<div><span class="eyebrow">${t(lang, "使用场景", "使用情境", "USE CASE")}</span><h3>${title}</h3><p>${desc}</p><a href="${pathFor(lang, "community")}#${id}">${t(lang, "查看建议组合", "查看建議組合", "See a suggested setup")} →</a></div></article>`).join("")}</div></section><section class="model-feature band"><div class="section-top"><div><p class="eyebrow">04 / MODELS</p><h2>${t(lang, "模型，按需搭配。", "模型，依需求搭配。", "Your model library.")}</h2></div><p>${t(lang, "从默认组合开始，了解内存需求与实测速度，再决定是否更换。", "從預設組合開始，了解記憶體需求與實測速度，再決定是否更換。", "Start with the included pair. Compare memory requirements and measured speed before switching.")}</p></div><div class="model-teasers">${[
      [
        "06-model-speech",
        "Zipformer Small",
        "ASR / ONNX",
        t(
          lang,
          "中英转写 · 默认包含",
          "中英轉寫 · 預設包含",
          "Chinese + English · included",
        ),
      ],
      [
        "07-model-notes",
        "Qwen3 1.7B",
        "LLM / GGUF",
        t(
          lang,
          "纪要生成 · 默认包含",
          "紀要產生 · 預設包含",
          "Note generation · included",
        ),
      ],
      [
        "08-model-custom",
        "Metal / MLX",
        "APPLE SILICON",
        t(
          lang,
          "设备适配 · 查看实测与限制",
          "裝置適配 · 查看實測與限制",
          "Device options · measurements & limits",
        ),
      ],
    ]
      .map(
        ([img, title, meta, text]) =>
          `<article>${art(img)}<div><span class="eyebrow">${meta}</span><h3>${title}</h3><p>${text}</p><a href="${pathFor(lang, "models")}${meta === "APPLE SILICON" ? "#measurements" : "#catalog"}">${t(lang, "查看详情", "查看詳情", "View details")} →</a></div></article>`,
      )
      .join(
        "",
      )}</div></section><section class="network-callout"><div><p class="eyebrow">LOCAL DATA / CLEAR BOUNDARIES</p><h2>${t(lang, "对话留在本机。", "對話留在本機。", "Your conversations stay local.")}</h2><p>${t(lang, "录音、转写和 AI 整理离线运行。浏览配置广场、下载模型和安装包会联网；分享配置不包含会议内容。", "錄音、轉寫與 AI 整理離線執行。瀏覽設定廣場、下載模型及安裝包會連網；分享設定不含會議內容。", "Recording, transcription and AI notes run offline. Browsing configurations and downloading models or installers use the internet. Shared configurations exclude meeting content.")}</p></div>${button(lang, "privacy", t(lang, "逐项查看联网边界", "逐項查看連網邊界", "See the network boundary"), true)}</section><section class="closing-cta"><div><h2>${t(lang, "开始你的记录。", "開始你的記錄。", "Start your own setup.")}</h2><p>${t(lang, "录下对话，补上重点，继续编辑。", "錄下對話，補上重點，繼續編輯。", "Record. Transcribe. Add your context. Make it yours.")}</p><div class="actions">${button(lang, "download", t(lang, "下载 OmaScribe", "下載 OmaScribe", "Download OmaScribe"))}${button(lang, "community", t(lang, "浏览配置", "瀏覽設定", "Explore the community"), true)}</div></div>${art("10-footer-notebooks")}</section>`;
  }
  function downloadChooser(lang) {
    const a = releases.assets;
    return `<div class="download-layout"><div class="download-console"><wa-tab-group id="platform-tabs" active="mac"><wa-tab panel="mac" active>${platformIcon("apple")} macOS</wa-tab><wa-tab panel="windows">${platformIcon("windows")} Windows</wa-tab><wa-tab panel="linux">${platformIcon("linux")} Linux</wa-tab><wa-tab-panel name="mac" active><p class="eyebrow">macOS 11+ / DMG</p><h2>${t(lang, "为你的 Mac 选择版本", "為你的 Mac 選擇版本", "Choose your Mac build")}</h2><wa-select id="mac-architecture" label="${t(lang, "处理器", "處理器", "Processor")}" value="mac_arm"><wa-option value="mac_arm">Apple Silicon / M-series</wa-option><wa-option value="mac_intel">Intel / x86-64 · AVX2</wa-option></wa-select><p id="installer-size" class="small">${size(a.mac_arm.size)} · ${e(releases.tag)}</p><wa-button id="mac-download" href="${e(a.mac_arm.url)}" data-arm-url="${e(a.mac_arm.url)}" data-intel-url="${e(a.mac_intel.url)}" data-arm-size="${size(a.mac_arm.size)}" data-intel-size="${size(a.mac_intel.size)}" data-version="${e(releases.tag)}">${t(lang, "下载 macOS 版本", "下載 macOS 版本", "Download for macOS")} ↓</wa-button></wa-tab-panel><wa-tab-panel name="windows"><p class="eyebrow">WINDOWS 10 / 11 · x64</p><h2>Windows</h2><p>${t(lang, "适用于 Intel / AMD，需支持 AVX2。包含 WebView2 离线安装器。", "適用於 Intel / AMD，需支援 AVX2。包含 WebView2 離線安裝程式。", "For Intel / AMD processors with AVX2. Includes the WebView2 offline installer.")}</p><p class="small">${size(a.windows.size)} · ${e(releases.tag)}</p>${link(a.windows.url, t(lang, "下载 Windows 版本", "下載 Windows 版本", "Download for Windows"))}</wa-tab-panel><wa-tab-panel name="linux"><p class="eyebrow">LINUX / x86-64 · AVX2</p><h2>Linux</h2><p>${t(lang, "适用于 Ubuntu 22.04+ 及兼容发行版。麦克风录音需要 PipeWire；压缩音频导入需要 FFmpeg。", "適用於 Ubuntu 22.04+ 及相容發行版。麥克風錄音需要 PipeWire；壓縮音訊匯入需要 FFmpeg。", "For Ubuntu 22.04+ and compatible distributions. Microphone recording requires PipeWire; compressed audio imports require FFmpeg.")}</p>${a.linux_appimage ? `<div class="linux-downloads"><div><p class="small">AppImage · ${size(a.linux_appimage.size)}</p>${link(a.linux_appimage.url, t(lang,"下载 AppImage","下載 AppImage","Download AppImage"))}</div><div><p class="small">Debian / Ubuntu · ${size(a.linux_deb.size)}</p>${link(a.linux_deb.url, t(lang,"下载 .deb","下載 .deb","Download .deb"),true)}</div></div>` : `<p>${t(lang,"当前已发布版本暂未提供 Linux 安装包，可从源码构建。","目前已發布版本尚未提供 Linux 安裝包，可由原始碼建置。","This release does not yet include Linux installers. Build from source.")}</p>${link("https://github.com/EazyLee30/omascribe#build-and-install", t(lang, "阅读构建指南", "閱讀建置指南", "Read the build guide"))}`}</wa-tab-panel></wa-tab-group><div class="console-footer">${t(lang, "默认模型", "預設模型", "INCLUDED MODELS")} / Zipformer Small + Qwen3 1.7B</div></div>${art("02-download-recorder", "download-page-art")}</div>`;
  }
  return { header, footer, home, button, downloadChooser };
}
