# Nanoclaw 自改进实现计划

Date: 2026-03-31

Status: Active Execution Plan

Depends On:

- `docs/meta-agent-evolution-plan.md`

## 1. 本文档的目标

这份文档只回答一件事：

- 如何让 `nanoclaw` 在运行过程中，持续改进 `nanoclaw` 自身

这里的目标不是：

- 做一个通用的 `self-improving code agent` 平台
- 做一个以外部 benchmark 为中心的训练系统
- 做一个越来越复杂的 `/improve plan.json` 执行器

这里的目标是：

- `nanoclaw` 正常服务用户
- 同时持续观察自己的失败、低效、回归和人工修正
- 在隔离环境里对 `nanoclaw` 自己的 prompt / skill / workflow / code 产出候选
- 用 `nanoclaw` 自己的 replay / regression / verifier 去判断是否值得推广

所以这份计划的中心对象不是通用 `TaskSuite`，而是：

- `NanoclawArtifact`
- `SelfImproveSignal`
- `SelfImproveTask`
- `SelfImproveRun`
- `NanoclawVerifierReport`

## 2. 核心设计原则

### 2.1 前台运行与后台自改进必须分离

正确模型应当是：

```text
foreground runtime
  -> 继续服务用户

background self-improver
  -> 收集运行信号
  -> 形成自改进任务
  -> 在隔离环境里评估候选
  -> 产出推广建议
```

不允许：

- 在同一 turn 中热改当前正在执行的 runtime
- 在主工作区直接写入并立刻切换到未验证版本

### 2.2 优化对象必须是 `nanoclaw` 自身资产

当前阶段只优化 `nanoclaw` 自己可控的资产层：

- prompt / instructions
- skills
- routing / workflow policy
- hook prompt / verifier recipe
- runtime code

不把目标泛化成任意第三方仓库的通用求解器。

### 2.3 运行证据优先于外部 benchmark

最重要的数据源应当来自 `nanoclaw` 自己：

- 失败 turn
- 人工纠正
- 高成本 / 高延迟 turn
- 错误工具调用
- 错误 subagent 路由
- hook / verifier 拦截结果
- 历史修复 patch

外部 benchmark 仍有价值，但它们是补充验证，不是系统中心。

### 2.4 推广单位必须是版本化 artifact

推广的不是“一次实验结果”，而是：

- 一个 `NanoclawArtifactVersion`

它至少要带上：

- baseline version
- candidate version
- self-improvement task batch
- verifier reports
- rollback pointer

### 2.5 verifier 是防自毁边界，不只是评分器

对于 self-improving `nanoclaw`，verifier 的职责是：

- 阻止系统把自己改坏
- 阻止系统放宽 sandbox / approval / protected path
- 阻止系统在 replay 上退化
- 阻止系统引入工具行为回归

## 3. 目标架构

```text
Nanoclaw Runtime Plane
    ├── normal user turns
    ├── subagent execution
    ├── approvals / sandbox / hooks
    └── transcripts / tool traces / event logs
            │
            ▼
Observation Plane
    ├── failure mining
    ├── correction mining
    ├── latency / cost anomaly mining
    ├── verifier / hook denials
    └── runtime export snapshots
            │
            ▼
Self-Improvement Plane
    ├── self-improve signal queue
    ├── self-regression corpus builder
    ├── candidate artifact generator
    ├── isolated self-edit runner
    └── nanoclaw-specific verifiers
            │
            ▼
Promotion Plane
    ├── versioned artifact ledger
    ├── operator review surface
    ├── staged rollout
    └── rollback
```

最小正确闭环应当是：

```text
collect nanoclaw runtime signals
  -> derive self-improvement tasks
  -> pin current nanoclaw artifact version
  -> generate candidate artifact version
  -> run candidate in isolated worktree
  -> replay nanoclaw regressions and runtime-derived cases
  -> run safety / policy / behavior verifiers
  -> promote or reject candidate version
  -> keep rollback pointer and lineage
```

## 4. 当前仓库可直接复用的基础

下列现有模块是这条路线的现成基座：

- 运行与子代理：
  - `crates/runtime/src/subagent_impl.rs`
  - `crates/runtime/src/agent_session_manager.rs`
  - `crates/runtime/src/write_lease.rs`
- 事件与历史：
  - `crates/runtime/src/runtime/event_log.rs`
  - `apps/code-agent/src/backend/events.rs`
  - `apps/code-agent/src/backend/session_history.rs`
  - `apps/code-agent/src/backend/session_catalog.rs`
- runtime export：
  - `crates/memory/src/runtime_exports.rs`
- hook 边界：
  - `crates/runtime/src/hooks/handlers/agent.rs`
  - 目前仍是 fail-closed stub，这是后续重要接入点
- 实验与归档基础：
  - `crates/meta/*`
  - `crates/store/*`

这些能力说明，仓库已经具备：

- 观测面
- 子代理执行面
- 持久化面
- runtime 安全边界

真正缺的是：

- 面向 `nanoclaw self-improvement` 的信号提炼
- 面向 `nanoclaw self-regression` 的语料和 verifier
- 面向 `nanoclaw` 自身代码 / prompt / workflow 的版本化推广流程

## 5. 不该再继续扩的方向

当前阶段应明确停止把时间花在以下方向上：

- 更复杂的 config candidate taxonomy
- 更复杂的 `/improve plan` 格式
- 脱离 `nanoclaw` 自身运行证据的通用 task gym
- workflow search / island evolution 的前置实现

原因很简单：

- 这些东西会让系统越来越像实验平台
- 但不会让 `nanoclaw` 更快进入“自改进自身”的闭环

## 6. 分阶段实施路线

## W0：术语冻结与边界收紧

目标：

- 把实现对象从通用 `experiment / candidate` 收紧到
  `nanoclaw self-improvement`

必须明确的新术语：

- `NanoclawArtifactKind`
- `NanoclawArtifactVersion`
- `SelfImproveSignal`
- `SelfImproveTask`
- `SelfImproveRun`
- `VerifierFinding`
- `PromotionProposal`

建议写入范围：

- `docs/meta-agent-execution-plan.md`
- 视需要：
  - `crates/types/src/*`
  - `docs/plan.md`

完成标准：

- 后续实现不再以通用 `candidate plan runner` 为中心

## W1：运行信号采集与归一化

目标：

- 先把 `nanoclaw` 自己的可用自改进信号采出来

第一批信号源：

- 失败或中断的 turn
- tool 调用失败
- approval / sandbox 拒绝
- hook fail-closed
- retry churn
- 高 token / 高 latency
- 被人工 rollback / 修正的结果
- 任务等待或多 agent 协调异常

建议写入范围：

- `crates/runtime/src/runtime/event_log.rs`
- `apps/code-agent/src/backend/events.rs`
- `apps/code-agent/src/backend/session_history.rs`
- `apps/code-agent/src/backend/task_history.rs`
- `crates/memory/src/runtime_exports.rs`

建议新增对象：

- `SelfImproveSignalRecord`
- `SignalSeverity`
- `SignalSource`

完成标准：

- 能从真实运行历史中提炼出结构化 signal
- signal 不依赖手写 experiment plan

## W2：自改进任务提炼器

目标：

- 把原始 signal 转成“可执行的 nanoclaw 自改进任务”

第一批任务类型建议：

- `prompt_regression_fix`
- `tool_selection_fix`
- `subagent_routing_fix`
- `hook_policy_fix`
- `cost_latency_optimization`
- `runtime_bugfix`

建议写入范围：

- `crates/meta/src/signals.rs`
- `crates/meta/src/tasks.rs`
- `crates/meta/src/miner.rs`
- 可选：
  - `crates/meta/src/failure_taxonomy.rs`

完成标准：

- 给定一批 runtime signal，系统能生成 `SelfImproveTask` 列表
- 每个任务都能指向：
  - 原始 turn / session
  - 相关文件
  - 期望修复目标

## W3：Nanoclaw 自回归语料

目标：

- 构造 `nanoclaw` 自己的 replay / regression / validation 语料

语料来源：

- 历史失败 turn replay
- 已修复 bug 的 before/after case
- tool misuse case
- prompt failure case
- hook denial case
- subagent coordination failure case

建议写入范围：

- `crates/meta/src/corpus.rs`
- `crates/meta/src/replay.rs`
- `crates/meta/src/regression_pack.rs`
- `crates/store/src/*`
- `crates/memory/src/runtime_exports.rs`

完成标准：

- 至少形成三类集合：
  - `train`
  - `validation`
  - `holdout`
- 这些集合都直接来源于 `nanoclaw` 自身运行历史

## W4：Artifact 版本模型与演化账本

目标：

- 把“改进 nanoclaw 自己”建模成版本化资产推广

第一批 artifact 种类建议：

- `PromptArtifact`
- `SkillArtifact`
- `WorkflowArtifact`
- `HookArtifact`
- `VerifierArtifact`
- `RuntimePatchArtifact`

建议写入范围：

- `crates/types/src/artifact.rs`
- `crates/types/src/id.rs`
- `crates/store/src/traits.rs`
- `crates/store/src/file.rs`
- `crates/store/src/memory.rs`
- `crates/meta/src/archive.rs`

完成标准：

- 任意一个候选都能被记录为 `NanoclawArtifactVersion`
- 任意一次推广都能回溯到 baseline、signal batch、verifier 结果和 rollback pointer

## W5：隔离 self-edit runner

目标：

- 让 `nanoclaw` 修改 `nanoclaw` 的候选只能在隔离环境中运行

最小 runner 行为：

1. 从当前 baseline version 创建 worktree
2. 在 worktree 内应用 prompt / skill / workflow / code 改动
3. 执行 replay、tests、verifiers
4. 回收 diff、stdout/stderr、tool traces、artifacts
5. 清理 worktree

建议写入范围：

- `crates/meta/src/worktree_runner.rs`
- `crates/meta/src/git_gate.rs`
- `crates/meta/src/runner_trace.rs`
- `apps/code-agent/src/backend/session.rs`

完成标准：

- 失败候选不会污染主工作区
- 每个候选都有完整 runner trace

## W6：Nanoclaw 专用 verifier bundle

目标：

- verifier 直接围绕 `nanoclaw` 自己的行为面构造

第一批 verifier：

- `ReplayVerifier`
  - 对历史 turn replay，比较 baseline 与 candidate
- `ToolBehaviorDiffVerifier`
  - 比较工具选择、参数、失败率
- `SubagentRoutingVerifier`
  - 比较子代理分工与任务结果
- `HookPolicyVerifier`
  - 确保 hook 决策未放宽安全边界
- `SandboxApprovalInvariantVerifier`
  - 确保 sandbox / approval 不退化
- `RegressionPackVerifier`
  - 跑已知自回归案例

建议写入范围：

- `crates/evals/src/verifiers/replay.rs`
- `crates/evals/src/verifiers/tool_diff.rs`
- `crates/evals/src/verifiers/routing.rs`
- `crates/evals/src/verifiers/hook_policy.rs`
- `crates/evals/src/verifiers/security_invariants.rs`
- `crates/runtime/src/hooks/handlers/agent.rs`

完成标准：

- verifier 报告能回答：
  - `nanoclaw` 是否更稳
  - `nanoclaw` 是否更便宜
  - `nanoclaw` 是否更安全
  - `nanoclaw` 是否引入行为回归

## W7：后台自改进控制器

目标：

- 把观察、提炼、候选运行、验证和推广建议串起来

控制器职责：

1. 周期性或事件驱动读取 signal queue
2. 聚合成一个 `SelfImproveRun`
3. 选择 baseline artifact version
4. 生成候选版本
5. 调用 isolated runner
6. 聚合 verifier 报告
7. 产出 `PromotionProposal`

建议写入范围：

- `crates/meta/src/controller.rs`
- `crates/meta/src/budget.rs`
- `crates/meta/src/run.rs`
- `crates/meta/src/proposal.rs`

完成标准：

- 可以后台异步运行，不影响前台正常服务
- 默认只产出 proposal，不自动热切换

## W8：Operator Surface

目标：

- 给 operator 一个能理解 `nanoclaw 正在如何改自己` 的界面

建议命令：

- `/self-improve status`
- `/self-improve runs`
- `/self-improve run <id>`
- `/self-improve propose`
- `/self-improve promote <version>`
- `/self-improve rollback <version>`

建议写入范围：

- `apps/code-agent/src/backend/session.rs`
- `apps/code-agent/src/backend/session_history.rs`
- `apps/code-agent/src/frontend/tui/commands.rs`
- `apps/code-agent/src/frontend/tui/history.rs`
- `apps/code-agent/src/frontend/tui/mod.rs`

完成标准：

- operator 可以看到：
  - 哪些 signal 被采集
  - 形成了哪些自改进任务
  - 产出了哪些 artifact version
  - 为什么 proposal 被拒绝或允许

## W9：安全推广与回滚

目标：

- 让 self-improvement 真正可上线，但仍然可控

推广原则：

- 默认不热切换当前活跃 turn
- 先从新 session / 新 run 生效
- 先 shadow，再 limited rollout，再 default
- 任意时刻可 rollback 到上一个稳定 version

建议写入范围：

- `crates/meta/src/promotion.rs`
- `crates/meta/src/rollback.rs`
- `crates/config/*`
- `apps/code-agent/src/backend/*`

完成标准：

- promotion 是版本切换，不是直接覆盖
- rollback 是正式主路径，不是手工补救

## W10：RuntimePatchArtifact 主路径

目标：

- 最终让 `nanoclaw` 可以在受控条件下修改 `nanoclaw` 的 Rust 代码本体

进入条件：

- `W1-W9` 已稳定
- replay / regression / security invariant verifier 足够强
- operator surface 已能审查 proposal

建议写入范围：

- `crates/meta/src/candidate.rs`
- `crates/meta/src/worktree_runner.rs`
- `crates/evals/src/verifiers/*`
- `crates/runtime/*`
- `apps/code-agent/*`

完成标准：

- 能对 `nanoclaw` 本仓库产出 code patch candidate
- 能在隔离环境里通过完整 verifier bundle
- 能版本化推广并回滚

## 7. 并行实施建议

### Wave A

- 串行：
  - `W0`

### Wave B

- 并行：
  - `W1`
  - `W2`
  - `W4`

说明：

- `W4` 的模型定义可以先和 `W1/W2` 并行推进
- 但不能先做 promotion 逻辑

### Wave C

- 并行：
  - `W3`
  - `W5`
  - `W6`

说明：

- `W5` 和 `W6` 在最后通过 `W3` 的 replay / regression corpus 收口

### Wave D

- 串行：
  - `W7`
  - `W8`
  - `W9`

### Wave E

- 最后推进：
  - `W10`

## 8. 风险与止损点

### 风险 1：把系统做成“通用训练平台”

止损：

- 每个新增模块都先问一句：
  - 它是否直接服务 `nanoclaw` 改进 `nanoclaw`？

### 风险 2：把前台 runtime 和后台自改进混在一起

止损：

- 自改进默认走后台控制器
- 推广默认不影响当前活跃 turn

### 风险 3：过早进入 runtime code self-edit

止损：

- 先做 prompt / skill / workflow / hook / verifier 层
- 等 replay / security invariants 成熟后再进 `RuntimePatchArtifact`

### 风险 4：只有实验记录，没有真正可复用版本

止损：

- archive 必须围绕 `artifact version` 建模
- 不再把 experiment 记录当成最终产物

### 风险 5：verifier 太弱，导致自毁

止损：

- `sandbox / approval / protected path` 必须有 invariant verifier
- hook evaluator 接入前必须 fail-closed

## 9. 第一刀应该做什么

如果现在只做第一刀，我建议严格收缩为：

1. `W1` 运行信号采集
2. `W2` 自改进任务提炼器
3. `W3` 最小 nanoclaw 自回归语料
4. `W4` artifact 版本模型
5. `W5` 最小 isolated self-edit runner
6. `W6` 第一批 nanoclaw 专用 verifier

这第一刀的验证目标不是“能不能跑 improve plan”，而是：

- 能不能从 `nanoclaw` 自己的运行中抽取有效信号
- 能不能把这些信号变成针对 `nanoclaw` 自己的改进任务
- 能不能把 `nanoclaw` 的候选版本关进隔离环境评估
- 能不能可靠阻止坏版本被推广

## 10. 简短结论

如果目标真的是：

- `nanoclaw` 在运行过程中 self-improving `nanoclaw` 本身

那么工程主线就不该再写成：

`archive`
-> `candidate generator`
-> `single-run improve`

而应当写成：

`runtime observation`
-> `signal mining`
-> `self-regression corpus`
-> `artifact versioning`
-> `isolated self-edit`
-> `nanoclaw-specific verification`
-> `proposal / promotion / rollback`

前者会做出一个实验平台。  
后者才有机会做出一个真正会持续改进自身的 `nanoclaw`。
