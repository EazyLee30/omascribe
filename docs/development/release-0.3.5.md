# 0.3.5 发布收敛清单

这是三端共用的唯一发布阻塞清单。日常只更新问题、负责人、固定源码与验证结果；原始通过、失败、运行输出和哈希保留在证据文件中。

## 固定范围与当前状态

0.3.5 优先解决预算更正、否定/撤回、人物与动作对象、无依据补写、字幕收尾，以及实际阻塞安装、启动、录音、停止或升级的问题。差分更新、完整说话人重构、二次ASR/VAD、新模型和新后端继续研究，不把全部研究塞进本次小版本。

当前固定候选8：`1ff746bd9acd6061278379d9de4e41d735e05c86`，分支 `codex/release-0.3.5-candidate-8` 已推送且不移动。本地279核心/7忽略、27 harness及严格Clippy通过，Windows-core/Linux与prepare/validate CI通过；5项受影响实际检查全部通过后，同一干净CLI/核心/harness/模型身份完成一次完整61项自动双语回归，**61/61原自动检查通过，身份前后相同**。三分片逐阶段复核72份快照、142条字幕、139处引用。仍有字幕称谓/动作与语言质量问题，完整语义和发布验收未通过；不把引用匹配当事实全保留。[候选8完整结果与限制](../evaluation/apple-candidate8-quality-2026-10-29.json)，[5项闭环](../evaluation/apple-closed-retention-2026-10-28.json)。

候选7与c6/be3的原失败均保留，不用新通过覆盖旧记录。下一步只收敛已发现的字幕称谓、动作与状态表达，并由三端围绕候选8验收本端原生/录音/安装路径；需要共享源码修复时冻结新候选，不重指向候选8。

## 失败项—负责人—验证结果

| ID | 问题 | 负责人 | 固定来源、验证与下一步 |
| --- | --- | --- | --- |
| Q1 | 预算更正与未批准 | 主线程/共享纪要 | 候选8完整语料与逐阶段复核保留3000→5000、无新增币种、未批准、独立同额发票及后续话题；受测语料范围通过。 |
| Q2 | 计划/完成、人物与动作对象 | 主线程/共享字幕和纪要 | 候选8真实提交/计划提交、页面发布/测试链接、Riley/Morgan等完整控制通过。复核仍发现字幕“刘老师继续负责布置”→“Mr. Liu…assigning”，新增性别称谓且动作漂移；中文纪要正确。另有未完成状态被译成without completing及发布it先行词歧义，须窄项复核。 |
| Q3 | 星期擅加周次 | 主线程/共享验证 | 候选8未限定/本周/下周和绝对日期控制均保留各自范围；不多加周次，日期/星期同时保留。受测范围通过，不宣称任意跨任务日期均已证明。 |
| Q4/T1 | 录音对象、本机保留与源内关系 | 主线程/共享验证和生成 | 1ff五项窄闭环和完整候选语料均保留Friday否定、录音IN及本机，notes-only不添录音。只对原文全部字段可确定的来源使用闭合约束；未确定前后文、姓名和疑问弃权，不靠自由尾补原话。 |
| Q5 | CER/WER/SER类型与后续独立事实 | 主线程/共享字幕和纪要 | c8c1171的既有定向Mac/Windows证据保留，指标、缓存后文和否定窄项通过。数值绑定与更广语义仍待验证；不追加完整CPU×GPU矩阵。 |
| Q6 | 大写字幕收尾成本 | 主线程/共享调度与harness | 1ff窄项11.854秒完成，完整候选再次通过，五段字幕与纪要核心事实保留。旧120秒失败及错误输出不改写；无受控速度/能耗提升百分比声明。 |
| Q7 | 焦点关键词被误当专名 | 主线程/术语与纪要 | 保留原fixture **date**，候选8绝对日期字幕/纪要保留2024-5-7星期二，无新增周次。显式词表和实际名保护仍保留；Q2称谓问题继续单列，不宣称所有中文姓名均已解决。 |
| Q8 | 提示词或无依据限制写入字幕/纪要 | 主线程/共享生成 | c6误提交已拒绝并保留证据。1ff移除自由续写，实际5项及完整语料未再出现该模板泄露/复制禁令；原文完整字段约束和负例通过。任意模型幻觉不宣称全面解决。 |

“微检”、paper→“论文”、重复噪音文章的精炼继续登记为已知词义/可读性问题，不另开提示词调参矩阵。完整引用只证明出处匹配，不能证明正文保留了每个事实。

## 三端与发布门

| 平台/门 | 现有证据 | 尚未证明的范围 |
| --- | --- | --- |
| Mac/M系列 | 候选8完整61项自动PASS，72份实际M5 Metal日志29/29，279核心/严格Clippy通过；完整逐阶段复核保留开放质量项。 | Q2称谓/动作及普通词festival未译等需收敛；真实麦克风、物理窗口和安装升级尚无本候选验收，桌面工具仍报告锁屏。 |
| Windows | GTX1650/621.01在a8指定两项复用签名CUDA；ALICE关系/链接纪要窄项正确，录音遗漏失败，29/29 offload；core243/6忽略、标识符7及Clippy通过。 | 共享新源码未完成硬件复测、完整语料或安装发布验收；本轮未调用的旧ASR worker不算同构建证据。 |
| Linux/Omarchy | 共享源码CI和GPU组件隔离测试已有证据；CUDA/Vulkan组件保持独立。 | 实际GPU/音频/原生窗口和候选安装升级仍待本端验收，不能由编译通过代替。 |
| 组件与模型复用 | 签名/hash/原子安装/回滚路径及组件污染负例已有代码与CI；基础包禁CUDA/Vulkan/HIP依赖。 | 新候选安装包及升级保留模型/组件流程仍需实测。没有为了本批语义修复重编SDK/组件或安装包。 |

## 交付节奏

- 主线程负责共享转写/翻译/纪要、事实校验、Mac Metal、集成和候选冻结；Windows负责本端输入/停止、冷ASR、CUDA与回退；Linux负责本端桌面/音频及Omarchy。共享问题在PR给最小差异，避免重复实现。
- 开发期只跑受影响检查，及时推送源码；已知共享失败未修好时不启动完整矩阵。候选冻结后，三端围绕同一SHA完成完整语料、真实录音、必要原生和安装升级验收；最后统一构建候选包。
- 复用已签GPU组件、模型和SDK；只有组件缺陷才重编/重签。Windows暂不重跑已知共享失败或追加完整CPU×GPU矩阵，等待主线程给新的固定SHA及最小范围。
- PR #9已整合到be3：literal-only延迟启动、两个8秒工作预算及首次CPU回退缓存身份均共用现成实现；无需另写一遍。
- 只报“问题ID—负责人—SHA—结果—下一阻塞”，无新增阻塞可直接回报。原始证据保留，测试借用模型副本释放不等于同量磁盘空间回收。
- 发布时间等下一候选完整回归后再估算；0.3.5发布也不等于P0/P1/P2/P3/G1/G2全部研究结束。

## 原始证据

- [候选8完整61项与开放质量项](../evaluation/apple-candidate8-quality-2026-10-29.json)，[1ff五项闭环](../evaluation/apple-closed-retention-2026-10-28.json)。[c6误提交与拒绝](../evaluation/apple-retention-context-rejection-2026-10-28.json)、[be3原失败](../evaluation/apple-retention-source-fields-2026-10-28.json)、[c57原检查](../evaluation/apple-week-retention-2026-10-28.json)、[a8录音遗漏更正](../evaluation/a8-recording-retention-correction-2026-10-28.json)保持原记录。
- [a8 8项自动+3项独立纪要](../evaluation/apple-source-action-language-2026-10-28.json)，[841中间18项](../evaluation/apple-source-task-submission-trial-2026-10-28.json)。各来源分开，不拼接为同源总数。
- [Windows a8原报告](https://github.com/EazyLee30/omascribe/blob/7c33bb233cce830da0d7856bfad5f541842b473e/docs/evaluation/windows-a8f1058-targeted-2026-10-28.json)，[前一c8定向报告](https://github.com/EazyLee30/omascribe/blob/d5aa285a400f9fb867e2d74588ac7794bd58a021/docs/evaluation/windows-c8c1171-targeted-2026-10-28.json)。
- [候选7完整51项](../evaluation/apple-candidate7-quality-2026-10-28.json)，[候选6语义失败](../evaluation/candidate6-semantic-failures-2026-10-28.json)，[候选6回放](../evaluation/apple-candidate6-recording-2026-10-28.json)，[候选6公开ASR](../evaluation/apple-candidate6-asr-public-2026-10-28.json)。
- [候选5原始输出与字幕收尾](../evaluation/apple-candidate5-caption-finalization-2026-10-28.json)，[候选5回放](../evaluation/apple-candidate5-recording-2026-10-28.json)，[候选4及中间失败](../evaluation/apple-caption-source-spellings-2026-10-27.json)。更早证据仍在evaluation目录与本文件Git历史中。
