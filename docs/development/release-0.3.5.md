# 0.3.5 发布收敛清单

共享生产修复：[`b858d66`](https://github.com/EazyLee30/omascribe/commit/b858d66078f0f0859b6ac9ef5196d4b1175e3af0)；撤回实验后的完整检查源码为 `6dc54e5`。稳定版仍为 [0.3.4](https://github.com/EazyLee30/omascribe/releases/tag/v0.3.4)，**尚无 0.3.5 Release 或正式安装包**。[PR #1](https://github.com/EazyLee30/omascribe/pull/1) 保持 draft。

## 当前阻塞与进行中工作

| 项目 | 负责人 | 当前验证结果与下一步 |
| --- | --- | --- |
| Q16：纪要正文质量 | 主线程；三端复核 | **FAIL**。位置分组候选只运行一次，1 job / 2 完整回复；全零分组虽合法，正文仍改名、重复音乐并把可能结束写成已结束，cursor 0、文章 0，来源 6 未进入。`6dc54e5` 已撤回候选，不调参重跑；结构控制的合成 0→5→6 不冒充模型质量。[原输出、成本与撤回](../evaluation/topic-partition-stop-2026-10-31.json)。[此前 1.7B / 4B 对照](../evaluation/qwen4b-admission-stop-2026-10-31.json)继续保留。 |
| Q9：真实语音专名召回 | 主线程 ASR；Windows 实录 | `52c2b95` 的同源 FLEURS 对照仍有名称未命中；不能据热词准入或合成文本通过宣称实录识别已解决。下一步限定实际说出的名称与时段核验。[原对照与范围](../evaluation/note-asr-hotword-admission-2026-10-31.json)。 |
| 三端候选交付 | 各平台负责人 | 新候选仍需同一固定 SHA 的真实录音、停止、原生窗口、GPU/CPU 回退及安装升级验收；历史不同构建结果不能合为当前版本通过。 |
| 同输入重复失败成本 | 主线程／调度与运行时 | 尚未实施抑制；完整 runtime／组件／驱动失效身份未闭合，不用弱 epoch 替代。终态归因已独立修复，完整身份与质量门继续推进。[修复与未完成边界](../evaluation/minutes-terminal-diagnostics-2026-10-31.json)。 |

## 已完成范围

以下历史结果保留各自源码身份，不替代当前共享源码的完整验收：

- `b858d66`：专名保存后的转写、标记、纪要与引用统一提交，文档写失败不再消费重试依据；用户笔记保留，同内容重试无模型完成同步。恢复后的源码完整 558 核心 / 12 忽略、2 CLI、30 集成及严格检查通过；该生产提交 Windows / Linux CI 通过。旧版本既有半提交数据不迁移。[真实红绿测与原子边界](../evaluation/note-spelling-atomic-retry-2026-10-31.json)。
- `e9d8167`：终态来自明确标记及实际 Result，后续 Scope／资源／SQL 停止不再继承首稿拒绝；原尝试和成本保留。本机 555 核心、2 CLI、29 集成及严格检查通过。[真实红绿测与范围](../evaluation/minutes-terminal-diagnostics-2026-10-31.json)。
- `6217c0f`：两条自动路径按实际输入清旧失败罚时，保留 45 秒节流，旧任务不能重建恢复后会议的罚时。[输入变化与调度证据](../evaluation/minutes-input-aware-retry-2026-10-31.json)。
- `dd6d90c`：同 ID 笔记纯拼写更正可复用已完成纪要，保留人工与歧义保护；任意音近识别仍未解决。[专名同步与无新增纪要任务控制](../evaluation/apple-candidate12-spelling-reuse-2026-10-29.json)。
- `e77f911`：冲突别名保护、实际笔记上下文与定向读取；容量和耗时观察只代表该读取步骤。[原红测与范围](../evaluation/ambiguous-alias-writer-lookup-2026-10-31.json)。
- `ad04cc0`：ASR 热词必要覆盖与无变化不重载、冷状态无凭据时跳过无效驱动读取。原 PCM 否定前缀反例与 canonical PCM 对照保留。[真实 worker 验证及边界](../evaluation/asr-coverage-and-cold-selection-2026-10-31.json)。
- `f8c2478`：显式 Vulkan ICD 内容绑定、harness 首轮无进展止损、Windows 原 Child 身份退出测试；真实 GPU 与默认驱动发现仍待验证。[原失败及修复检查](../evaluation/vulkan-driver-and-evaluation-stop-2026-10-31.json)。
- `61b62d7`：默认录音中失败退避按 meeting/run 隔离，保留原节流与手动重试。[调度红绿测](../evaluation/default-live-minutes-retry-2026-10-31.json)。

## 三端下一步与发布门

- **主线程 / Mac M 系列**：优先解决 Q16 的专名、事实归属、否定与不确定性保真；继续 Metal 和低资源适配。已失败候选按止损撤回，不继续提示词调参矩阵。
- **Windows**：围绕下一固定 SHA 定向核验实际专名、输入停止、CUDA 与 CPU 回退；复用已签组件，随后验收安装升级。
- **Linux / Omarchy**：同 SHA 验收本端音频、原生窗口与操作、可用 GPU 及 CPU 回退、安装升级；共享 CI 不替代实体设备结果。
- 开发期跑受影响检查并及时推源码；冻结候选后再统一跑完整语料、真实录音和安装升级，最后构建候选包。三端在 PR #1 协调共享改动，避免重复实现。
- 差分更新、完整说话人重构、新模型与新后端继续独立研究，不扩大本次发布范围。发布时间待下一候选验收后确定。

## 历史与证据

[截至 `864dc69` 的完整发布台账](https://github.com/EazyLee30/omascribe/blob/864dc69c6bd25918e5ddbce211e8370ee17b57d3/docs/development/release-0.3.5.md)保留旧候选、逐项结果、平台反馈及原失败链接。所有 [evaluation 原证据](../evaluation/)继续保留；失败、通过和成本按原源码分别记录，不由后续结果覆盖。
