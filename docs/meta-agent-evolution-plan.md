# Nanoclaw 自改进研究计划

Date: 2026-03-31

Status: Active Research Plan

Literature Window: refreshed through 2026-03-31

Execution Companion:

- `docs/meta-agent-execution-plan.md`

## 1. 目标重述

这份文档要回答的不是：

- 如何做一个更通用的 `self-improving code agent`

而是：

- 如何让 `nanoclaw` 在运行过程中，持续改进 `nanoclaw` 自身

这里必须先纠正两个常见误解：

### 1.1 目标不是“通用 code agent 训练平台”

你要的不是一个面向任意仓库的通用改进系统。  
你要的是一个：

- self-hosted
- runtime-coupled
- self-referential
- nanoclaw-specific

的自改进系统。

也就是说，优化对象不是抽象 agent，而是：

- `nanoclaw` 的 prompt
- `nanoclaw` 的 skills
- `nanoclaw` 的 workflow / routing
- `nanoclaw` 的 hook / verifier recipe
- `nanoclaw` 的 runtime code

### 1.2 “运行过程中 self-improving” 不等于在线热改自己

工程上正确的理解应当是：

- `nanoclaw` 在前台继续正常服务用户
- `nanoclaw` 在后台持续观察自身运行
- `nanoclaw` 在隔离环境里评估自修改候选
- `nanoclaw` 通过版本化推广逐步采用改进

而不是：

- 在当前活跃 turn 内直接修改当前运行时并立刻切换

因此，本计划的核心原则是：

- **前台运行回路**
- **后台自改进回路**

必须分离。

## 2. 外部一手参考

### 2.1 工业级实现

#### Claude Code

- 官方文档：
  - <https://code.claude.com/docs/en/sub-agents>
  - <https://code.claude.com/docs/en/hooks>
- 关键事实：
  - sub-agent 具备独立权限、独立上下文和独立工具面
  - hooks 可以把 verifier、tests、policy gate 接入 runtime 控制流
  - `WorktreeCreate` 等 hook 说明隔离执行是正式能力，不是外围脚本
- 设计启发：
  - `nanoclaw` 的自改进候选必须走隔离执行面
  - verifier 应进入 runtime 控制面，而不是只写日志

#### OpenAI Codex

- 官方文档：
  - <https://developers.openai.com/codex/subagents/>
  - <https://developers.openai.com/codex/agent-approvals-security/>
  - <https://developers.openai.com/codex/hooks/>
- 关键事实：
  - subagent、sandbox、approval、hooks 都是显式控制面
  - patch workflow、受控写入和安全边界优先级高于“递归自改”
- 设计启发：
  - `nanoclaw` 的自改进不能绕开 approval / sandbox / protected path
  - promotion 必须是审计友好的版本切换

#### OpenHands

- 官方文档 / 仓库：
  - <https://docs.openhands.dev/openhands/usage/developers/evaluation-harness>
  - <https://docs.openhands.dev/openhands/usage/runtimes/remote>
  - <https://github.com/OpenHands/benchmarks>
- 关键事实：
  - benchmark harness、isolated workspace、远程并行评测是同一套工程体系
  - evaluation harness 以 workflow 和 benchmark 为入口，而不是 ad-hoc 单次实验
- 设计启发：
  - `nanoclaw` 的自改进必须有正式 verifier / replay substrate
  - 没有隔离评测环境，自改进就不可控

### 2.2 与“自改进自身”直接相关的论文

#### A Self-Improving Coding Agent

- 论文：<https://arxiv.org/abs/2504.15228>
- 核心结论：
  - coding agent 可以通过自主编辑自身并在后续评测中提升表现。
- 对应实现要求：
  - `nanoclaw` 的自改进不能永远停留在配置层
  - 必须允许受控的 self-edit path

#### Automated Design of Agentic Systems (ADAS)

- 论文：<https://arxiv.org/abs/2408.08435>
- 核心结论：
  - 把 agent 表示成代码对象后，meta-level agent 可以自动发现更好的 agent 设计。
- 对应实现要求：
  - `nanoclaw` 的 prompt / workflow / routing / hooks / code 都要具备代码化表示
  - 版本化 artifact 是必要前提

#### AgentFactory

- 论文：<https://arxiv.org/abs/2603.18000>
- 核心结论：
  - 成功经验更应沉淀为可执行 subagent code，而不是文本反思。
- 对应实现要求：
  - `nanoclaw` 的成功修复不该只写 archive，而应沉淀成 executable artifact

#### Meta Context Engineering

- 论文：<https://arxiv.org/abs/2601.21557>
- 核心结论：
  - meta-level agent 优化 skill，base-level agent 生成和维护 context artifact；
    这是双层优化问题。
- 对应实现要求：
  - 必须区分：
    - 自改进控制器
    - 正常服务 runtime
  - 不能把“改自己”和“执行用户任务”混成一个黑盒

#### Scaling Agentic Verifier for Competitive Coding

- 论文：<https://arxiv.org/abs/2602.04254>
- 核心结论：
  - verifier 不只做被动打分，更应主动暴露候选之间的行为差异。
- 对应实现要求：
  - `nanoclaw` 的 verifier 必须能做 replay diff 和 counterexample search

#### Verified Multi-Agent Orchestration

- 论文：<https://arxiv.org/abs/2603.11445>
- 核心结论：
  - verify 应是 orchestration level 的控制信号，而不是流程末端 QA。
- 对应实现要求：
  - verifier 输出必须回流到自改进控制器
  - verification-driven replan 应作为正式能力设计

## 3. 研究归纳后的核心结论

### 3.1 系统中心必须从“通用任务”改成“nanoclaw 自身运行证据”

这里最重要的数据源，不应是外部 benchmark，而应是：

- 失败 turn
- 人工纠正
- 高成本 / 高延迟 turn
- 错误工具调用
- 错误 subagent 路由
- hook / verifier 拦截结果
- 真实修复 patch

因此，本系统中心对象不应优先是泛化 `TaskSuite`，而应优先是：

- `SelfImproveSignal`
- `SelfImproveTask`
- `NanoclawRegressionCorpus`
- `NanoclawArtifactVersion`

### 3.2 `nanoclaw-self-gym` 仍然需要，但它应从 runtime signal 长出来

我之前强调 `TaskGym-first` 并非完全错误，但对你的目标来说，
它必须内化成：

- `nanoclaw-self-gym`

也就是：

- 来源于 `nanoclaw` 自己的运行历史
- 面向 `nanoclaw` 自己的 replay / regression / repair
- 用于验证 `nanoclaw` 自己是否变好

### 3.3 verifier 在这里是“防自毁边界”

通用 code agent 的 verifier 更多是在判断“是否更强”。  
`nanoclaw` 自改进的 verifier 还必须负责：

- 防止放宽 sandbox / approval
- 防止 protected path 被破坏
- 防止 replay 退化
- 防止工具行为变坏
- 防止 hook policy 漂移

所以这里的 verifier 不是普通评分器，而是：

- quality gate
- safety gate
- behavior regression firewall

### 3.4 archive 的真正终点是 `artifact version ledger`

对 `nanoclaw self-improvement` 来说，真正要积累的不是：

- 文本反思
- 单次 experiment 记录

而是：

- prompt artifact versions
- skill artifact versions
- workflow artifact versions
- hook/verifier artifact versions
- runtime patch artifact versions

archive 应当围绕：

- baseline
- candidate
- verifier bundle
- promotion decision
- rollback pointer

来设计。

### 3.5 runtime code self-edit 是终点，但不能被永久后置

Prompt / Skill / Workflow 层是低风险起步点。  
但如果目标真是 `nanoclaw` 改进 `nanoclaw`，那么最终必须进入：

- runtime code self-edit

否则系统始终只是：

- 优化 nanoclaw 的“外层配置”

而不是真正优化：

- `nanoclaw` 本体

## 4. 对当前仓库的直接含义

从当前仓库结构看，已有基础能力并不少：

- 事件与历史：
  - `crates/runtime/src/runtime/event_log.rs`
  - `apps/code-agent/src/backend/events.rs`
  - `apps/code-agent/src/backend/session_history.rs`
- runtime export：
  - `crates/memory/src/runtime_exports.rs`
- 子代理执行：
  - `crates/runtime/src/subagent_impl.rs`
  - `crates/runtime/src/agent_session_manager.rs`
- hook 安全边界：
  - `crates/runtime/src/hooks/handlers/agent.rs`
  - 目前为 fail-closed stub
- 持久化与归档：
  - `crates/store/*`
  - `crates/meta/*`

这意味着仓库已经有：

- 观测面
- 子代理面
- runtime 边界
- 持久化面

但还缺：

1. `nanoclaw` 自身 signal mining
2. `nanoclaw` 自回归语料
3. `nanoclaw` artifact versioning
4. 隔离 self-edit runner
5. `nanoclaw` 专用 verifier bundle
6. 后台自改进控制器

## 5. 正确的目标架构

```text
Nanoclaw Runtime Plane
    ├── user-facing turns
    ├── subagent execution
    ├── approvals / sandbox / hooks
    └── transcripts / tool traces / event logs
            │
            ▼
Observation Plane
    ├── failure mining
    ├── correction mining
    ├── cost / latency anomaly mining
    ├── hook / verifier denials
    └── runtime export snapshots
            │
            ▼
Self-Improvement Plane
    ├── signal queue
    ├── task miner
    ├── self-regression corpus
    ├── candidate artifact generator
    ├── isolated self-edit runner
    └── nanoclaw-specific verifiers
            │
            ▼
Promotion Plane
    ├── versioned artifact ledger
    ├── operator review
    ├── staged rollout
    └── rollback
```

最小正确闭环应是：

```text
observe nanoclaw runtime
  -> derive self-improvement signals
  -> build self-improvement tasks
  -> pin current nanoclaw artifact version
  -> produce candidate artifact version
  -> run candidate in isolated worktree
  -> replay nanoclaw regressions and runtime-derived cases
  -> run safety / behavior / policy verifiers
  -> promote or reject candidate version
  -> keep lineage and rollback pointer
```

## 6. 需要显式建模的对象

### 6.1 `SelfImproveSignal`

- runtime 中可被聚合的改进信号
- 来源：
  - errors
  - retries
  - denials
  - human corrections
  - cost anomalies

### 6.2 `SelfImproveTask`

- 由 signal 提炼出的、针对 `nanoclaw` 自身的改进任务

### 6.3 `NanoclawArtifactVersion`

- 版本化的 prompt / skill / workflow / hook / verifier / runtime code

### 6.4 `NanoclawRegressionCorpus`

- 来源于 `nanoclaw` 自身运行历史的 replay / regression / validation cases

### 6.5 `NanoclawVerifierReport`

- 包含：
  - quality findings
  - behavior regressions
  - safety invariant violations
  - replay diffs

### 6.6 `PromotionProposal`

- 一次正式的“是否让 `nanoclaw` 采用这个版本”的治理对象

## 7. 实现策略

### 7.1 先做 `self-observation`，再做 `self-edit`

优先顺序应是：

1. 观察自己
2. 形成自己的回归语料
3. 形成自己的版本化 artifact
4. 在隔离环境中试改自己
5. 最后再推广自己

### 7.2 先做 `proposal`，再做 `automatic promotion`

第一阶段更合理的目标不是自动切换，而是：

- 自动产出高质量 `PromotionProposal`

原因：

- `nanoclaw self-improvement` 的首要风险不是效率，而是自毁

### 7.3 先做低风险资产，再进 runtime code

推荐顺序：

1. prompt
2. skill
3. workflow / routing
4. hook / verifier recipe
5. runtime code

这样可以在不丢掉最终目标的前提下，逐步扩大权限面。

## 8. 分阶段路线

## Phase 0：Signal 与 Task

目标：

- 把 `nanoclaw` 自己的运行问题提炼成正式任务

## Phase 1：Regression Corpus 与 Verifier

目标：

- 形成 `nanoclaw` 自己的 replay / regression / safety 验证体系

## Phase 2：Artifact Versioning

目标：

- 让所有候选都成为正式 `artifact version`

## Phase 3：Isolated Self-Edit

目标：

- 让 prompt / skill / workflow / code 候选都能在 worktree 中试运行

## Phase 4：Promotion Proposal 与 Rollback

目标：

- 让 self-improvement 成为可审查、可回滚、可渐进推广的主路径

## Phase 5：Runtime Code Self-Edit

目标：

- 最终允许 `nanoclaw` 在受控条件下修改 `nanoclaw` 本体代码

## 9. 安全与治理原则

### 9.1 不允许自动放宽安全边界

不允许自动放宽：

- sandbox
- approval
- network
- protected paths
- hook safety policy

### 9.2 promotion 必须是版本切换，而不是直接覆盖

至少要有：

- baseline version
- candidate version
- staged rollout
- rollback

### 9.3 当前活跃 turn 默认不接受热切换

自改进的结果默认影响：

- 新 session
- 新 run
- shadow path

而不是当前活跃 turn。

## 10. 简短结论

真正对齐你需求的研究主线，不应写成：

- `TaskGym-first generic self-improving code agent`

而应写成：

- `runtime-coupled nanoclaw self-hosted self-improvement`

因此，研究重点不再是：

- 如何优化一个通用 code agent

而是：

- 如何让 `nanoclaw` 观察自己
- 如何让 `nanoclaw` 从自己的失败里生成改进任务
- 如何让 `nanoclaw` 在隔离环境里试改自己
- 如何让 `nanoclaw` 版本化地推广和回滚自己
