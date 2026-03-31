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
`offline self-improving code-agent MVP`：

- 有实验账本
- 有 evaluator / verifier substrate
- 有 baseline compare 与 relative promotion gate
- 有 worktree-scoped code candidate runner
- 有 host 侧操作入口
- 全流程可回滚、可审计、可测试

## 1.1 方案纠偏

当前实现已经证明一件事：

- 只扩 candidate generator，并不会自然得到 `self-improving code agent`

如果目标真的是 code agent 的自改进，必须把主线收敛到下面这条闭环：

```text
pin baseline
  -> run baseline on coding benchmark
  -> derive failure taxonomy
  -> generate candidate
  -> run candidate in isolated worktree
  -> compare candidate vs baseline
  -> verify regressions / counterexamples
  -> promote or reject
  -> rollback if needed
```

因此后续实施优先级要调整为：

1. baseline compare
2. isolated worktree candidate runner
3. coding benchmark + verifier
4. automatic critic derivation
5. iterative improve controller

下列能力仍有价值，但都应放到第二优先级：

- richer generator taxonomy
- workflow IR
- island / group evolution

## 2. 第一阶段交付边界

### 2.1 本阶段必须交付

- `Experiment Archive`
- `Evaluator / Verifier Substrate`
- `baseline evaluation + relative improvement gate`
- `worktree-scoped code candidate runner`
- `offline coding benchmark + verifier` 闭环
- `PromptVariant / SkillVariant / PolicyVariant` 只作为低风险启动对象
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

- 当前仓库最缺的是 baseline compare、隔离执行、verifier 和治理面，
  不是再加一层 generator 花样

## 3. 依赖关系图

```text
Wave 0
  └── W0 Workspace Seeding
        │
        ├── W1 Experiment Schema + Store
        ├── W2 Evals Crate Core
        └── W3 Meta Crate Core
                │
                ├── W4 Baseline Compare + Promotion Gate
                ├── W5 Coding Benchmark Pack
                ├── W6 Worktree Candidate Runner
                └── W7 Host Commands + Views
                        │
                        └── W8 End-to-End Offline Improve Run
                                │
                                ├── W9 Automatic Critic Derivation
                                ├── W10 Iterative Improve Controller
                                ├── W11 Workflow IR Skeleton
                                └── W12 Group / Island Evolution
```

说明：

- `W0` 必须串行，因为会改 workspace manifest
- `W1/W2/W3` 可并行，前提是先把 crate 目录和 manifest 种好
- `W4/W5/W6/W7` 中，`W6` 是 self-improving code agent 的关键路径，不能长期后置
- generator 丰富度可以并行探索，但不能阻塞 baseline compare 与 worktree runner

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
- `CodePatchVariant`

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

- 给定 baseline，系统能明确回答“candidate 是否优于 baseline”

验证：

```bash
cargo test --manifest-path crates/Cargo.toml -p meta
```

完成标准：

- `meta` crate 能在无 TUI 条件下完成一次离线 experiment 判定

## 7. Wave 2：MVP 业务闭环

## W4：Baseline Compare + Promotion Gate

目标：

- 把 `/improve` 从“候选筛选”收紧成“相对 baseline 的改进判定”

写入范围：

- `crates/meta/src/benchmark.rs`
- `crates/meta/src/improve.rs`
- `crates/meta/src/promotion.rs`
- `crates/meta/src/experiment.rs`
- `crates/types/src/experiment.rs`
- `crates/store/src/traits.rs`

执行步骤：

1. 记录 baseline evaluation。
2. 在 improve / benchmark 中显式跑 baseline。
3. 引入 relative gate：
   - minimum score
   - minimum score gain over baseline
4. 在 archive 和 TUI 中可见 baseline score 与 delta。

完成标准：

- promotion decision 不再只看 candidate 自身分数
- operator 能看到 candidate 是否真实优于 baseline

## W5：Coding Benchmark Pack

目标：

- 给 code-agent MVP 提供稳定、可回归的 coding task 集

写入范围：

- `crates/evals/src/benchmarks/*.rs`
- `crates/evals/fixtures/meta-agent/*`
- 可选：
  - `crates/test-support/src/meta_agent/*`

第一批 benchmark 类型建议：

- `patch_review`
  - 评估代码审查任务的发现质量
- `small_patch_fix`
  - 评估小规模代码修复任务的通过率
- `tool_use_reliability`
  - 评估 code-agent 在真实工具链中的稳定性
- `regression_guard`
  - 评估 candidate 是否破坏既有行为

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

## W6：Worktree Candidate Runner

目标：

- 让 code candidate 真正跑在隔离环境里，而不是只在配置层比较

写入范围：

- `crates/meta/src/worktree_runner.rs`
- `crates/meta/src/git_gate.rs`
- `apps/code-agent/src/backend/*`

执行步骤：

1. 为每个 code candidate 准备独立 worktree。
2. 在 worktree 内执行 patch / test / verifier。
3. 回收 artifact、diff、stdout/stderr、test result。
4. 保证失败时不污染主工作区。

完成标准：

- 每个 code candidate 都可独立运行和销毁
- improve run 能区分“配置候选”与“代码候选”

## W7：Host Commands + Views

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

## W8：End-to-End Offline Improve Run

目标：

- 打通一次完整闭环，并明确这是 code-agent 的改进闭环，而不是通用配置试验器

闭环定义：

```text
pin baseline
  -> run baseline on benchmark
  -> derive critic / failure taxonomy
  -> generate candidate
  -> run candidate in isolated worktree
  -> compare candidate vs baseline
  -> verify regressions
  -> record experiment
  -> promote or reject
  -> rollback if needed
```

建议集成顺序：

1. 先用 `PromptVariant / PolicyVariant` 验证 archive 和 gate 契约。
2. 尽快接 `CodePatchVariant`，不要长期停留在配置层。
3. 当 code candidate loop 稳定后，再考虑 workflow optimization。

完成标准：

- 在本仓库内能稳定重放同一 improve run
- promotion 语义明确是“优于 baseline”，不是“单独过阈值”

## W9：Automatic Critic Derivation

目标：

- 不再手写 critic report，而是从 benchmark/verifier failure 中自动抽取

写入范围：

- `crates/meta/src/critic.rs`
- `crates/meta/src/candidate.rs`
- `crates/evals/src/verifiers/*.rs`

完成标准：

- improve run 可以从 evaluator / verifier 结果自动生成 failure taxonomy

## W10：Iterative Improve Controller

目标：

- 让 improve 不只是一次性 run，而是预算受控的多轮尝试

写入范围：

- `crates/meta/src/controller.rs`
- `crates/meta/src/budget.rs`

完成标准：

- 支持 stop condition、budget、early stop、best-so-far tracking

## W11：Workflow IR Skeleton

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

## W12：Group / Island Evolution

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
  - `W12`

## 11. 风险清单与止损点

### 风险 1：过早把 workflow search 和 code evolution 混到 MVP

止损：

- MVP 先收紧在：
  - baseline compare
  - coding benchmark
  - worktree candidate runner
  - relative promotion gate
- generator 与 workflow 扩展都不能挤占这条关键路径

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
- 没有 regression suite 与 baseline compare，不允许自动 promote

## 12. 建议的首个实现切片

如果只做第一刀，我建议选下面这个切片：

1. `W0.1`
2. `W1`
3. `W2`
4. `W3` 的最小版本：
   - 只支持 `PromptVariant`
5. `W4`
   - baseline evaluation
   - relative promotion gate
6. `W5`
   - 只做 10 个 repo-local coding case
7. `W6`
   - 只支持最小 worktree candidate runner
8. `W7`
   - 先接 CLI/backend 闭环，TUI 保持最小

这样可以最短路径验证四件关键事：

- experiment archive 是否合理
- baseline compare 是否真实工作
- worktree candidate loop 是否安全可控
- Meta Agent 是否真的在 code task 上改进结果

## 13. 简短结论

可执行路线的关键不是把阶段写得更细，而是先把 shared contract 锁住，
然后按低风险闭环推进。

对 `nanoclaw`，最优先顺序应是：

`workspace seeding`
-> `experiment schema`
-> `evals`
-> `meta core`
-> `baseline compare`
-> `coding benchmark`
-> `worktree runner`
-> `host commands`
-> `end-to-end offline improve run`

只有这条 MVP 线稳定后，才值得继续做 workflow IR、active verifier、
hybrid nodes 与 group evolution。

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
  - 定义 `PromptVariant` / `SkillVariant` / `PolicyVariant` / `CodePatchVariant`
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

### 14.5 W4 逐文件 Checklist

- `crates/meta/src/benchmark.rs`
  - 记录 baseline evaluation
  - 产出 candidate vs baseline delta
- `crates/meta/src/improve.rs`
  - 强制 baseline 与 candidate 跑同一评测集
- `crates/meta/src/promotion.rs`
  - 新增 relative gate
  - 新增 minimum score gain over baseline
- `crates/meta/src/experiment.rs`
  - 写入 baseline score、candidate score、delta、decision
- `crates/types/src/experiment.rs`
  - 扩展 baseline evaluation / compare 结果结构
- `crates/store/src/traits.rs`
  - 暴露 baseline compare 相关查询接口

### 14.6 W5 逐文件 Checklist

- `crates/evals/src/benchmarks/mod.rs`
  - 定义 `BenchmarkManifest` / `BenchmarkSuite`
- `crates/evals/src/benchmarks/*.rs`
  - 实现 repo-local coding cases
- `crates/evals/src/verifiers/*.rs`
  - 定义 regression / behavior diff verifier
- `crates/evals/fixtures/meta-agent/*`
  - 增加 benchmark fixture
- `crates/test-support/src/meta_agent/*`
  - 视需要增加 benchmark 测试支撑

### 14.7 W6 逐文件 Checklist

- `crates/meta/src/worktree_runner.rs`
  - 创建、执行、回收独立 worktree
- `crates/meta/src/git_gate.rs`
  - 校验 write set / protected path
- `crates/meta/src/candidate.rs`
  - 为 `CodePatchVariant` 增加 patch / artifact schema
- `apps/code-agent/src/backend/*`
  - 接 worktree runner 的 host-side orchestration

### 14.8 W7 逐文件 Checklist

- `apps/code-agent/src/backend/session.rs`
  - 暴露 `/benchmark` 与 `/improve` 的 backend 入口
- `apps/code-agent/src/backend/session_history.rs`
  - 暴露 experiment compare 视图
- `apps/code-agent/src/frontend/tui/commands.rs`
  - 解析 `/benchmark`、`/improve`、`/experiment`
- `apps/code-agent/src/frontend/tui/history.rs`
  - 展示 baseline、candidate、delta、decision
- `apps/code-agent/src/frontend/tui/mod.rs`
  - 路由 meta-agent operator flow

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
5. `feat(meta): add baseline compare and relative promotion core`
   - 优先建立 candidate vs baseline 语义
6. `feat(evals): add repo-local coding benchmark pack`
   - 先保证 benchmark 稳定
7. `feat(meta): add isolated worktree candidate runner`
   - 让 `CodePatchVariant` 真正进入闭环
8. `feat(code-agent): add offline benchmark/improve operator surface`
   - 最后改 host app
9. `feat(meta): derive critic from benchmark failures`
   - 在闭环稳定后再接自动诊断

这样切的好处是：

- 每个 commit 的职责单一
- 每个 commit 都有清晰测试边界
- 若 `meta` 设计调整，不会污染 `types/store/evals` 的基础层
- code-agent 关键路径会先收敛在 baseline compare、benchmark、worktree runner
