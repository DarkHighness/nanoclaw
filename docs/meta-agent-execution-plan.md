# Meta Agent 可执行实施计划

Date: 2026-03-31

Status: Active Execution Plan

Depends On:

- `docs/meta-agent-evolution-plan.md`

## 1. 本文档的作用

上一份文档回答的是：

- 为什么要做 Meta Agent
- 参考了哪些工业项目与论文
- 总体架构和阶段路线是什么

这份文档只做一件事：

- 把路线拆成可以直接开工的工程任务

默认目标不是一步到位实现完整自进化系统，而是先交付一个
`offline meta-agent MVP`：

- 有实验账本
- 有 evaluator substrate
- 有 prompt / skill / policy 自优化闭环
- 有 host 侧操作入口
- 全流程可回滚、可审计、可测试

## 2. 第一阶段交付边界

### 2.1 本阶段必须交付

- `Experiment Archive`
- `Evaluator / Verifier Substrate`
- `PromptVariant / SkillVariant / PolicyVariant` 候选模型
- `offline replay + benchmark` 闭环
- host 操作入口：
  - `/improve`
  - `/experiments`
  - `/promote`
  - `/benchmark`

### 2.2 本阶段明确不做

- 自动修改 `nanoclaw` 核心代码
- live traffic 自动推广
- 全图 workflow search
- active verifier 的复杂反例搜索
- group / island evolution

原因：

- 当前仓库最缺的是评测与治理面，不是再加一层搜索算法

## 3. 依赖关系图

```text
Wave 0
  └── W0 Workspace Seeding
        │
        ├── W1 Experiment Schema + Store
        ├── W2 Evals Crate Core
        └── W3 Meta Crate Core
                │
                ├── W4 Benchmark Pack
                ├── W5 Promotion / Rollback MVP
                └── W6 Host Commands + Views
                        │
                        └── W7 End-to-End Offline Improve Run
                                │
                                ├── W8 Workflow IR Skeleton
                                ├── W9 Active Verifier
                                └── W10 Worktree Candidate Runner
```

说明：

- `W0` 必须串行，因为会改 workspace manifest
- `W1/W2/W3` 可并行，前提是先把 crate 目录和 manifest 种好
- `W4/W5/W6` 可并行，但最终接线必须单独做一轮 integration

## 4. 全局 Definition of Done

任一工作包完成前，必须同时满足：

1. 有明确写入边界，未修改无关文件。
2. 有单元测试或集成测试覆盖核心路径。
3. 有最小 operator 可见面：
   - 事件可查
   - 结果可看
   - 错误可解释
4. 有失败时的回滚路径。
5. 不放宽现有 sandbox / approval 安全边界。

## 5. Wave 0：工作区种子与接口冻结

## W0.1 新 crate 与 workspace 挂载

目标：

- 为后续并行开发先把写冲突最高的 shared manifest 改完

写入范围：

- `crates/Cargo.toml`
- 可选：`crates/core/Cargo.toml`
- 新目录：
  - `crates/evals/`
  - `crates/meta/`

执行步骤：

1. 在 `crates/Cargo.toml` 注册 `evals` 与 `meta`。
2. 建立两个 crate 的最小 `Cargo.toml` 与 `src/lib.rs`。
3. 先不写复杂逻辑，只暴露占位模块。
4. 若 `core` 负责对外聚合，再决定是否 re-export。

交付物：

- workspace 能识别两个新 crate
- `cargo check` 不因缺 crate 失败

验证：

```bash
cargo check --manifest-path crates/Cargo.toml
```

完成标准：

- 后续 `W1/W2/W3` 可以并行且不再争抢 workspace manifest

## 6. Wave 1：MVP 基础能力

## W1：Experiment Schema + Store

目标：

- 把“实验”和“候选”变成一等持久化对象

写入范围：

- `crates/types/src/id.rs`
- `crates/types/src/event.rs`
- `crates/store/src/traits.rs`
- `crates/store/src/file.rs`
- `crates/store/src/file/index_sidecar.rs`
- 可能新增：
  - `crates/store/src/experiments.rs`

建议新增对象：

- `ExperimentId`
- `CandidateId`
- `BaselineId`
- `ExperimentSpec`
- `CandidateSpec`
- `EvaluationSummary`
- `PromotionDecision`

建议新增事件：

- `ExperimentStarted`
- `ExperimentBaselinePinned`
- `CandidateGenerated`
- `CandidateEvaluated`
- `CandidatePromoted`
- `CandidateRejected`
- `CandidateRolledBack`

执行步骤：

1. 在 `types` 增加 id 与结构体。
2. 在 `event.rs` 增加实验相关事件。
3. 在 `store::traits` 增加最小 API：
   - `append_experiment_event`
   - `load_experiment`
   - `list_experiments`
4. 在 file store 中落盘：
   - experiment summary
   - candidate summary
   - evaluation report
5. 做 round-trip tests。
6. 确保和已有 `SessionStore` 模型兼容，不复制 transcript。

交付物：

- 任何一次 improve run 都有独立 experiment 记录
- experiment 与 session / agent-session 可关联

验证：

```bash
cargo test --manifest-path crates/Cargo.toml -p store
cargo test --manifest-path crates/Cargo.toml -p types
```

完成标准：

- 给定 `ExperimentId`，可以查到 baseline、候选、评测、推广决策

## W2：Evals Crate Core

目标：

- 提供统一 evaluator substrate

写入范围：

- `crates/evals/Cargo.toml`
- `crates/evals/src/lib.rs`
- `crates/evals/src/result.rs`
- `crates/evals/src/evaluator.rs`
- `crates/evals/src/registry.rs`
- `crates/evals/src/context.rs`
- `crates/evals/src/evaluators/*.rs`

建议模块：

- `result.rs`
- `evaluator.rs`
- `registry.rs`
- `evaluators/command.rs`
- `evaluators/test_suite.rs`
- `evaluators/output_schema.rs`
- `evaluators/diff_policy.rs`
- `evaluators/safety.rs`
- `evaluators/cost_latency.rs`

核心接口建议：

```rust
pub struct EvalContext {
    pub experiment_id: ExperimentId,
    pub candidate_id: CandidateId,
    pub workspace_root: PathBuf,
}

pub struct EvalResult {
    pub evaluator_name: String,
    pub passed: bool,
    pub score: Option<f64>,
    pub summary: String,
    pub details: serde_json::Value,
}

#[async_trait]
pub trait Evaluator {
    async fn evaluate(&self, ctx: &EvalContext) -> anyhow::Result<EvalResult>;
}
```

执行步骤：

1. 先定义结果模型与 trait。
2. 做 registry，支持串行与固定顺序执行。
3. 先接 5 个基础 evaluator：
   - command
   - test suite
   - schema
   - diff policy
   - safety
4. 把 cost/latency evaluator 做成汇总器。
5. 为每个 evaluator 写最小 fixture test。

交付物：

- evaluator 能独立运行，不依赖 host UI
- 同一 candidate 能产出 evaluator matrix

验证：

```bash
cargo test --manifest-path crates/Cargo.toml -p evals
```

完成标准：

- evaluator matrix 可以被 `meta` crate 直接消费

## W3：Meta Crate Core

目标：

- 提供候选生成、比较、推广的最小控制面

写入范围：

- `crates/meta/Cargo.toml`
- `crates/meta/src/lib.rs`
- `crates/meta/src/candidate.rs`
- `crates/meta/src/experiment.rs`
- `crates/meta/src/critic.rs`
- `crates/meta/src/comparator.rs`
- `crates/meta/src/promotion.rs`
- `crates/meta/src/rollback.rs`

建议最小候选类型：

- `PromptVariant`
- `SkillVariant`
- `PolicyVariant`

建议核心流程对象：

- `ExperimentRunner`
- `CandidateGenerator`
- `CandidateComparator`
- `PromotionGate`
- `RollbackPlan`

执行步骤：

1. 定义 candidate schema。
2. 定义 baseline pin 与 candidate pin。
3. 实现 comparator：
   - 必须通过哪些 evaluator
   - 哪些 score 可以是 soft improvement
4. 实现 promotion gate：
   - 默认 reject
   - 必须显式满足 gate 才 allow
5. 实现 rollback model：
   - promotion 后保留 baseline pointer

交付物：

- 给定 baseline 与失败样本，能产出候选并比较

验证：

```bash
cargo test --manifest-path crates/Cargo.toml -p meta
```

完成标准：

- `meta` crate 能在无 TUI 条件下完成一次离线 experiment 判定

## 7. Wave 2：MVP 业务闭环

## W4：Benchmark Pack

目标：

- 给 MVP 提供稳定、可回归的任务集

写入范围：

- `crates/evals/src/benchmarks/*.rs`
- `crates/evals/fixtures/meta-agent/*`
- 可选：
  - `crates/test-support/src/meta_agent/*`

第一批 benchmark 类型建议：

- `prompt_quality`
  - 评估 prompt/profile 调整后的输出稳定性
- `skill_selection`
  - 评估 skill 组合是否改善任务完成率
- `readonly_research`
  - 评估只读研究任务的答案质量
- `patch_review`
  - 评估代码审查任务的发现质量

执行步骤：

1. 先定义 benchmark manifest 格式。
2. 把 benchmark case 与 evaluator matrix 解耦。
3. 先做 repo-local 小任务集，不接外部大 benchmark。
4. 预留后续导入：
   - HumanEval
   - MBPP
   - SWE-Gym / SWE-Bench 风格

交付物：

- 一个 `BenchmarkSuite` 能驱动多 evaluator

验证：

```bash
cargo test --manifest-path crates/Cargo.toml -p evals benchmarks
```

完成标准：

- 至少有 10-20 个稳定 case 可用于 regression

## W5：Promotion / Rollback MVP

目标：

- 让候选不是“跑完就丢”，而是能晋级或回滚

写入范围：

- `crates/meta/src/promotion.rs`
- `crates/meta/src/rollback.rs`
- `crates/store/*`
- 可能新增：
  - `apps/code-agent/src/backend/meta.rs`

执行步骤：

1. 定义 promotion policy：
   - hard gate
   - soft score
2. 定义 candidate 状态：
   - draft
   - evaluated
   - promoted
   - rejected
   - rolled_back
3. 实现 rollback pointer。
4. 加一条安全规则：
   - 任何自动推广都不能放宽 sandbox / approval。

交付物：

- promotion decision 可持久化
- rollback 可恢复 baseline

验证：

- unit tests 覆盖：
  - gate pass
  - gate reject
  - rollback success

完成标准：

- promotion 与 rollback 都不依赖手工改 store 文件

## W6：Host Commands + Views

目标：

- 给 operator 一个最小可用的交互面

写入范围：

- `apps/code-agent/src/backend/mod.rs`
- 新增：
  - `apps/code-agent/src/backend/meta.rs`
- `apps/code-agent/src/frontend/tui/commands.rs`
- `apps/code-agent/src/frontend/tui/state.rs`
- `apps/code-agent/src/frontend/tui/render/view.rs`
- 视情况补：
  - `history.rs`
  - `render/transcript.rs`

建议命令：

- `/benchmark [suite]`
- `/improve [target]`
- `/experiments`
- `/experiment <id>`
- `/promote <candidate-id>`
- `/rollback <candidate-id>`

执行步骤：

1. 在 backend 暴露 meta-agent service。
2. 在 slash command parser 加命令。
3. 在主视图增加 experiment summary 渲染。
4. 在历史视图中可跳转到 experiment detail。
5. 为 parser 和渲染加测试。

交付物：

- operator 能从 TUI 发起 benchmark / improve / promote / rollback

验证：

```bash
cargo test --manifest-path apps/Cargo.toml -p code-agent commands
```

完成标准：

- 不需要直接操作 store 文件就能跑通一次实验

## 8. Wave 3：MVP 集成与验收

## W7：End-to-End Offline Improve Run

目标：

- 打通一次完整闭环

闭环定义：

```text
select benchmark suite
  -> pin baseline
  -> generate Prompt/Skill/Policy candidates
  -> run evaluator matrix
  -> compare
  -> record experiment
  -> promote or reject
  -> rollback if needed
```

建议集成顺序：

1. 先只支持 `PromptVariant`
2. 再打开 `SkillVariant`
3. 最后加 `PolicyVariant`

验收脚本：

1. 创建一个固定 benchmark suite。
2. 人工制造一个 baseline 缺陷。
3. 跑 `/improve`。
4. 确认有候选生成。
5. 确认 evaluator matrix 完整。
6. 确认 promotion / reject 生效。
7. 如 promotion，执行 rollback 验证。

完成标准：

- 在本仓库内能稳定重放同一 improve run

## 9. MVP 之后的执行包

以下内容在 MVP 收口后再开：

## W8：Workflow IR Skeleton

目标：

- 让优化对象从配置升级到结构

写入范围：

- `crates/meta/src/workflow_ir.rs`
- `crates/meta/src/workflow_exec.rs`
- `crates/meta/src/workflow_templates.rs`

第一批节点：

- `PromptNode`
- `SubagentNode`
- `RouterNode`
- `ParallelMapNode`
- `EvaluatorNode`
- `RetryNode`

第一批模板：

- review-revise
- plan-execute-verify
- orchestrator-worker

## W9：Active Verifier

目标：

- 从 pass/fail evaluator 升级到能主动找反例的 verifier

写入范围：

- `crates/evals/src/verifiers/*.rs`
- `crates/meta/src/replan.rs`

第一批 verifier：

- `CounterexampleVerifier`
- `BehaviorDiffVerifier`
- `ReplanTriggerVerifier`

完成标准：

- verifier 可在候选之间挖掘行为差异并触发 replan

## W10：Worktree Candidate Runner

目标：

- 让代码级候选在隔离环境中运行

写入范围：

- 新增：
  - `crates/meta/src/worktree_runner.rs`
  - `crates/meta/src/git_gate.rs`

完成标准：

- 每个 code candidate 都能在独立 worktree 中评测

## W11：Group / Island Evolution

目标：

- 实现 2026 文献强调的 group-level experience sharing

写入范围：

- `crates/meta/src/archive.rs`
- `crates/meta/src/lineage.rs`
- `crates/meta/src/island.rs`
- `crates/meta/src/group_evolution.rs`

完成标准：

- 支持 archive、多样性候选、跨 lineage 经验迁移

## 10. 并行实施建议

为了避免写冲突，建议按下面分波：

### Wave A

- 串行：
  - `W0.1`

### Wave B

- 并行：
  - `W1`
  - `W2`
  - `W3`

注意：

- `W1/W2/W3` 不应同时改 `crates/Cargo.toml`
- `W3` 不应提前改 host UI

### Wave C

- 并行：
  - `W4`
  - `W5`
  - `W6`

注意：

- `W6` 与 `W5` 的集成点放到最后一轮单独收口

### Wave D

- 串行：
  - `W7`

### Wave E

- 视风险推进：
  - `W8`
  - `W9`
  - `W10`
  - `W11`

## 11. 风险清单与止损点

### 风险 1：过早把 workflow search 和 code evolution 混到 MVP

止损：

- MVP 严格限制在 Prompt / Skill / Policy

### 风险 2：把 session store 直接做成 experiment store

止损：

- 共享底层持久化能力
- 但 experiment 模型单独成对象，不污染 transcript 主线

### 风险 3：TUI 先行导致 backend contract 反复重写

止损：

- 先完成 `meta` crate 与 backend service，再接 TUI

### 风险 4：evaluator 太弱，导致 promotion 虚高

止损：

- promotion 默认 reject
- 没有 regression suite 不允许自动 promote

## 12. 建议的首个实现切片

如果只做第一刀，我建议选下面这个切片：

1. `W0.1`
2. `W1`
3. `W2`
4. `W3` 的最小版本：
   - 只支持 `PromptVariant`
5. `W4` 的最小版本：
   - 只做 10 个 repo-local case
6. `W7` 的 CLI/backend 闭环，先不接完整 TUI

这样可以最短路径验证三件关键事：

- experiment archive 是否合理
- evaluator substrate 是否足够强
- Meta Agent MVP 是否真的能改进结果

## 13. 简短结论

可执行路线的关键不是把阶段写得更细，而是先把 shared contract 锁住，
然后按低风险闭环推进。

对 `nanoclaw`，最优先顺序应是：

`workspace seeding`
-> `experiment schema`
-> `evals`
-> `meta core`
-> `benchmark pack`
-> `promotion/rollback`
-> `host commands`
-> `end-to-end offline improve run`

只有这条 MVP 线稳定后，才值得继续做 workflow IR、active verifier、
worktree 自修改与 group evolution。

## 14. Wave 0 / Wave 1 逐文件 Checklist

这一节只覆盖最先开工的切片，目标是把任务进一步压缩到“改哪些文件”。

### 14.1 W0.1 逐文件 Checklist

- `crates/Cargo.toml`
  - 把 `evals`、`meta` 加入 `members`
  - 视需要加入 `default-members`
  - 如果两个 crate 共享依赖，补到 `workspace.dependencies`
- `crates/evals/Cargo.toml`
  - 建 package 元数据
  - 只引入最小依赖：
    - `async-trait`
    - `serde`
    - `serde_json`
    - `thiserror` 或 `anyhow`
    - `tokio`
    - `types`
    - `store`
- `crates/evals/src/lib.rs`
  - 先导出空模块：
    - `context`
    - `evaluator`
    - `registry`
    - `result`
- `crates/meta/Cargo.toml`
  - 建 package 元数据
  - 只引入最小依赖：
    - `async-trait`
    - `serde`
    - `serde_json`
    - `tokio`
    - `types`
    - `store`
    - `evals`
- `crates/meta/src/lib.rs`
  - 先导出空模块：
    - `candidate`
    - `critic`
    - `experiment`
    - `promotion`
    - `rollback`
- `crates/core/Cargo.toml`
  - 仅在确定要对外 re-export 时改
  - 若当前阶段只在 app 内消费，可先不动

### 14.2 W1 逐文件 Checklist

- `crates/types/src/id.rs`
  - 新增：
    - `ExperimentId`
    - `CandidateId`
    - `BaselineId`
- `crates/types/src/event.rs`
  - 新增 experiment/candidate 结构体
  - 新增事件 kind
  - 保持和现有 `SessionEventEnvelope` 风格一致
- `crates/types/src/lib.rs`
  - 导出新增 id 与 event 类型
- `crates/store/src/traits.rs`
  - 扩展 `SessionStore`
  - 新增 experiment 查询 / 持久化 API
  - 新增 experiment summary/result 类型
- `crates/store/src/file.rs`
  - 实现 file-backed experiment 存储
  - 尽量沿用现有 sidecar/index 思路，不复制 transcript 文件设计
- `crates/store/src/memory.rs`
  - 同步实现 in-memory 版本，避免 test 环境与 file backend 分叉
- `crates/store/src/lib.rs`
  - 导出 experiment 相关模块
- `crates/store/src/file/index_sidecar.rs`
  - 如果 experiment 也需要索引，就在这里扩展 sidecar
  - 若复杂度过高，可先让 experiment 走独立 index 文件

### 14.3 W2 逐文件 Checklist

- `crates/evals/src/context.rs`
  - 定义 `EvalContext`
- `crates/evals/src/result.rs`
  - 定义 `EvalResult`
  - 定义 `EvalMatrix`
- `crates/evals/src/evaluator.rs`
  - 定义 `Evaluator` trait
- `crates/evals/src/registry.rs`
  - 定义 evaluator registry 与执行顺序
- `crates/evals/src/evaluators/command.rs`
  - 命令退出码 evaluator
- `crates/evals/src/evaluators/test_suite.rs`
  - 测试套件 evaluator
- `crates/evals/src/evaluators/output_schema.rs`
  - 结构化输出 evaluator
- `crates/evals/src/evaluators/diff_policy.rs`
  - diff / protected path evaluator
- `crates/evals/src/evaluators/safety.rs`
  - sandbox / network / approval policy evaluator
- `crates/evals/src/evaluators/cost_latency.rs`
  - 成本与延迟汇总 evaluator

### 14.4 W3 逐文件 Checklist

- `crates/meta/src/candidate.rs`
  - 定义 `PromptVariant` / `SkillVariant` / `PolicyVariant`
- `crates/meta/src/experiment.rs`
  - 定义 `ExperimentRunner`
  - 连接 `store` 与 `evals`
- `crates/meta/src/critic.rs`
  - 输入失败样本，输出 failure taxonomy
- `crates/meta/src/promotion.rs`
  - 定义 promotion gate
- `crates/meta/src/rollback.rs`
  - 定义 rollback model
- `crates/meta/src/lib.rs`
  - 对外导出最小可用 API

## 15. 建议提交切片

为了降低 review 与回滚成本，建议按下面顺序提交：

1. `chore(workspace): seed evals and meta crates`
   - 只改 workspace manifest 与新 crate 骨架
2. `feat(types): add meta-agent experiment identifiers and event contracts`
   - 只改 `types`
3. `feat(store): persist experiment archive records`
   - 只改 `store`
4. `feat(evals): add evaluator core and base evaluators`
   - 只改 `evals`
5. `feat(meta): add candidate, comparator, and promotion core`
   - 只改 `meta`
6. `feat(code-agent): add offline improve operator surface`
   - 最后改 host app

这样切的好处是：

- 每个 commit 的职责单一
- 每个 commit 都有清晰测试边界
- 若 `meta` 设计调整，不会污染 `types/store/evals` 的基础层
