<p align="center"><img src="assets/banner.svg" alt="OmaScribe — 为对话留下一本文档" width="100%"></p>
<p align="center"><a href="README.md">English</a> · <b>简体中文</b> · <a href="README.zh-TW.md">繁體中文</a></p>
<p align="center">
<a href="https://github.com/EazyLee30/omascribe/actions/workflows/ci.yml"><img src="https://github.com/EazyLee30/omascribe/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-d8b76e?style=flat-square" alt="MIT"></a>
<img src="https://img.shields.io/badge/Linux-Omarchy-91a87d?style=flat-square&logo=linux&logoColor=white" alt="Linux / Omarchy">
<img src="https://img.shields.io/badge/runtime-offline-647950?style=flat-square" alt="离线运行">
<img src="https://img.shields.io/badge/Rust-Tauri_2-b6c1a8?style=flat-square&logo=rust&logoColor=white" alt="Rust + Tauri 2">
</p>

# 你写几个词，会议补全上下文。

**OmaScribe 是面向 Linux 的离线录音、中英转写与 Markdown 会议笔记应用。** 左侧随手记录关键词，右侧结合会议原话，自动整理为有标题、有段落、可回听的会议文档。

围绕一台运行 Omarchy 的 2016 年 12 英寸 MacBook 开发：双核 Intel、8GB 内存。名字来自 **Oma**rchy 与 **Scribe**。

<p align="center"><img src="docs/screenshots/workspace-zh-CN.png" alt="左侧 Markdown 文档与右侧自动会议纪要" width="100%"><br><sub>实际应用渲染；图中会议内容为虚构测试样本。</sub></p>

## 一边听，一边写

- **开始、暂停、继续、结束。** 录音独立写盘，暂停时停止采音，继续后追加到同一会议。也支持导入 WAV、MP3、M4A、FLAC、Ogg 等音频。
- **Markdown 输入，文档呈现。** 与应用统一的深色编辑器，支持工具栏、标题、列表、待办、表格、引用、代码、链接、撤销重做、源码切换和中文输入法。
- **用 `/` 快速操作。** 搜索内容块、插入录音时间戳、标记关键词、使用会议模板。按 Escape 保留普通斜杠；正文和路径中的斜杠不会弹出菜单。
- **自动形成会议纪要。** 关键词引导中英检索，本地模型将相关转写整理为流畅段落，无需手动点击更新。
- **转写位置由你决定。** 右侧停靠、可拖动和缩放的浮窗、收起模式、左右吸附。屏幕字幕支持顶部／底部、显示器、字号、颜色、透明度、宽度、行数、边距、对齐和鼠标穿透设置。
- **人工内容始终由你掌控。** AI 不改写左侧笔记；人工编辑右侧段落后，该段受到保护。每段保留原话和录音时间戳，转写被修改时会提示复核。
- **完整导出。** 导出个人文档、当前纪要、出处与转写，可同时导出 WAV。界面提供英语、简体中文与繁体中文。

## 关键词如何变成文档

你写下 `预算 3000？` 和 `张老师 / 周五测试`，原话是：

> 张老师周五给测试结果，预算先不定，下周再看。

期望的纪要是：“张老师将于周五提供测试结果。预算尚未确定，将于下周继续讨论。” 每段能展开对应原话，`3000？` 仍是你的个人笔记。

系统合并频繁变化，单次只运行一个写作任务。在这台老电脑上，纪要按批次出现，录音和转写优先。**小模型仍可能理解错误或遗漏内容**；出处、数字和未定状态校验只能减少部分错误，不能证明所有内容都正确。

## 构建与安装

当前支持 **x86-64 / AVX2 Linux、PipeWire、GTK3、WebKitGTK 4.1**，在 Omarchy 上验证。暂不支持 macOS 或 Windows。

```bash
sudo pacman -S --needed base-devel git cmake pkgconf nodejs npm python \
  gtk3 webkit2gtk-4.1 openssl librsvg ffmpeg pipewire wayland wayland-protocols xdotool
```

准备稳定版 [Rust 工具链](https://www.rust-lang.org/tools/install)，然后：

```bash
git clone https://github.com/EazyLee30/omascribe.git
cd omascribe
bin/build       # 默认单任务编译
bin/setup       # 下载校验过的模型，构建本地推理与字幕运行库
bin/install     # 安装到当前用户及应用启动器
~/.local/bin/omascribe
```

首次准备需要联网，模型下载约 1.6GB，另需编译缓存空间。准备好后，录音、转写、编辑、推理与导出均可离线使用，无需账户或 API Key。模型不进入 Git 仓库。无风扇设备编译时可能升温并暂时影响桌面响应。

## 日常使用

1. 新建会议，在设置中选择麦克风。
2. 开始录音。用工具栏、Markdown 语法或 `/` 菜单记录，把重点词加粗。
3. 右侧纪要自动更新。点击「实时转写」打开浮窗，齿轮调整显示方式，**CC** 控制屏幕字幕。
4. 按需暂停、继续。点击出处回听，编辑纪要段落可保留自己的措辞。
5. 结束录音后，等待剩余转写和纪要处理完成，再导出。

| 快捷键 | 操作 |
| --- | --- |
| `Ctrl Shift R` | 开始／结束录音；暂停时继续 |
| `Ctrl Shift P` | 暂停／继续 |
| `Ctrl B` / `Ctrl I` | 加粗／斜体 |
| `Ctrl K` | 插入或修改链接 |
| `Ctrl Alt 1 / 2 / 3` | 标题层级 |
| `Ctrl Shift 8 / 7 / 9` | 无序／有序／待办列表 |
| `Ctrl Z` / `Ctrl Shift Z` | 撤销／重做 |
| `Ctrl F` | 打开右侧转写并搜索 |
| `Ctrl N` | 新建会议 |
| 空段落开头输入 `/` | 内容菜单；方向键选择、Enter 确定、Escape 保留斜杠 |

## 本地架构与边界

`独立录音 → 流式 ASR → SQLite → 关键词与词法 RAG → 小模型写作 → 校验 → 文档版本保存`

默认使用双语 Zipformer 和 Qwen3-1.7B Q4_K_M。写作模型单线程、单请求、2,048 token 上下文，任务结束后卸载。转写持续积压时，自动让出写作算力。失败和取消不会推进已处理位置。

默认数据位于 `~/.local/share/omascribe/`，遵循 `$XDG_DATA_HOME`。目录仅当前用户可访问，应用本身不加密文件。可用 `OMASCRIBE_DATA_DIR`、`OMASCRIBE_MODELS_DIR`、`OMASCRIBE_RUNTIME_DIR` 指定位置。CLI 提供 `diagnose`、`import`、`record`、`replay`、`snapshot`、`note`、`generate`、`export`，输出 JSON。

- 当前采集麦克风，尚未实现系统音频混录和说话人分离。
- 小型 ASR 的人名、远场、重叠说话和中英混说准确率需要真实场景评估；关键词当前用于笔记检索，不做 ASR 热词偏置。
- RAG 使用会议内 BM25、中文字符／双字及精确词匹配，不常驻向量模型。
- 浮窗吸附针对 Hyprland；屏幕字幕需要支持 layer-shell 的 Wayland 合成器。
- 会议期间请保持电脑唤醒；睡眠、驱动切换后的恢复尚未充分验证。
- 没有云同步、跨会议搜索或自动更新器。

[实测报告](docs/BENCHMARKS.md) · [测试与回归 harness](docs/TESTING.md) · [架构](docs/ARCHITECTURE.md) · [模型与许可证](docs/MODELS.md) · [贡献](CONTRIBUTING.md) · [隐私](SECURITY.md)

交互灵感来自 [Google Foresight](https://developers.google.com/edge/foresight)，这是独立的 Rust/Tauri 实现。感谢 Tauri、Tiptap、sherpa-onnx、llama.cpp、Qwen 和 gtk-layer-shell。代码及原创图形采用 [MIT](LICENSE)；第三方模型与依赖保留各自许可证。
