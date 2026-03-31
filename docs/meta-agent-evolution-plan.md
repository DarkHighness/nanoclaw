# Meta Agent 自我改造与自我进化实现计划

Date: 2026-03-31

Status: Active Research Plan

Literature Window: refreshed through 2026-03-31

Execution Companion:

- `docs/meta-agent-execution-plan.md`

## 1. 目标

本计划要解决的不是“再做一个多 Agent”，而是为 `nanoclaw` 构造一个
可持续改进自身行为的 Meta Agent 控制面。

这里的“自我改造 / 自我进化”按风险从低到高分四层：

1. 自优化
   - 调整 prompt、agent profile、skill 组合、模型路由、工具权限。
2. 工作流进化
   - 调整单 Agent / 多 Agent 的拓扑、分工、路由、反思回路、验证回路。
3. 混合节点进化
   - 把纯 LLM 节点替换成“LLM + 确定性代码 / verifier / tool node”的混合图。
4. 代码级自修改
   - 在隔离工作区内修改 `nanoclaw` 或项目代码，并通过评测后候选升级。

本计划不包含：

- 在线更新基础模型权重
- 无约束修改 sandbox / approval / security 默认值
- 直接在主工作区无审计地自写入和自推广

## 2. 一手外部参考

### 2.1 工业级项目

#### OpenAI Codex

- 官方文档：
  - <https://developers.openai.com/codex/subagents/>
  - <https://developers.openai.com/codex/agent-approvals-security/>
  - <https://developers.openai.com/codex/hooks/>
- 关键事实：
  - subagent 默认只在显式请求时 spawn
  - 子代理继承父级 sandbox / approval，并支持项目级自定义 agent 配置
  - 支持批量 fan-out/fan-in
  - hooks 被视为 agentic loop 的扩展点，但仍受安全边界约束
- 设计启发：
  - Meta Agent 必须是显式控制面，不应默认递归自繁殖
  - 自进化必须继承并收紧安全边界，不能绕过父级策略

#### Anthropic Claude Code

- 官方文档：
  - <https://docs.anthropic.com/en/docs/claude-code/sub-agents>
  - <https://docs.anthropic.com/en/docs/claude-code/hooks>
- 关键事实：
  - subagent 运行在独立 context window，可配置独立工具、权限、hooks、memory
  - 支持 `worktree` 隔离、background 运行、persistent memory
  - async hook 可在不阻塞主流程的情况下触发测试并把结果回灌后续 turn
- 设计启发：
  - 自我进化需要把“候选变体执行”和“主会话服务”严格隔离
  - hooks 很适合接 verifier、测试、policy gate，但必须 fail-closed

#### OpenCode

- 官方文档：
  - <https://opencode.ai/docs/agents/>
- 关键事实：
  - 区分 primary agent 与 subagent
  - `permission.task` 可以约束哪些 subagent 能被调用
  - `steps` 上限用于控制成本和防失控
- 设计启发：
  - Meta Agent 不只是生成候选，也要管理“谁能调用谁”
  - 进化系统必须内置预算和步骤上限，而不是只靠 prompt 自觉

#### LangGraph

- 官方文档：
  - <https://docs.langchain.com/oss/python/langgraph/workflows-agents>
- 关键事实：
  - 明确区分 workflow 与 agent
  - 官方直接给出 orchestrator-worker、parallelization、routing、
    evaluator-optimizer 等图式
- 设计启发：
  - 自进化对象不应只是 prompt 文本，而应是显式 workflow graph
  - evaluator-optimizer 是 Meta Agent 的最小闭环，不需要先上复杂进化算法

#### OpenHands

- 官方文档 / 官方仓库：
  - <https://docs.all-hands.dev/>
  - <https://docs.all-hands.dev/openhands/usage/runtimes/remote>
  - <https://github.com/All-Hands-AI/OpenHands>
  - <https://github.com/OpenHands/benchmarks>
- 关键事实：
  - 平台层同时提供 agent SDK、CLI、GUI、remote sandbox、benchmark infra
  - benchmark harness 支持 SWE-Bench、GAIA、Commit0、安全评测等
  - remote runtime 允许大规模并行评测
- 设计启发：
  - 工业级 agent 不是只有“执行器”，还必须自带评测与隔离运行环境
  - 自进化没有 benchmark harness，本质上就是不可控自改

#### AutoGen AgentOptimizer

- 官方文档：
  - <https://microsoft.github.io/autogen/0.2/docs/notebooks/agentchat_agentoptimizer/>
  - <https://microsoft.github.io/autogen/0.2/blog/2023/12/23/AgentOptimizer/>
- 关键事实：
  - 把 agent 的 function / skill list 视为可训练对象
  - 根据历史会话与满意度迭代添加、修订、删除函数
  - 内置 rollback 与 early-stop
- 设计启发：
  - 第一阶段应先从“可回滚的能力表 / skill 表优化”开始
  - 不必一开始就做代码级自修改

### 2.2 2026 年最新 ArXiv 文献

以下文献优先级高于 2024-2025 基础论文，因为它们直接回答了
“2026 年最新的 Meta Agent 应该怎么做”。

#### AgentFactory

- 论文：<https://arxiv.org/html/2603.18000v1>
- 核心结论：
  - 比起保存 textual reflection，更有效的是把成功子任务保存成可执行
    subagent code，并在后续任务里继续修改、复用、部署。
- 对应实现要求：
  - `nanoclaw` 不应只存 reflection text，还应存 `executable skill/subagent`
  - Meta Agent 应优先演化“可复用技能单元”，不是每次重写整条链路

#### Meta Context Engineering

- 论文：<https://arxiv.org/html/2601.21557>
- 核心结论：
  - context engineering 本身可以成为一个双层优化问题：
    meta-agent 进化 skill，base-agent 用这些 skill 生成和维护 context artifact。
- 对应实现要求：
  - 计划中必须区分：
    - skill evolution
    - context artifact optimization
  - memory / context 不应被视为单一黑盒

#### Scaling Agentic Verifier for Competitive Coding

- 论文：<https://arxiv.org/abs/2602.04254>
- 核心结论：
  - verifier 不应只是被动评分器；更强做法是主动构造可区分输入与反例，
    以暴露候选之间的行为差异。
- 对应实现要求：
  - `crates/evals` 不应只包含 `pass/fail` evaluator
  - 应预留 `active verifier`，能主动生成 counterexample / adversarial case

#### Verified Multi-Agent Orchestration

- 论文：<https://arxiv.org/abs/2603.11445>
- 核心结论：
  - plan-execute-verify-replan 的 verification-driven loop，
    可以作为编排层的正式控制信号，而不只是末端 QA。
- 对应实现要求：
  - verifier 结果要能反向驱动 replanning
  - workflow IR 里应把 `Verify` / `Replan` 视为一等节点

#### Group-Evolving Agents

- 论文：<https://arxiv.org/html/2602.04837v1>
- 核心结论：
  - 组级经验共享比单 agent 独立 lineage 更有效；
    多样性只有能被显式复用，才会转化成长期进化收益。
- 对应实现要求：
  - 后期 archive 不应只是单链 lineage
  - 应支持 group / island 级经验复用与跨 lineage patch 迁移

#### HyEvo

- 论文：<https://arxiv.org/html/2603.19639v1>
- 核心结论：
  - 纯 LLM workflow 不够经济；最新趋势是异构节点、反思生成、分层快筛。
- 对应实现要求：
  - 要把 CodeNode / VerifierNode 设计成正式执行节点
  - 要有 cascaded sandbox evaluation

### 2.3 基础 ArXiv 文献

#### Reflexion

- 论文：<https://arxiv.org/abs/2303.11366>
- 核心结论：
  - agent 可以不改权重，而通过语言化反思写入 episodic memory，
    再在后续试次中改进行为。
- 对应实现要求：
  - 需要显式的失败反馈抽取
  - 需要结构化 reflection memory，而不是把“经验”混在普通 transcript 里

#### Voyager

- 论文：<https://arxiv.org/abs/2305.16291>
- 核心结论：
  - 自动 curriculum、可复用 skill library、基于执行反馈与自验证的迭代
    prompting，能形成长期能力积累。
- 对应实现要求：
  - Meta Agent 要维护可检索 skill / procedure 库
  - 每次改进都要变成可复用产物，而不是一次性临时上下文

#### ADAS

- 论文：<https://arxiv.org/abs/2408.08435>
- 核心结论：
  - agent 可以在代码空间中自动发现更强的 agent 设计；
    “meta agent 编写更好的 agent”是可行方向。
- 对应实现要求：
  - workflow / prompt / tool policy 必须具备代码化表示
  - 需要 archive、候选 lineage、跨任务迁移评测

#### AFlow

- 论文：<https://arxiv.org/abs/2410.10762>
- 核心结论：
  - 把 workflow 优化表述为代码表示上的搜索问题是有效的；
    执行反馈与树状经验对搜索很关键。
- 对应实现要求：
  - `nanoclaw` 需要 workflow IR，而不只是临时 prompt 编排
  - 需要 candidate execution log 与搜索经验缓存

#### OpenHands

- 论文：<https://arxiv.org/abs/2407.16741>
- 核心结论：
  - 软件工程 agent 平台需要 sandbox、代码执行、web、multi-agent、
    benchmark 共同构成闭环。
- 对应实现要求：
  - Meta Agent 不能脱离真实运行环境做“纸上优化”

#### SWE-Gym

- 论文：<https://arxiv.org/abs/2412.21139>
- 核心结论：
  - 软件工程 agent 的提升依赖真实代码库、真实运行环境、真实单测，
    以及 verifier。
- 对应实现要求：
  - 必须有独立 verifier / evaluator 层
  - 必须把“解决率 / 回归率 / 成本 / 延迟”作为联合目标

#### SEW

- 论文：<https://arxiv.org/abs/2505.18646>
- 核心结论：
  - coding workflow 的 topology 与 prompt 都可以自进化，不必人工手写。
- 对应实现要求：
  - workflow 变体与 prompt 变体要统一进入 experiment archive

#### AlphaEvolve

- 论文：<https://arxiv.org/abs/2506.13131>
- 核心结论：
  - evolutionary coding agent 在多个 evaluator 持续反馈下，可以稳定优化
    算法与代码，并得到可验证改进。
- 对应实现要求：
  - 代码级自演化必须依赖多 evaluator，而不是单一 LLM 自评

## 3. 归纳后的核心结论

把上面的项目与论文放在一起看，Meta Agent 的工业可行路径非常明确：

1. 先把改进对象显式化
   - prompt、skill、policy、workflow graph、code patch 都要成为一等对象。
2. 再把反馈闭环结构化
   - 成败、成本、延迟、安全事件、人工拒绝都要可度量。
3. 再把改进执行隔离化
   - 候选变体必须在独立 worktree / sandbox / remote runtime 中运行。
4. 最后才做自动推广
   - 没有 verifier、archive、rollback 的自修改，不是进化，是破坏。

截至 2026-03-31，最新文献还把路线进一步收紧为三条新增约束：

1. 从 textual memory 升级到 executable skill / subagent accumulation
   - 来自 AgentFactory、MCE
2. 从被动 evaluator 升级到主动 verifier
   - 来自 Scaling Agentic Verifier、VMAO
3. 从单 agent lineage 升级到组级经验共享
   - 来自 Group-Evolving Agents、HyEvo

因此，`nanoclaw` 的计划不能停留在：

- 只做 reflection memory
- 只做静态 benchmark 打分
- 只做单条候选链条

而应明确转向：

- executable skills
- active verifiers
- archive + island/group evolution

换句话说，Meta Agent 不是一个“更聪明的 planner”。
它本质上是一个带实验平台的 agent compiler + evaluator + promoter。

## 4. 面向 `nanoclaw` 的目标架构

```text
User-facing Agent Plane
    │
    ├── Normal runtime turns
    └── Task/subagent execution
            │
            ▼
Observation Plane
    ├── Session events
    ├── Agent envelopes
    ├── Token/cost/latency
    ├── Artifacts
    └── Memory exports
            │
            ▼
Meta Agent Plane
    ├── Critic
    ├── Candidate generator
    ├── Workflow optimizer
    ├── Verifier orchestrator
    └── Archive / lineage manager
            │
            ▼
Promotion Plane
    ├── Safety gate
    ├── Regression gate
    ├── Human approval gate
    └── Rollback / pinning
```

Meta Agent 的最小闭环应为：

```text
Observe
  -> Diagnose
  -> Generate candidate
  -> Sandbox run
  -> Evaluate
  -> Compare to baseline
  -> Promote or reject
  -> Write reflection + lineage
```

## 5. 当前仓库基础与缺口

### 5.1 已具备的基础能力

从当前代码看，`nanoclaw` 已经不是从零开始：

- 多 Agent 基础已经有：
  - `crates/tools/src/agentic/task.rs`
  - `crates/runtime/src/subagent_impl.rs`
  - `crates/runtime/src/agent_session_manager.rs`
  - `crates/runtime/src/agent_mailbox.rs`
- 冲突控制已经有：
  - `crates/runtime/src/write_lease.rs`
- 事件与产物基础已经有：
  - `crates/types/src/event.rs`
  - 已有 `AgentEnvelope`、`AgentArtifact`、`AgentResultEnvelope`
- 会话持久化与导出已经有：
  - `crates/store/src/traits.rs`
  - `crates/memory/src/runtime_exports.rs`
- hook 与安全边界已经有：
  - `crates/runtime/src/hooks/*`
  - approval / sandbox / execution policy 都已有 runtime 边界

这意味着：

- 观测面已经有雏形
- 子代理协调面已经有雏形
- memory/export 面已经可复用

### 5.2 关键缺口

但要走到 Meta Agent，还缺几个决定性模块：

#### 缺口 A：没有 evaluator / verifier substrate

现状：

- 仓库里没有独立的 benchmark / verifier / fitness substrate
- 也没有“baseline vs candidate”统一评测协议

后果：

- agent 只能“执行”，不能“被系统性比较”

#### 缺口 B：没有 experiment archive 与 lineage

现状：

- session event 能记历史
- 但没有“候选变体、实验配置、评测结果、推广决策”的专门模型

后果：

- 无法做回滚、Pareto 排序、跨任务迁移分析

#### 缺口 C：agent hook 还没有真正落地

现状：

- `crates/runtime/src/hooks/handlers/agent.rs`
  目前仍是 fail-closed stub

后果：

- Meta Agent 还不能作为正式的 runtime control-plane extension 接入

#### 缺口 D：没有 workflow IR

现状：

- 当前多 Agent 更偏“任务委派协议”
- 还没有显式的 workflow graph / node / edge / evaluator loop 表达

后果：

- 无法把 AFlow / SEW / HyEvo 这类方法自然映射进来

#### 缺口 E：没有隔离的自修改执行面

现状：

- 当前已有 sandbox 和写租约
- 但没有专门面向“候选自修改”的 worktree / branch / experiment workspace

后果：

- 代码级自进化很难安全上线

## 6. 建议的分层对象模型

Meta Agent 不要直接优化“整个 runtime”，而应该优化以下五类对象：

### 6.1 `PromptVariant`

- 目标：
  - 改写 system prompt、developer instructions、review rubric、reflection rubric
- 风险：
  - 最低
- 适合作为：
  - 第一阶段 MVP

### 6.2 `SkillVariant`

- 目标：
  - 调整 skill 组合、加载顺序、项目级 skill 内容
- 风险：
  - 低
- 来源：
  - Voyager、AgentOptimizer

### 6.3 `PolicyVariant`

- 目标：
  - 优化模型路由、reasoning effort、工具 allow/deny、task permission
- 风险：
  - 中
- 约束：
  - 不允许自动放宽 sandbox / approval 默认值

### 6.4 `WorkflowVariant`

- 目标：
  - 优化 agent graph：并行、路由、评审回路、evaluator-optimizer 回路
- 风险：
  - 中高
- 来源：
  - LangGraph、AFlow、SEW

### 6.5 `CodePatchVariant`

- 目标：
  - 修改 agent runtime、hook、tooling、workflow executor 代码
- 风险：
  - 最高
- 来源：
  - ADAS、AlphaEvolve、HyEvo

## 7. 实现策略

### 7.1 先做“离线自优化”，后做“在线自进化”

推荐顺序：

1. 离线 replay + benchmark 改进 prompt / skill / policy
2. 离线 workflow search
3. 隔离 worktree 中的代码级 patch search
4. 有足够 guardrail 后，再允许有限在线触发

不推荐：

- 直接在 live user session 内对自身主逻辑做在线改写

### 7.2 先做“evaluator-optimizer”，再做“evolutionary search”

第一批可落地能力不需要复杂演化算法：

- baseline
- candidate generator
- evaluator
- comparator
- promotion gate

这套最小闭环成熟后，再叠加：

- archive
- diversity search
- multi-island evolution
- cross-task transfer

### 7.3 先做“配置与 workflow 进化”，再做“代码进化”

原因很简单：

- PromptVariant / SkillVariant / PolicyVariant 可回滚、可比对、低风险
- WorkflowVariant 次之
- CodePatchVariant 风险最高，应当最后进入自动化

## 8. 分阶段落地路线

## Phase 0：观测与实验账本

目标：

- 把“进化”所需数据面补齐

建议改动：

- 在 `crates/types` 增加实验实体
  - `ExperimentId`
  - `CandidateId`
  - `BaselineId`
  - `PromotionDecision`
- 在 `crates/types/src/event.rs` 增加事件
  - `ExperimentStarted`
  - `CandidateGenerated`
  - `CandidateEvaluated`
  - `CandidatePromoted`
  - `CandidateRejected`
- 在 `crates/store` 持久化
  - candidate config
  - evaluator results
  - lineage
  - rollback pointer

完成标志：

- 任一候选改进都可重放、可比对、可追溯

不做这一步的代价：

- 后续所有“进化”都无法审计

## Phase 1：Verifier / Evaluator Substrate

目标：

- 为 Meta Agent 提供独立评分层

建议新增：

- `crates/evals`

核心 trait：

```rust
pub trait Evaluator {
    async fn evaluate(&self, candidate: CandidateRef) -> EvalResult;
}
```

第一批 evaluator：

- `CommandExitEvaluator`
  - 跑命令，看退出码
- `TestSuiteEvaluator`
  - 跑单测 / 集成测试
- `OutputSchemaEvaluator`
  - 检查结构化输出格式
- `DiffPolicyEvaluator`
  - 检查是否修改了受限路径
- `SafetyEvaluator`
  - 检查网络、权限、敏感文件触达
- `CostLatencyEvaluator`
  - 汇总 token / wall-clock / tool count

第二批 verifier：

- `CounterexampleVerifier`
  - 主动构造输入，放大候选差异
- `BehaviorDiffVerifier`
  - 对 baseline 与 candidate 做行为分歧挖掘
- `ReplanTriggerVerifier`
  - 在 workflow 执行中触发 verify-replan

第一批 benchmark：

- 仓库内固定任务集
  - 文档任务
  - 代码审查任务
  - 小型 bugfix 任务
  - 只读研究任务
- 后续再接：
  - HumanEval / MBPP 风格任务
  - SWE-Gym / SWE-Bench 风格任务

完成标志：

- 候选变体可得到统一分数：
  - quality
  - safety
  - cost
  - latency
  - regressions

## Phase 2：Prompt / Skill / Policy 自优化 MVP

目标：

- 构造最低风险 Meta Agent MVP

建议新增：

- `crates/meta`
  - `critic.rs`
  - `candidate.rs`
  - `optimizer.rs`
  - `promotion.rs`
- `apps/code-agent`
  - `/improve`
  - `/experiments`
  - `/promote`

执行流：

1. 从最近 N 个 session 与 benchmark 选取失败样本
2. Critic 生成 failure taxonomy
3. Candidate generator 产出 PromptVariant / SkillVariant / PolicyVariant
4. Evaluator 跑基线与候选
5. Promotion gate 决定是否升级
6. 把 reflection 写入 memory export 与 experiment archive

这里建议同时借鉴 AgentOptimizer 与 2026 年 AgentFactory / MCE：

- 支持 add / revise / remove skill
- 支持 early-stop
- 支持 rollback
- 支持把稳定能力沉淀成 executable skill / subagent，而不只是文本摘要

完成标志：

- 系统能稳定优化 agent profile 与 skill 组合
- 不修改 runtime 核心代码

## Phase 3：把 Workflow 变成显式 IR

目标：

- 让进化对象从“prompt 文本”升级为“可执行图”

建议新增：

- `crates/meta/src/workflow_ir.rs`

最小 IR 节点类型：

- `PromptNode`
- `ToolNode`
- `SubagentNode`
- `RouterNode`
- `ParallelMapNode`
- `JoinNode`
- `EvaluatorNode`
- `RetryNode`
- `CodeNode`

最小图模式：

- orchestrator-worker
- evaluator-optimizer
- review-revise
- route-specialist
- parallel-audit

实现策略：

- 复用 `task` / `agent_spawn` / `agent_wait` 作为执行后端
- 先手工模板化，再允许 LLM 变异图结构

完成标志：

- workflow 可序列化、可执行、可评测、可比较

## Phase 4：Workflow Search

目标：

- 让 Meta Agent 能改“结构”而不仅改“话术”

搜索策略建议按难度递进：

1. 模板参数搜索
   - 选择哪种拓扑
   - 是否启用 reviewer
   - 并行度
   - retries
2. 局部图变异
   - 插入 evaluator node
   - 插入 reflection node
   - 替换 router prompt
3. 全图生成
   - 参考 AFlow / SEW / ADAS

推荐初始算法：

- beam search 或 best-first search

不推荐一开始就上：

- 大规模开放式代码搜索

完成标志：

- 对同一任务集，workflow candidate 能系统性优于静态手写流程

## Phase 5：混合节点与分层评测

目标：

- 按 HyEvo 路线，把“可确定的工作”从 LLM 推理迁到代码 / verifier 节点

应优先引入的 `CodeNode`：

- JSON / schema 校验
- 文件路径与 diff 规则检查
- 测试结果解析
- 搜索结果去重与排序
- 基于日志的失败模式分类

推荐评测顺序：

1. 快筛
   - schema / policy / syntax / smoke test
2. 中筛
   - 小样本 benchmark
3. 全量
   - 全测试 / 全任务集 / 回归集

这里应明确吸收 2026 verifier 文献：

- verifier 不只做末端打分
- verifier 可以在候选之间主动制造分歧案例
- verifier 结果可以触发局部 replanning，而不是只给最终 verdict

完成标志：

- 候选生成成本显著下降
- verifier 通过率显著提高

## Phase 6：隔离工作区中的代码级自修改

目标：

- 允许 Meta Agent 在隔离环境里修改自身代码或宿主项目代码

建议新增：

- `WorktreeCandidateRunner`
- `BranchPromotionGate`
- `RegressionPack`

运行原则：

- 每个 candidate 在独立 git worktree 或临时 clone 中执行
- 默认只允许修改声明过的 write set
- 所有 patch 必须经过 evaluator matrix
- promotion 默认要求人工确认

promotion gate 至少检查：

- 全部关键 evaluator 通过
- 无安全策略回退
- 无 protected path 违规修改
- 成本 / 延迟没有越过上限
- 回归率不高于基线

完成标志：

- 系统能以 PR-like 方式产出自修改候选，而非直接覆盖主线

## Phase 7：Archive、Pareto Front 与 Multi-Island Evolution

目标：

- 从“单候选优化”走向“持续种群进化”

建议引入：

- archive
  - 保存优胜候选与多样性候选
- lineage graph
  - 记录谁由谁变异而来
- Pareto ranking
  - 质量 / 成本 / 延迟 / 安全联合排序
- island model
  - 不同任务域维护不同子种群
- group evolution
  - 同一轮允许多个 agent / workflow 共享经验与 patch 方向

为何要最后做：

- 这一步只有在前 6 步已经稳定后才有价值
- 否则只是更快地产生不可控候选

但这里的实现形态应优先参考 2026 文献，而不是传统单 parent 演化：

- 支持 group-level reflection
- 支持跨 lineage patch / skill 迁移
- 支持以 workflow/tool usage 为主的模型无关改进

## 9. 推荐的数据与记忆分层

### 9.1 Transcript

- 保留原始执行过程
- 不直接承载长期演化知识

### 9.2 Reflection Memory

- 存失败模式、修复策略、已知坑
- 来源：
  - Reflexion
  - Voyager

### 9.3 Skill Library

- 存可复用 prompt、procedure、工具链套路

### 9.4 Experiment Archive

- 存候选、评分、对比、lineage、推广决策

### 9.5 Benchmark Corpus

- 存固定任务集与回归集

这五层不能混在一个抽象里。
尤其 `reflection memory` 与 `experiment archive` 必须分开：

- 前者服务推理
- 后者服务进化治理

## 10. 安全与治理原则

### 10.1 默认只允许向内收紧，不允许自动向外放权

允许自动优化：

- prompt
- skill
- model routing
- workflow topology
- read-only / tighter tool restrictions

不允许自动优化：

- 放宽 sandbox
- 放宽 approval
- 放宽网络权限
- 关闭关键 verifier

### 10.2 所有推广都必须可回滚

至少需要：

- baseline pin
- candidate pin
- one-click rollback
- lineage 记录

### 10.3 先 benchmark 过，再进入 live traffic

推荐顺序：

```text
offline replay
  -> benchmark pack
  -> shadow mode
  -> limited live traffic
  -> default promotion
```

### 10.4 把“拒绝升级”当成正常结果

多数候选应被淘汰。
如果候选几乎都被接受，说明 evaluator 太弱。

## 11. 建议的仓库落点

### 11.1 新增 crate

- `crates/evals`
  - evaluator trait、benchmark adapters、result schema
- `crates/meta`
  - critic、candidate generator、workflow search、promotion gate

### 11.2 重点改造现有模块

- `crates/types/src/event.rs`
  - 增加实验事件
- `crates/store/src/traits.rs`
  - 增加 experiment archive
- `crates/memory/src/runtime_exports.rs`
  - 导出 reflection / experiment summary
- `crates/runtime/src/hooks/handlers/agent.rs`
  - 从 fail-closed stub 升级为受控 meta-agent evaluator
- `crates/runtime/src/subagent_impl.rs`
  - 作为 workflow runtime backend 继续复用
- `apps/code-agent/src/backend/*`
  - 增加 experiment / promotion / benchmark 操作入口

## 12. 建议的里程碑顺序

### Milestone A

- Phase 0 + Phase 1
- 结果：
  - 有统一实验账本
  - 有 evaluator substrate

### Milestone B

- Phase 2
- 结果：
  - 有 Prompt / Skill / Policy 自优化 MVP

### Milestone C

- Phase 3 + Phase 4
- 结果：
  - 有 workflow IR 与 workflow search

### Milestone D

- Phase 5
- 结果：
  - 有 HyEvo 风格混合节点与分层评测

### Milestone E

- Phase 6 + Phase 7
- 结果：
  - 有隔离代码级自修改与受控进化种群

## 13. 我对 `nanoclaw` 的明确建议

如果目标是“尽快做出可工作的 Meta Agent”，推荐路线是：

1. 不要直接实现全自动代码自进化
2. 先把 `PromptVariant + SkillVariant + PolicyVariant` 做通
3. 同时补 `crates/evals`
4. 再把 workflow graph 显式化
5. 最后才进入 worktree 内的代码 patch 搜索

理由：

- 当前仓库已经具备 subagent、event、store、memory、write lease 基础
- 最缺的是 evaluator、archive、workflow IR、promotion gate
- 这些补齐后，Meta Agent 才从“想法”变成“系统”

## 14. 下一步执行清单

建议按以下顺序开工：

1. 建 `crates/evals`，先把统一 `EvalResult` 与 4 个基础 evaluator 做出来。
2. 在 `crates/types/src/event.rs` 与 `crates/store` 增加 experiment/candidate 事件与持久化。
3. 做 Phase 2 MVP：
   - 只优化 prompt / skill / policy
   - 只跑离线 benchmark
   - 必须支持 rollback
4. 设计 `workflow_ir.rs`，用现有 subagent executor 做执行后端。
5. 实现 `agent` hook evaluator，把 Meta Agent 作为 runtime 正式扩展点接入。
6. 最后再做 worktree candidate runner 和代码级 promotion gate。

## 15. 简短结论

工业级 Meta Agent 的正确实现路径，不是“让 agent 自己多想一点”，而是：

- 把改进对象代码化
- 把反馈闭环结构化
- 把候选执行隔离化
- 把推广决策制度化

对 `nanoclaw` 来说，最现实的路线是：

`subagent substrate` -> `evaluator substrate` -> `prompt/skill/policy 自优化`
-> `workflow IR + search` -> `active verifier + hybrid nodes`
-> `isolated code evolution` -> `group/island evolution`

这条路线和当前仓库基础是对齐的，也和现有工业项目与最新论文的共识一致。
