# 0.3.5 发布收敛清单

共享生产源码：[`6217c0f`](https://github.com/EazyLee30/omascribe/commit/6217c0f3889c86701e3d09b7c48099ca7842044e)。稳定版仍为 [0.3.4](https://github.com/EazyLee30/omascribe/releases/tag/v0.3.4)，**尚无 0.3.5 Release 或正式安装包**。[PR #1](https://github.com/EazyLee30/omascribe/pull/1) 保持 draft。

## 当前阻塞与进行中工作

| 项目 | 负责人 | 当前验证结果与下一步 |
| --- | --- | --- |
| Q16：纪要正文质量 | 主线程；三端复核 | **FAIL**。固定同源 1.7B / 4B 对照均仅运行一次，各 1 job / 2 请求，cursor 0、文章 0；4B 虽 Metal 37/37，仍改名、漏大学名，首稿另有无据状态评论。按预设撤回模型条目并清理下载副本；未跑 68/18 扩展。[本轮真实对照与止损](../evaluation/qwen4b-admission-stop-2026-10-31.json)；[此前提醒试验](../evaluation/minutes-reminder-stop-2026-10-31.json)。 |
| Q9：真实语音专名召回 | 主线程 ASR；Windows 实录 | `52c2b95` 的同源 FLEURS 对照仍有名称未命中；不能据热词准入或合成文本通过宣称实录识别已解决。下一步限定实际说出的名称与时段核验。[原对照与范围](../evaluation/note-asr-hotword-admission-2026-10-31.json)。 |
| 三端候选交付 | 各平台负责人 | 新候选仍需同一固定 SHA 的真实录音、停止、原生窗口、GPU/CPU 回退及安装升级验收；历史不同构建结果不能合为当前版本通过。 |
| 输入变化清旧失败罚时 | 主线程／调度作者 | `6217c0f` 已提交并完成本机验证：551 核心测试、29 集成、CLI 及严格静态检查通过。两条自动路径绑定实际输入，清旧罚时并保留 45 秒节流；删除恢复后的旧任务不能重建罚时。模型质量与三端验收另列。[原红测、最终检查与边界](../evaluation/minutes-input-aware-retry-2026-10-31.json)。 |

## 已完成范围

以下历史结果保留各自源码身份，不替代当前共享源码的完整验收：

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
