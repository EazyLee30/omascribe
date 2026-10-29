# 0.3.5 发布收敛清单

这是三端共用的唯一发布阻塞清单。日常只更新问题、负责人、固定源码与验证结果；原始通过、失败、运行输出和哈希保留在证据文件中。

## 固定范围与当前状态

0.3.5 优先解决预算更正、否定/撤回、人物与动作对象、无依据补写、字幕收尾，以及实际阻塞安装、启动、录音、停止或升级的问题。差分更新、完整说话人重构、二次ASR/VAD、新模型和新后端继续研究，不把全部研究塞进本次小版本。

候选13固定`67e04649189020438cdc4e7c393124f359a6dea1`及不可移动分支`codex/release-0.3.5-candidate-13`。完整自动调度原65项通过，但新增3项未声明中文纪要设置、其中文断言与auto产生的英文文章冲突，故完整结果保持**65/68 FAIL**；字幕通过、原复数/未批准/专名与引用保留。另一次操作者误将全语料强制中文的中断试验（23项、2语言失败）独立保留，进程及专用模型克隆已清理。只补新3项`minutes_language=zh`，不改原65字节、任何源文/断言/预算；fixture-only修改也纳入Windows-core触发，GPU编译仍需显式选择。下一份候选需完整重跑；候选12和13均不移动，未发0.3.5。[原始失败与配置边界](../evaluation/apple-candidate13-language-config-2026-10-30.json)。

当前固定候选12：`dd6d90cac30f992b3b642f77deed3d6b55d1ff4e`，分支 `codex/release-0.3.5-candidate-12` 已推送且不移动。331核心/7忽略、原有集成7项、专名集成12项、严格workspace Clippy及精确源码Windows-core/Linux/prepare/Website CI通过；GPU build/catalog跳过。**65/65原自动检查在一次连续运行中通过，源码/CLI/harness/23份既有资产身份前后相同**。三分片复核80份快照、148条字幕、147处引用；原64项仅一处字幕把源明确的批准状态从“预算未通过”精确化为“预算未获批准”，其余可见内容与候选11相同。新增第65项证明：同一现有document只改专名拼写，转写/正文/引用修订1→2，随后字幕补齐，纪要jobs1→1、文章与引用不再改写。新增预算/主题、换document、原有dirty、未覆盖、人工/过期/歧义继续正常整理。原a6的5PASS/1FAIL和全部红测保留，未改source/断言/预算。**尚无0.3.5正式安装包，稳定版仍为0.3.4**；真实ASR、平台和安装升级门仍开放。[候选12结果与范围](../evaluation/apple-candidate12-spelling-reuse-2026-10-29.json)。


独立G1开发跟进 `b569e32`：原生加载前模型SHA校验与进程内缓存、Windows全生命周期读句柄、Mac ARM校验加速已推送；Windows原三项失败保留，修复后同SHA CI通过。此项不移动候选12，也不增加本次发布范围；实际专名/停录和平台/安装验收继续按下表推进。[独立证据及未完成边界](../evaluation/apple-model-integrity-2026-10-29.json)。随后 `e30460a` 固定原生worker明确传入的共享BPE路径，避免新建本地BPE改路，并限制读取上限；同源码Windows/Linux CI通过，[原发现与修复范围](../evaluation/apple-shared-tokenizer-2026-10-29.json)。

候选11：`ed9d2e7f7800219f98156fec41fec279d76f1681`，分支 `codex/release-0.3.5-candidate-11` 保持原位。325核心/7忽略、原有集成7项、专名集成8项、严格workspace Clippy及精确源码Windows-core/Linux/prepare/Website CI通过；GPU build/catalog跳过。**64/64原自动检查在一次连续运行中通过，源码/CLI/harness/23份SDK、模型和runtime身份前后相同**。三分片复核76份快照、145条字幕和143处引用；原63项全部可见正文与候选10一致。新增项修复实际`MX-2750`被误判为新增数字的字幕阻塞。另有晚补专名字幕专项的严格“后续零生成/文章不变”原FAIL，发生1次翻译和1次纪要；该用例新建document并切换有效内容，重生成有上下文理由，不是候选12同ID纯拼写情形。[候选11及原失败](../evaluation/apple-candidate11-model-separator-2026-10-29.json)。

候选10：`17380f64783c26ce77def7dd13429699958f5259`，分支 `codex/release-0.3.5-candidate-10` 保持原位。296核心/7忽略、原有集成7项、专名集成8项、28 harness、严格Clippy及精确源码Windows-core/Linux/prepare/validate CI通过。**63/63原自动检查通过，源码/CLI/harness/模型/runtime身份前后相同**：原进程中断后保留19项完成结果，同身份独立目录补跑44项，不能称为一次不间断运行。三分片复核75份快照、144条字幕和142处引用；原62项全部可见正文与候选9一致，继承其措辞限制。新增正式CLI用例证明后补专名时旧转写、纪要和引用同步更正，保存命令期间新增生成任务为0。旧字幕缓存失效，尚未据此证明更正后的双语字幕。真实ASR、平台和安装升级门仍保留。[候选10结果](../evaluation/apple-candidate10-note-sync-2026-10-29.json)。

候选9 `2f6edac071aadbe3f9995e9fb248a9a8a174c456` 和分支 `codex/release-0.3.5-candidate-9` 保持原位；原62项完整结果、291核心/7忽略及27 harness记录不改写。[候选9结果](../evaluation/apple-candidate9-quality-2026-10-29.json)。

父检查9ffa的4项实际检查与8臂资源对照均通过；803的错误字幕被拦但重试失败，以及新增控制项的语言配置冲突，仍按原FAIL保留。只修正控制项的明确中文输出设置，不改原source/required/forbidden。9ffa Rust1.99 CI lint失败经等价?写法修正，未压制warning。候选6/7/8和所有中间失败不改写；后续共享源码修复须另立候选。

## 失败项—负责人—验证结果

| ID | 问题 | 负责人 | 固定来源、验证与下一步 |
| --- | --- | --- | --- |
| Q1 | 预算更正与未批准 | 主线程/共享纪要 | 候选9完整语料与逐阶段复核保留3000→5000、币种、未批准、独立同额发票和后续话题；受测范围通过。 |
| Q2 | 计划/完成、人物与动作对象 | 主线程/共享字幕和纪要 | 候选9精确保留刘老师/张老师；布置为setting up。长文第9条现明确today-only discussion、系统音频未完成和发布日期未定。实际提交/计划、页面/测试链接控制通过；既有其他字幕的用户修改/要保存/先等限定较弱和发布it指代歧义继续保留。 |
| Q3 | 星期擅加周次 | 主线程/共享验证 | 候选9未限定/本周/下周和绝对日期控制均保留各自范围；不多加周次，日期/星期同时保留。受测范围通过，不宣称任意跨任务日期均已证明。 |
| Q4/T1 | 录音对象、本机保留与源内关系 | 主线程/共享生成 | 候选9完整语料保留Friday否定、IN与本机，notes-only不添录音；未说刘老师仅在笔记，未进字幕/纪要。完整已识别字段可不启动模型，未知上下文、姓名、疑问或已有部分结果回普通链路；保持4片段/8请求原限额。 |
| Q5 | CER/WER/SER类型与后续独立事实 | 主线程/共享字幕和纪要 | 候选9完整指标控制与后续事实均通过；原数值/类型、缓存后文、否定证据保留。更广数值绑定不宣称已证明。 |
| Q6 | 大写字幕收尾成本 | 主线程/共享调度 | 候选9完整语料继续通过，五段核心事实与引用保留。父9ffa窄字幕配对中完全同文的闭合批次由1worker/2次生成变0/0，中位3.1205→0.5425秒；混合段仍用Metal。仅此范围，非整会/ASR/能耗结论。[配对结果](../evaluation/apple-closed-state-followup-2026-10-29.json)，[中间原失败](../evaluation/apple-caption-direct-trial-2026-10-29.json)。 |
| Q7 | 焦点词与姓名分类 | 主线程/术语 | 候选9保留原 **date** 控制与日期/星期；新增完整姓名+称谓提示须原文实际出现才保护，不据笔记添人/性别。显式词表优先、未说姓名和泛角色控制保留；不宣称任意中文实体识别完成。 |
| Q8 | 提示词或无依据限制写入 | 主线程/共享生成 | 候选9未再出现c6模板/复制禁令。完整来源字段严格消费、原子提交；未知内容回模型，保留原validation和revision/cancel guard。长空白跨640字节模型chunk已红→绿，避免给部分来源附整段译文。 |
| Q9 | 真实语音专名识别 | Windows/ASR，主线程协作 | 候选8同源码 CLI/ASR worker 对34.4秒 Realtek 录音重转写，Small目标词0/4、Bilingual仅Alice 1/4；词汇笔记不能证明每个目标词实际说出。原结果保留，未被候选9的合成文本通过覆盖。[本端报告与边界](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6474420928)。 |
| Q10 | 停录时序与完整收尾 | Windows/输入停止 | 候选8合成回放首次5000ms请求停止仅捕获4480ms，低于原4500ms门槛，保持失败。另一轮5056ms不解释首次差异，也不证明实体麦克风/UI停录可靠；保持原门槛，由Windows定位。[补充时序](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6474488061)。 |
| Q11 | 后补笔记专名后旧纪要不更新 | 主线程/共享术语和存储；三端复核 | 候选10通过正式save_note将实际模型已生成的CEDARLEDGER更正为CedarLedger，源和引用revision1→2、正文仅专名变化、未完成/未批准保持；保存命令inference_jobs1→1。原转写/文章历史保留；人工接受/忽略、原本过期和有歧义引用不自动覆盖，事务失败回滚正文+引用+历史。自动整理关闭的路径由集成用例覆盖。未解决任意音近错误、短未标记中文名；后续后台整理/字幕生成不能由本次note-only阶段证明。[证据](../evaluation/apple-candidate10-note-sync-2026-10-29.json)。 |
| Q12 | 源内完整型号的数字/分隔符误拒 | 主线程/共享字幕与纪要；Windows复核 | 候选11原5项定向和完整64项通过；实际MX二七五零→MX-2750不再被拒，字幕展示为MX2750。仅源内已知完整系列可等价；未知前缀、错数字、前导零丢失、额外后缀、独立发票借值和跨来源拼接继续拒绝。来源/笔记明确保护的连字符拼写保留，v30使旧自动结果重新检查。原41的4PASS/1FAIL与120秒超时、bb诊断、全部红测保留。共享热词输入另已修整词UTF-8预算及拒绝更新诊断，原source41实际8分支检查不计作候选11真实ASR准确率。原晚补专项的严格零后续生成测试保留FAIL：它换了document与有效内容，有重新整理的上下文理由；Q13单独验证同ID纯拼写情形，不改写原结果。[证据与边界](../evaluation/apple-candidate11-model-separator-2026-10-29.json)。 |
| Q13 | 纯拼写更正后的无必要纪要重写及含混批准措辞 | 主线程/共享术语、字幕与存储；三端最小复核 | 候选12同ID纯拼写用例通过真实自动调度：保存0新增推理，随后1次translation、0次minutes，文章/完整引用/旧纪要诊断保持同字节。旧fingerprint、完整覆盖、全部章节及来源身份必须先通过证明，ack与正文/引用/历史同事务；不推进cursor、不建立新增量seal。跨锁变化、事务失败和7类保护负例通过。实际a6原译“预算未通过”未满足精确批准谓词，原FAIL保留；在简单明确源句授权、避开条件/引述/撤回/保护名称后不增加请求地精确为“未获批准”，再过原校验。字幕cachev31，纪要fingerprint仍v30，不因字幕措辞升级重写文章。[完整65项及原失败](../evaluation/apple-candidate12-spelling-reuse-2026-10-29.json)。 |
| Q14 | 活跃专名优先级、普通词误改复数及“不完整”误拒 | 主线程/共享术语、字幕与纪要；三端复核 | 开发提交7470018的374核心/7忽略、2 CLI、21集成、严格Clippy和精确源码Windows-core/Linux/Website通过。原65项字节不变，追加3控制；6项实际本地自动调度定向PASS。原39c为5PASS/1FAIL，其中正确“不完整”被误拒，120.140秒/12请求/0提交；修复后同项6.881秒/2请求/2提交，原复数、未批准和引用保留。新回退仅接受逐来源有引用的闭合转录谓词，额外未完成任务、错引用、条件/引述及保护名称不放行；不改提示词、预算和模型。受测专名/纯拼写复用控制继续通过，尚不证明任意音近ASR或完整68项/安装验收。候选12不移动，没有发布0.3.5。[原失败与新结果](../evaluation/apple-term-priority-incomplete-state-2026-10-30.json)。 |
| G1-R | 损坏组件无法恢复 | 主线程/共享组件；Windows/Linux验收 | f946f64的296核心、8项模拟交互及Windows/Linux CI通过。Windows隔离原生DOM/invoke已验证确认取消保留、确认移除目标版本、签名候选12,674,455B重下载/校验及重装后测试才启用；首次缺helper超时保留，补同源helper后续测。未测已有active撤销/跨版本保留、实体鼠标和正式安装升级；Linux实机仍待验收。候选9不移动。[范围与证据](../evaluation/component-recovery-2026-10-29.json)。 |

“微检”、paper→“论文”、重复噪音文章的精炼继续登记为已知词义/可读性问题，不另开提示词调参矩阵。完整引用只证明出处匹配，不能证明正文保留了每个事实。

## 三端与发布门

| 平台/门 | 现有证据 | 尚未证明的范围 |
| --- | --- | --- |
| Mac/M系列 | 候选12同CLI/原installed runtime完整65项自动PASS、80快照复核，331核心/严格Clippy通过；第65项同现有document纯拼写更正后补齐rev2字幕而不新增纪要。原64可见内容仅明确源批准谓词的预期精确化。父8臂资源和候选10保存命令证据保留。 | festival未译、部分源限定/代词/含混词义仍公开。新复用只针对已证明的纯拼写变更，换内容/主题或新来源继续整理；不把该单例推广为连续实录或整会内存/能耗收益。真实麦克风、物理窗口及最终安装升级待验证。 |
| Windows | 候选8 GTX1650/621.01复用签名CUDA，3/3定向自动+独立语义/引用通过。Realtek预览定时停止/释放、DOM/invoke语言切换，以及受控CUDA子进程故障→CPU和独立CPU推理通过；三份peer报告字节hash与源码已核对。 | 另有同候选8源码的真实ASR专名和合成停录失败，见Q9/Q10。实际OOM、实体会议录音/停录、物理鼠标和安装升级未验收；不得把这些候选8结果当候选9同构建证明。[平台检查点](../evaluation/candidate8-platform-checkpoint-2026-10-29.json)。 |
| Windows/候选10 | 同源码17380f6的两份脱敏peer报告按固定45b885f/base64取回，字节hash均匹配：虚构late-note控制报告CUDA29/29，先有纪要再补笔记、auto_notes=false的同步通过，保存jobs1→1。 | 另一份真实ASR单片段重建的初始纪要被数字/日期校验拒绝、0篇提交；虽更正GTX1650，OmaScribe/NVIDIA仍未精确命中，不能算真实旧纪要同步通过。首次PATH/时间参数准备失败各自保留，未新录音/调用ASR；Mac仅核对peer身份，未复跑Windows。[分开记录的原证据](../evaluation/apple-candidate10-note-sync-2026-10-29.json)。 |
| Windows/候选11 | [同SHA定向反馈](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6494316987)报告16/16型号边界、2/2分隔符测试通过；既有两份私有被拒草稿在test-only隔离探针复验原validator通过。 | 未重跑ASR或模型、未生成/提交纪要；不能当真实生成、完整语义、实机停录或安装验收。真实Realtek尾音离线重转写的Alice/NVIDIA命中不证明OmaScribe/GTX1650或尾音标记，继续由Windows定位。 |
| Linux/Omarchy | 共享源码CI和GPU组件隔离测试已有证据；CUDA/Vulkan组件保持独立。 | 实际GPU/音频/原生窗口和候选安装升级仍待本端验收，不能由编译通过代替。 |
| 组件与模型复用 | 签名/hash/原子安装/回滚路径及组件污染负例已有代码与CI；基础包禁CUDA/Vulkan/HIP依赖。 | 新候选安装包及升级保留模型/组件流程仍需实测。没有为了本批语义修复重编SDK/组件或安装包。 |

## 交付节奏

- 主线程负责共享转写/翻译/纪要、事实校验、Mac Metal、集成和候选冻结；Windows负责本端输入/停止、冷ASR、CUDA与回退；Linux负责本端桌面/音频及Omarchy。共享问题在PR给最小差异，避免重复实现。
- 开发期只跑受影响检查，及时推送源码；已知共享失败未修好时不启动完整矩阵。候选冻结后，三端围绕同一SHA完成完整语料、真实录音、必要原生和安装升级验收；最后统一构建候选包。
- 复用已签GPU组件、模型和SDK；只有组件缺陷才重编/重签。Windows暂不重跑已知共享失败或追加完整CPU×GPU矩阵，等待主线程给新的固定SHA及最小范围。
- 候选11的[固定SHA与三端分工](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6494186963)已同步；共享完整64项和分片复核完成。Windows继续真实专名/停止，Linux继续本端音频/桌面/Omarchy；不重复编译SDK或已签GPU组件。补充字幕测试的原失败和观察结果分开保留，后续资源优化不扩成本次新模型矩阵。
- 候选12的[固定SHA与范围](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6496350429)已同步，完整65项及分片复核现已完成。两端围绕同SHA做专名/停止、本端音频/桌面、安装升级；共享改动没有更换ASR/LLM模型、提示词、SDK或已签组件。文档后续提交不移动冻结候选。
- 候选10已在[PR #1给出三端最小验证范围](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6487506753)：Windows继续真实专名/停止；Linux继续桌面/音频/Omarchy；均可复核“先有纪要、后补笔记专名”。PR #10原测试已整合。PR #11的c3d512e已修多段加粗/中文复选框提取，整合提交1c5853f通过前端构建、原Chrome组件回归、受影响Prettier/diff-check；仅人工辅助，实体鼠标/新版原生WebView仍待验收，候选10核心和旧冻结分支不移动。[整合说明](https://github.com/EazyLee30/omascribe/pull/11#issuecomment-6488233436)。Windows候选9的实际笔记/纪要检查保留[原说明](https://github.com/EazyLee30/omascribe/pull/1#issuecomment-6483517183)：用户确认说过NVIDIA，不能把它判成凭空添加；词汇命中也不能代替全文语义与实际ASR验收。
- PR #9已整合到be3：literal-only延迟启动、两个8秒工作预算及首次CPU回退缓存身份均共用现成实现；无需另写一遍。
- 只报“问题ID—负责人—SHA—结果—下一阻塞”，无新增阻塞可直接回报。原始证据保留，测试借用模型副本释放不等于同量磁盘空间回收。
- 发布时间等下一候选完整回归后再估算；0.3.5发布也不等于P0/P1/P2/P3/G1/G2全部研究结束。

## 原始证据

- [候选12完整65项、纯拼写复用与批准谓词、a6原FAIL](../evaluation/apple-candidate12-spelling-reuse-2026-10-29.json)。原64 fixture字节和对象不变；资源子门通过不覆盖a6整项FAIL，C11换document的原FAIL也独立保留。
- [候选11完整64项、型号与热词契约、晚补字幕严格原失败](../evaluation/apple-candidate11-model-separator-2026-10-29.json)。原41定向FAIL、bb隔离响应诊断和新版原5项PASS各自保留，不合并成同源码结果。
- [候选10专名同步与完整63项](../evaluation/apple-candidate10-note-sync-2026-10-29.json)，[候选9完整62项](../evaluation/apple-candidate9-quality-2026-10-29.json)，[9ffa状态闭环](../evaluation/apple-closed-state-followup-2026-10-29.json)，[803资源对照与原FAIL](../evaluation/apple-caption-direct-trial-2026-10-29.json)。[候选8原61项](../evaluation/apple-candidate8-quality-2026-10-29.json)、[1ff五项](../evaluation/apple-closed-retention-2026-10-28.json)、[c6误提交](../evaluation/apple-retention-context-rejection-2026-10-28.json)、[be3原失败](../evaluation/apple-retention-source-fields-2026-10-28.json)不改写。
- [a8 8项自动+3项独立纪要](../evaluation/apple-source-action-language-2026-10-28.json)，[841中间18项](../evaluation/apple-source-task-submission-trial-2026-10-28.json)。各来源分开，不拼接为同源总数。
- [Windows a8原报告](https://github.com/EazyLee30/omascribe/blob/7c33bb233cce830da0d7856bfad5f541842b473e/docs/evaluation/windows-a8f1058-targeted-2026-10-28.json)，[前一c8定向报告](https://github.com/EazyLee30/omascribe/blob/d5aa285a400f9fb867e2d74588ac7794bd58a021/docs/evaluation/windows-c8c1171-targeted-2026-10-28.json)。
- [候选7完整51项](../evaluation/apple-candidate7-quality-2026-10-28.json)，[候选6语义失败](../evaluation/candidate6-semantic-failures-2026-10-28.json)，[候选6回放](../evaluation/apple-candidate6-recording-2026-10-28.json)，[候选6公开ASR](../evaluation/apple-candidate6-asr-public-2026-10-28.json)。
- [候选5原始输出与字幕收尾](../evaluation/apple-candidate5-caption-finalization-2026-10-28.json)，[候选5回放](../evaluation/apple-candidate5-recording-2026-10-28.json)，[候选4及中间失败](../evaluation/apple-caption-source-spellings-2026-10-27.json)。更早证据仍在evaluation目录与本文件Git历史中。
