# 项目进展归档报告

Date: 2026-03-27

## 补充文档

- `docs/archive/2026-03-27/refactor-plan.md`

## 归档范围

本归档基于以下材料整理当前项目进展：

- `docs/plan.md`
- `docs/sandbox-design.md`
- `docs/archive/2026-03-26/design.md`
- `docs/archive/2026-03-26/plan.md`
- `docs/archive/2026-03-26/memory-plugin-design.md`
- `docs/archive/2026-03-26/plugin-system-design.md`
- `git log` 自 `2026-03-25` 启动提交 `f48b0aa` 至当前 `HEAD` `a38e352`

为避免沿用已过期结论，本次归档还抽查了当前实现中的两个关键边界：

- `crates/core/src/plugin_boot.rs`
- `crates/memory/src/state.rs`

## 结论摘要

`nanoclaw` 已经明显越过“原型搭架子”阶段，进入“可运行的 agent substrate 持续强化”阶段。`2026-03-26` 的文档复盘已经把仓库判断为“主体能力已实现”，而之后到 `HEAD` 的提交主要是在补齐当时列出的差距，尤其集中在 `memory` 与 `sandbox`。

截至 `a38e352`，项目的核心状态可以概括为：

- 基础分层已经成型：`types`、`runtime`、`tools`、`skills`、`provider`、`mcp`、`store`、`core(agent)` 与 `apps/` 的边界清晰。
- 可运行宿主已经具备：`reference-tui` 作为参考壳层，`code-agent` 作为 codex-like 产品化示例。
- 核心运行时能力已经具备：append-only transcript、context compaction、tool approval、loop detection、persistent run store、structured tool outputs。
- 可选能力束已经落地：web retrieval、managed LSP code-intel、memory plugins、sandbox backend、plugin planning。
- 最近的增量工作不是“补 demo”，而是围绕性能、索引持久化、边界收敛与工程化稳定性持续打磨。

## 阶段性进展

### 1. 2026-03-25：基础仓库与运行闭环建立

`2026-03-25` 的提交序列完成了项目的第一轮骨架搭建：

- `f48b0aa` 到 `c2786da` 建立了 workspace、协议类型、runtime orchestration、provider/integration crates 与独立 reference shell。
- `ca2ee74` 增加了 `code-agent` 示例，说明仓库目标已经不是单一 TUI，而是“substrate + host apps”的产品组合。
- `b277238`、`94d32e1`、`76e63e0` 把本地文件工具与 `bash` 会话能力从基础版推进到更严格的 grounded contract。
- `09dcda1` 提前把 code-intel 放到 feature-gated optional bundle，说明工具层从一开始就按“最小核心 + 可选扩展”设计。

这一阶段的结果是：项目完成了从零到可运行 substrate 的第一跳，核心抽象并非停留在设计文档。

### 2. 2026-03-26：从可运行走向工程化与模块化

`2026-03-26` 是目前最密集的一天，主要完成了四类工作：

- 运行时和工具契约深化：`5e137e0`、`6452945`、`093613c`、`d18e61b`、`422b440`、`256ffc8`、`0c847c7`、`a44b4d6`
- 边界和宿主结构调整：`6da4eb5`、`14642ec`、`524f017`
- 能力束扩展：web、code-intel、plugin、memory、sandbox
- 文档状态校准：`6ac0079`、`be968c8`、`a9d15f7`、`d4592f1`

其中最关键的进展有：

- runtime 已支持 provider-managed continuation、approval policy composition 与 canonical tool lifecycle events。
- tool 层已不是简单文本接口，而是逐步转向带结构化输出、来源注释、受控 mutation contract 的统一 surface。
- web 工具从单一路径演进为 backend registry + catalog + source annotations + freshness/news mode。
- sandbox 从 `tools::process` 内部细节提升为独立边界，并逐渐影响到文件工具与进程工具的共同约束。
- plugin 与 memory 不再只是概念稿，而是已经进入 host boot、slot planning、driver activation 与工具注册链路。

### 3. 2026-03-26 晚段到 2026-03-27：开始回填文档复盘中暴露的差距

`docs/plan.md` 在 `2026-03-26` 明确记录了几个“已实现但未完全对齐原设计”的点，其中最重要的是：

- `memory-core` 仍是简化版实现
- plugin driver activation 仍由 host 侧 `match` 控制
- sandbox abstraction 比设计稿更窄

后续提交显示项目已经在主动处理其中两项：

- `2f88339`、`e9bab54`、`90fb5e0`、`af88269`、`2c11df8`、`a5bfdb4`、`a38e352` 明确把 `memory-core`/`memory-embed` 推向“持久化词法索引 + sidecar 复用 + 增量刷新 + temporal scoring”方向。
- `8dad2c6`、`6460acc`、`247a0a3`、`9ae6f51` 则持续强化 sandbox：抽离顶层 crate、启动期探测 backend、统一受保护路径、避免无后端时错误假设。

这说明文档复盘不是静态结论，而是已经成为后续提交的修正清单。

## 模块状态判断

### Runtime / Core

项目的 runtime/core 基本进入“可复用基础设施”状态，而不是高频改接口阶段。当前重点更像是围绕 provider continuation、tool lifecycle、workspace state、config split 做细节收口。

### Tools / Structured IO

工具层是当前仓库最成熟的公共面之一。文件工具、`bash`、web、agentic、code-intel 都已向结构化输出收敛，这对后续 host 侧渲染、审计、回放与跨 provider 降级都很重要。

### Web / Code-Intel

web 工具线在 `2026-03-26` 完成了从“可搜”到“可选择 backend、可标注来源、可解释 freshness 行为”的跃迁。code-intel 也已经从单点能力扩展为 managed LSP-backed bundle，并覆盖更多符号查询场景。

### Memory

`memory` 是最近最活跃的主线，也是最能体现项目从设计走向工程化的一块。根据 `docs/archive/2026-03-26/memory-plugin-design.md`，原始目标是 `memory-core` 与 `memory-embed` 共用 Markdown source-of-truth、通过 memory slot 激活、并提供统一工具接口。

结合后续提交和当前 `crates/memory/src/state.rs` 中的状态路径常量，可以判断这条线已经明显前进到：

- workspace-local `.nanoclaw/memory` 状态布局已稳定
- `memory-core` 已拥有持久化 lexical sqlite index
- `memory-embed` 与 lexical sidecar 的共享和增量刷新已经落地
- embedding 复用、去重、snapshot drift 刷新与 daily log temporal scoring 已开始优化

因此，`docs/plan.md` 中“`memory-core` 仍然是简化实现”的判断，到了当前 `HEAD` 已经不再完全成立。更准确的说法应当是：这条差距正在被快速回填，而且已经跨过“纯内存简化版”阶段。

### Sandbox

`docs/sandbox-design.md` 把 `crates/sandbox` 定义为 canonical boundary，这一点已经与近期提交高度一致。当前 sandbox 线的特点不是继续扩展抽象层数，而是把边界真正落到：

- backend availability probing
- allow-domains enforcement
- protected path enforcement
- file tools 与 process tools 的统一策略

这说明项目对 sandbox 的取向更偏“工程可执行性与 fail-closed 行为”，而不是继续追求更宽泛的抽象设计。

### Plugins

plugin planning、slot selection、builtin plugin roots 与 memory driver activation 都已经在运行链路中。但 `crates/core/src/plugin_boot.rs` 仍保留对 `builtin.memory-core` / `builtin.memory-embed` 的显式 `match`，说明 `docs/plan.md` 提到的“driver activation 仍非 registry-driven”这一点截至当前仍然成立。

换句话说，plugins 已经可用，但还没有彻底走到“完全泛化驱动注册表”的阶段。

## 与文档基线的对齐情况

### 已被证明基本落地

以下方向已不再只是设计目标，而是仓库中的既有能力：

- foundation 和 app workspace 分离
- host boot 负责装配 runtime，而不是把 config/TUI 混入 foundation
- append-only transcript 与 context compaction
- persistent run store 与 replay/search
- plugin planning 与 memory slot
- structured tool output 与 tool lifecycle events
- web/search backend boundary
- sandbox 统一边界

### 已被后续提交主动修正

- `memory-core` 过于简化的问题正在被系统性修正
- sandbox 可用性与受保护路径规则已经比 `2026-03-26` 复盘时更完整

### 仍然值得保留为后续工作

- plugin driver activation 仍需从 host-level `match` 过渡到更通用的 compiled driver registry
- 当前顶层文档仍有历史遗留痕迹；例如部分旧链接与“现状说明”没有完全和归档后的文档结构同步

## 当前阶段结论

目前最准确的阶段判断是：

> 核心 substrate 已完成，仓库正在从“功能补齐”转向“工程收敛 + 能力深化”。

更具体地说：

- 核心运行闭环已经完成，后续不会再回到“大面积缺失基础能力”的状态。
- `memory` 与 `sandbox` 是最近最强的推进线，说明项目重点正在从“能不能做”转向“能否稳定、可复用、可解释地做”。
- web、code-intel、plugin、run store 这些外围面已具备继续产品化的条件。
- 当前最明显的剩余架构债是 plugin driver generalization 与文档层的进一步收口。

## 建议的下一步归档方向

如果后续继续做进展归档，建议优先跟踪这四件事：

1. plugin driver registry 是否替代当前 host-coded activation。
2. `README.md` 与当前归档文档结构是否完成同步清理。
3. `memory-core` 与 `memory-embed` 的职责边界是否进一步稳定为长期方案。
4. sandbox 的 backend matrix 与 fail-closed 行为是否完成更多回归测试覆盖。
