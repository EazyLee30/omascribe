# 核心功能验收

验收围绕真实转写和本地纪要生成。测试使用公开音频及虚构会议，始终指定独立数据目录，不打开用户会议数据库。

## 准备和执行

先构建本地 CLI 与识别进程，并指定已有模型和运行时。模型目录遵循应用安装目录的 `models/zipformer`、GGUF 布局；额外模型位于 `OMASCRIBE_TEST_MODEL_STORE/<model-id>`。该目录只通过硬链接或副本复用于独立工作区，测试不修改模型文件。

```sh
cargo build -p omascribe-core -p omascribe-asr
export OMASCRIBE_MODELS_DIR=/absolute/path/to/models
export OMASCRIBE_RUNTIME_DIR=/absolute/path/to/runtime
export OMASCRIBE_TEST_MODEL_STORE=/absolute/path/to/test-model-store

# 这是唯一主动联网的步骤：约 815 MB 官方测试集，按固定 SHA-256 校验。
python3 tests/prepare_asr_fixtures.py --directory /tmp/omascribe-public-fleurs

# 运行时不下载模型或语料。输出目录必须全新，防止覆盖上次的失败证据。
python3 tests/harness.py \
  --fixtures /tmp/omascribe-public-fleurs \
  --output artifacts/core-run-1 --repeat 3 \
  --small /absolute/path/to/models/zipformer \
  --quality /absolute/path/to/test-model-store/zipformer-bilingual \
  --stress-minutes 30

# 对同一套语料比较转写错误率，任一模型/语言回退超过 2 个百分点即失败。
python3 tests/harness.py \
  --fixtures /tmp/omascribe-public-fleurs \
  --output artifacts/core-run-2 --baseline artifacts/core-run-1
```

`runtime` 需要 `omascribe-asr` 和本地 `llama` 运行时；Windows 使用 `.exe`。可用 `--cli` 指定已安装版本的 CLI。测试不依赖云端推理，关闭模型下载和在线推荐。这里的策略检查不等于抓包证明；压缩音频解码的协议白名单另有网络访问阻断测试。

## 检查什么

- **转写质量**：FLEURS 官方 test.tsv 中前四个不同句子，中文和英语各四段，固定版本及音频摘要。中文报告字错率 CER，英语报告词错率 WER；按总编辑数/总参考单位加权，不取每段百分比的简单平均。另测数字静音及固定随机种子的低音量合成噪声。
- **热词兼容**：实际运行两个模型，覆盖自带分词器、词表一致时共享分词器、缺失及不兼容分词器的降级路径，检查启用热词后的静音。
- **纪要**：实际运行本地 LLM，检查连续段落、未定状态、负责人及期限、后续更正、双向输出语言、纯试麦不成文、说话人归属，以及超过九条证据的文章。引用必须精确匹配片段 ID、修订版本和原话。多阶段测试模拟讨论推进，人工确认段落逐字段保持不变。
- **名称贯通**：中文笔记中的英文名称、英文复合专名及近似拆词，必须同时出现在校正后的转写和纪要中；禁止残留已知错误名称。笔记名称不允许被编造成会议事实。Rust 反例覆盖无上下文、短缩写、竞争名称、已知独立名称、普通英文词、别名二次更新、原文历史和人工修改保护。
- **录音归档**：跨进程续录保留人工修订；PCM 与可播放 WAV 一致；全量重转写保留录音边界；分段导出的时间戳从该段录音起算，允许开头存在静音。
- **长音频**：可选 1–120 分钟公开样本重复拼接，加速导入后检查保存的样本数、WAV/PCM 一致性、时间戳及识别完成状态。此项不能替代真实麦克风、休眠恢复和多种声音环境的长会测试。

转写绝对质量默认门槛为该小样本每模型/语言错误率不超过 45%，同时拒绝空转写、静音出字、时间戳异常和基线退化。此门槛用于发现严重退化，**不是产品准确率承诺或理想目标**。专名和口音仍需更多录音验证，不允许为了过关替换失败样本或提高阈值。

2026-10-27 已实际准备并校验八段官方朗读及两段构造切换音频，使用当前 CPU worker 完成两个模型共 24 项（含各自静音/低噪声）的基线。逐组错误率及实际 worker/模型/语料摘要见 [ASR 基线](evaluation/asr-public-2026-10-27.json)，精确参考文本与片段来源见 [公开 manifest](evaluation/fleurs-manifest-2026-10-27.json)。小模型中文 CER 32.45%、英文 WER 34.21%、拼接 MER 29.94%；双语大模型为 28.19%、32.89%、30.54%。这是严重退化烟测通过，部分单段仍有较高错误率，不能称为理想识别质量或换模型即可改进。未改默认模型/beam/score，也未用该小集作自然混说、专名或说话人评估。

准备命令现在打印 manifest SHA-256。记录该值后，可以清理已完成的 `.tar.gz` 下载档案，保留 manifest 和所选 WAV，以后离线验证复用，不重新下载 815 MB：

```sh
python3 tests/prepare_asr_fixtures.py --directory artifacts/fixtures/fleurs-20261026 \
  --verify-existing 76bcdade9d87b472a53e5f1e801d4aeaa32b5932c18c2a79f43c2f43eee9eb69
```

该摘要对应这次实际制备的十段语料。其他环境应使用自身已校验制备时记录的摘要；WAV 转换器的元数据可能随工具版本变化，不能只因 PCM 相似就强行改成同一文件摘要。命令同时校验 manifest 和每段音频，任何参考文本/音频修改都会拒绝复用。它不隐式修补失败证据或替换样本。

纪要先由本地模型生成正文，再由代码绑定本次选入的转写来源组，相关片段排在前面；绑定过程不再让模型改写正文。引用表示成文使用的上下文，不能证明每句话都被原话蕴含。金额、日期、待定状态、说话人和无依据因果词另作保守检查；这仍不能替代语义核对。以试麦和填充词为主的输入使用更窄的概括提示，不要求模型补齐负责人和期限，原始转写与引用上下文继续保留。

## 结果和失败证据

Apple Silicon 的并行负载验收使用 `tests/benchmark_recording_contention.py`：同一公开中英拼接音频补足静音边界，按真实时间回放；两轮反转顺序，对照仅 ASR 与 ASR＋Metal 双语翻译、自动纪要。运行时须同时包含当前 `omascribe-asr`、`llama/`、`platform/ffmpeg`，模型目录使用已有已校验文件，不下载组件。

```sh
python3 tests/benchmark_recording_contention.py \
  --fixtures artifacts/fixtures/fleurs-20261026 \
  --runtime /absolute/path/to/runtime \
  --asr-model-dir /absolute/path/to/zipformer-small \
  --llm-model-dir /absolute/path/to/qwen3-1.7b \
  --output artifacts/benchmarks/apple-recording-pair.json
```

CLI 的 `replay AUDIO SECONDS --telemetry NEW_JSONL_PATH` 每 200 ms 保存无正文的时序、积压、任务进度及 worker 计数。验收拒绝不完整录音、PCM 字节变化、ASR 输出变化、超过 2.5 s 的处理积压、超过 1.5 s 的采样空洞，以及后台没有实际翻译/纪要进展的空负载；任务 trace 必须报告 Metal，最后一个 worker 另查真实 offload 日志，CPU 回退不能冒充 Metal 成功。末尾尚未处理的任务如实报告，不在计时外补跑后宣称实时完成。CPU 时间与进程树 RSS 由同一采样器记录，不能据此推断能耗或独立显存。此项是文件回放的资源竞争验收，不能替代真实麦克风、自然会议语义、长会、设备故障和 M1–M4 实机结果。

总入口的 `report.json` 汇总每个子套件。未完成全部场景时不会标记为通过。CI 上传诊断时排除复用的模型目录，保留文本、数据库、测试音频和校验结果。单个场景失败后继续运行其余场景，并以非零状态退出。保留输入、进程输出、错误、每次生成草稿、校验失败原因、引用、模型/CLI 摘要、推理后端、耗时和 token 使用量；输出中途断开时，保留本地草稿但不提交为正式纪要。纪要生成诊断只覆盖最近一次任务，避免在产品数据目录无限追加正文副本。

`*.artifacts/workspace` 是测试专用数据库及音频；排错后可自行删除整个输出目录。不得把实际用户录音添加为公开 CI fixture。示例中的 `artifacts/` 已被 Git 忽略。

跨平台安装包烟测执行同一份完整的本地纪要场景清单（目前十八个），覆盖中文日期、英文期限、试麦过滤、多轮更正、说话人、长上下文、名称贯通、双向输出语言、人工段落保护和含混转写不得补造议题。轻量 CI 执行评分器测试、Rust 校验及流式中断/原文保护测试；公开语料全套需要显式准备后运行。

本轮固定样本的前后对照及限制见 [core-0.3.3.json](evaluation/core-0.3.3.json)。

## 语料和指标范围

[FLEURS, Google](https://huggingface.co/datasets/google/fleurs)，CC BY 4.0，固定 revision `70bb2e84b976b7e960aa89f1c648e09c59f894dd`。处理包括选取固定句子和转换为 16 kHz 单声道 PCM；原始音频及转换音频均记录摘要，所有文本使用官方参考。中文参考有英文专名括注，因此 CER 也包含这些字符；不人为删除模型识别困难的词。

归一化：NFKC、小写；中文计入字母和数字而移除标点/空白，英文按词计算、保留词内撇号、连字符分词。不进行任意数字、同音字或专名等价化。八段朗读音频仅用于固定回归，不能代表嘈杂会议、中英混说、重叠说话或总体识别准确率。热词实现遵循 [sherpa-onnx 官方上下文偏置文档](https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html)。

## 本地专名录音回放

```sh
python3 tests/replay_terminology.py \
  --wav /absolute/path/to/16k-mono.wav \
  --notes /absolute/path/to/note.txt \
  --expected OmaScribe --forbidden 'ALMA SCRIBE|OAS' \
  --language zh --output /tmp/fresh-terminology-replay
```

先把用户提供的名称写入隔离会议，再运行真实识别、自动校正和本地纪要。检查标准名称的大小写、错误名称残留、修订归档、全部转写覆盖和引用一致性。`--expected` / `--forbidden` 可重复指定；`--forbidden-minutes` 只检查纪要，适合禁止照抄原始杂音或补造议题，再人工核对成文。名称与引用通过不等于每一句含义都已验证。这里的记录可能包含私人内容，只留在本机；不作为默认 CI 语料，也不得上传。
