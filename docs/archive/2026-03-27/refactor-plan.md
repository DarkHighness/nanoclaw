# 结构收敛与模块化重构计划

Date: 2026-03-27

## 背景

这份计划用于回应今日报告中提出的三个判断：

- 结构过于发散
- 模块化不足
- 超大型文件数量过多，且没有按职责合理拆分

本计划基于当前仓库代码抽样、超大文件统计，以及多名 subagent 的并行调研结果整理。结论不是“crate 太多”，而是“少数神文件承载了过多职责，crate 边界没有继续细化到模块边界”。

本次抽样得到的直接信号如下：

- Rust 源文件总行数约 `47,159`
- `>=500 LOC` 的 Rust 文件有 `30` 个
- `>=1000 LOC` 的 Rust 文件有 `9` 个
- `>=1500 LOC` 的 Rust 文件有 `5` 个

热点主要集中在：

| 区域 | 总行数 | Rust 文件数 | `>=500 LOC` 文件数 |
| --- | ---: | ---: | ---: |
| `crates/tools` | 17,854 | 42 | 13 |
| `crates/memory` | 5,996 | 13 | 4 |
| `crates/runtime` | 4,526 | 21 | 1 |
| `crates/sandbox` | 3,379 | 14 | 2 |
| `apps/reference-tui` | 3,001 | 7 | 2 |
| `apps/code-agent` | 2,630 | 7 | 3 |
| `crates/provider` | 2,475 | 7 | 2 |

当前最突出的超大文件包括：

| 文件 | 行数 | 当前混合职责 |
| --- | ---: | --- |
| `crates/runtime/src/runtime.rs` | 2416 | turn loop、tool approval、hooks、continuation、compaction、run event |
| `crates/tools/src/web/search.rs` | 2304 | 输入输出 schema、backend 装配、HTTP 调用、过滤分页、catalog、render |
| `crates/memory/src/memory_embed.rs` | 2144 | 索引维护、embedding 同步、查询扩展、召回融合、排序、snippet |
| `crates/tools/src/code_intel/lsp.rs` | 1562 | runtime、session、LSP 协议适配、命令解析 |
| `apps/reference-tui/src/app.rs` | 1492 | 终端生命周期、命令分发、observer、approval、格式化 |
| `crates/tools/src/process/bash.rs` | 1435 | DTO、session 状态、执行控制、输出切窗、render |
| `crates/provider/src/openai.rs` | 1219 | provider bootstrap、Responses/Realtime transport、payload codec、错误映射 |
| `apps/reference-tui/src/boot.rs` | 934 | boot 编排、provider/sandbox/runtime 装配、startup summary |
| `apps/code-agent/src/main.rs` | 715 | CLI、config/env、runtime 组装、plugin/skill/tooling 装配 |
| `crates/sandbox/src/network_proxy.rs` | 788 | proxy 管理、accept loop、SOCKS 协商、relay |

## 核心判断

### 1. 主要问题是“文件级职责塌缩”，不是 crate 数量失控

`runtime`、`tools`、`memory`、`provider`、两个 host app 的 crate 划分总体合理，但很多 crate 内部仍停留在单文件集中编排阶段。结果是：

- facade 看起来存在，真正的职责却仍然聚集在一个 `impl` 中
- 文件已经按目录切开，但协议映射、状态机、IO 和 render 没有继续拆开
- host 层的启动编排在多个 app 中重复实现

### 2. 状态型子系统普遍把“策略”和“执行”绑在一起

最典型的例子是：

- `runtime.rs` 同时知道 transcript、approval、tool execution、provider continuation 和 compaction
- `bash.rs` 同时知道工具协议、session 生命周期和输出渲染
- `memory_embed.rs` 同时知道 sidecar 生命周期和检索排序策略
- `openai.rs` 同时知道 transport、payload 编码和 provider 错误分类
- `sandbox` 同时知道 backend probing、proxy attach 和最终 `Command` 准备

这类文件难以渐进演化，也不利于给关键不变量补充必要注释。

### 3. 当前最值得做的是“无行为变化的内聚拆分”

现阶段不应该先做大规模 crate 重组，也不应该先抽象新的通用框架。优先顺序应当是：

1. 把纯函数、DTO、格式化和协议映射从超大文件中拿出来。
2. 把子流程抽成内部模块，但保留现有 facade 与 public API。
3. 在行为稳定后，再合并重复 boot 路径或推广 registry 化方案。

## 重构目标

本轮重构建议以四个可衡量目标收束：

1. 将 `>=1000 LOC` 的 Rust 文件从 `9` 个降到 `3` 个以内。
2. 将 `>=500 LOC` 的 Rust 文件从 `30` 个降到 `15` 个以内。
3. 将 host boot 的 runtime/provider/sandbox/tooling 组装路径收敛为一条共享装配链。
4. 让 `runtime`、`memory`、`provider`、`sandbox` 的状态型子系统都形成“单一 facade + 明确内部模块”的结构。

## 2026-03-27 已落地对齐项

自本计划形成后，仓库已经落下七次实现提交：

- `71e4bc3` `refactor(internals): extract helper submodules`
- `750beef` `fix(reference-tui): finalize app module extraction`
- `37ca0cb` `refactor(mcp): move stdio fixture under tests`
- `9dae2ec` `refactor(code-agent): extract options and provider modules`
- `4ea26a6` `refactor(reference-tui): extract run history commands`
- `1303df2` `refactor(reference-tui): split catalog and mcp commands`
- `b224d99` `refactor(reference-tui): extract live render observer`

这七次提交完成的是“无行为变化的内聚拆分”第一批切片，而不是更深层的 crate 重组。它们与本计划的对齐关系如下。

### 已完成的切片

#### 1. `provider/openai` 已对齐到 Wave 5 的第一步

已落地：

- `crates/provider/src/openai/payload.rs`
- `crates/provider/src/openai/message_codec.rs`

当前状态：

- `openai.rs` 从计划基线中的 `1219 LOC` 收敛到当前 `751 LOC`
- request payload 组装与 `MessagePart` 编码已从 transport 主文件中抽离
- tool call / reasoning / message id 的协议解析已从主文件中抽离

已验证：

- `cargo test -p provider` 通过

仍未完成：

- `Responses SSE` 与 `Realtime WebSocket` transport 仍在 `openai.rs`
- `error.rs` 独立化和 transport 进一步拆分还未开始

#### 2. `reference-tui/app` 已对齐到 Wave 1 的后段

已落地：

- `apps/reference-tui/src/app/approval.rs`
- `apps/reference-tui/src/app/presenters.rs`
- `apps/reference-tui/src/app/commands/session.rs`
- `apps/reference-tui/src/app/commands/runs.rs`
- `apps/reference-tui/src/app/commands/catalog.rs`
- `apps/reference-tui/src/app/commands/mcp.rs`
- `apps/reference-tui/src/app/observer.rs`
- `apps/reference-tui/src/app/run_history.rs`

当前状态：

- `app.rs` 从计划基线中的 `1492 LOC` 收敛到当前 `301 LOC`
- tool approval 交互与会话级审批缓存已独立为 `approval.rs`
- sidebar / transcript / run summary / MCP 文本呈现逻辑已独立为 `presenters.rs`
- `apply_command` 已按 session / runs / catalog / mcp 四个命令域拆成 `commands/*`
- 实时 streaming observer 与 run history helper 已从 UI 主文件中抽离

已验证：

- `cargo test -p reference-tui` 通过

仍未完成：

- terminal 生命周期与键盘事件循环仍在 `app.rs`
- `terminal.rs` 仍未抽出，run 入口与 UI loop 还没有进一步收口

#### 3. `core/plugin_boot` 已对齐到 Wave 3 的第一步

已落地：

- `crates/core/src/plugin_boot/drivers.rs`
- `crates/core/src/plugin_boot/driver_env.rs`
- `crates/core/src/plugin_boot/background_sync.rs`

当前状态：

- `activate_driver_requests` 已从“单文件内联所有驱动逻辑”收敛为 `dispatch + driver-specific activator`
- driver env secret materialization 与 memory background sync 已独立出主文件
- `plugin_boot.rs` 当前收敛到 `188 LOC`

已验证：

- `cargo test -p agent` 通过

仍未完成：

- 仍然是 host-coded driver dispatch，不是 registry-driven compiled driver registry
- builtin root 解析与 activation plan 还没有进一步拆为更细模块

#### 4. `mcp` 测试边界已对齐到 Wave 0 的护栏要求

已落地：

- `crates/mcp/tests/support/stdio_fixture.rs`
- `crates/mcp/src/bin/test_stdio_server.rs`

当前状态：

- 原先放在 `src/bin/test_stdio_server.rs` 里的测试夹具服务逻辑，已迁回 `tests/support`
- `src/bin/test_stdio_server.rs` 现在只保留供 `env!(CARGO_BIN_EXE_test_stdio_server)` 启动的极薄 shim
- `stdio_integration.rs` 中的 recording executor 已对齐当前 `sandbox::ProcessExecutor` 契约

已验证：

- `cargo test -p mcp` 通过

仍未完成：

- 这条“测试夹具归 `tests`、运行时入口不承载测试实现”的边界规则，还没有系统化检查覆盖到整个仓库

#### 5. `apps/code-agent` 已对齐到 Wave 1 的第一步

已落地：

- `apps/code-agent/src/options.rs`
- `apps/code-agent/src/provider.rs`

当前状态：

- `main.rs` 从当前切片前的 `715 LOC` 收敛到 `468 LOC`
- CLI 参数解析、环境装载、provider 选择与 help 输出已从启动主文件移入 `options.rs`
- provider 默认模型、API key 可用性检查与 backend 构造已从主文件移入 `provider.rs`
- `main.rs` 现在主要保留 tracing 初始化、runtime facade、tool/runtime 装配与 plugin/skill 协调

已验证：

- `cargo test -p code-agent` 通过

仍未完成：

- `build_runtime` 仍然同时承担 tool registry、hook runner、skill/plugin 汇总与 subagent 装配
- `build_plugin_activation_plan`、`build_system_preamble` 与 skill root 解析还在 `main.rs`

### 本轮实施后的结构变化

以计划形成时的基线为参照，本轮切片带来的直接变化是：

- `>=1000 LOC` 的 Rust 文件从 `9` 个降到 `7` 个
- `>=1500 LOC` 的 Rust 文件仍为 `5` 个
- `>=500 LOC` 的 Rust 文件从 `30` 个降到 `29` 个
- `mcp` 的 stdio fixture 不再作为 `src/bin` 中的实现主体存在，测试辅助逻辑回到 `tests` 边界内
- `apps/code-agent/src/main.rs` 已不再属于 `>=500 LOC` 热点文件

这符合本计划“先消减最粗的千行神文件，再处理 500-1000 LOC 中段文件”的执行顺序。

### 当前仍然保持不变的重点

本轮没有触碰、也仍然是后续重点的部分包括：

- `crates/runtime/src/runtime.rs`
- `crates/tools/src/web/search.rs`
- `crates/memory/src/memory_embed.rs`
- `crates/tools/src/process/bash.rs`
- `apps/reference-tui/src/boot.rs` 与 `apps/code-agent/src/main.rs` 的共享 boot 收敛

因此，本计划后续的主线仍然有效：先继续做内部子模块抽离，再合并宿主 boot 路径，最后处理 `runtime` / `memory` / `sandbox` 的高状态密度核心。

## 重构原则

- 先拆内部模块，再谈 crate 重组；不要用新的顶层抽象掩盖现有职责混杂。
- 每次只处理一个大文件或一个明确子域，避免在同一批改动里同时做去重和行为变更。
- 优先抽出纯函数、DTO、render、protocol codec；这些部分边界最清晰、回归风险最低。
- `runtime` 的 append-only transcript、tool approval、loop detection 边界必须保持集中控制。
- `sandbox` 的 fail-closed 语义必须保留单一决策点，不能在拆分后稀释。
- 当模块边界变化影响 subsystem 理解成本时，要同步补充最近的设计说明或 crate doc。

## 工作流

### Wave 0: 建立护栏

目标：先为重构建立低风险路径，不在第一步就扩大回归面。

- 为目标文件补足当前行为的测试覆盖或快照用例，尤其是 `runtime`、`bash`、`memory`、`sandbox`。
- 为每个目标文件先写清楚“拆分后仍由谁拥有最终决策权”。
- 在不改公开接口的前提下，先移动纯 helper、DTO 和 formatter。
- 测试夹具、集成测试辅助逻辑和仅供测试使用的服务实现默认放在 `tests/`；如果 Cargo 必须保留可执行 target，则 `src/bin` 只保留最薄的启动 shim。

输出物：

- 文件级拆分任务单
- 回归测试清单
- 关键不变量清单

### Wave 1: 宿主层先收敛

目标：先拿低风险、高可见度的 host 层做切口。

#### `apps/reference-tui`

状态：已大部分落地（`commands/*`、`observer.rs`、`approval.rs`、`presenters.rs`、`run_history.rs` 已抽出）

建议拆分：

- `app.rs` -> `terminal.rs`
- `app.rs` -> `commands/session.rs`
- `app.rs` -> `commands/runs.rs`
- `app.rs` -> `commands/catalog.rs`
- `app.rs` -> `commands/mcp.rs`
- `app.rs` -> `observer.rs`
- `app.rs` -> `approval.rs`
- `app.rs` -> `presenters.rs`
- `app.rs` -> `run_history.rs`

先做：

- 先从 `apply_command` 的大 `match` 开始，按命令域拆开。
- 再把 sidebar/transcript/run summary 等文本格式化抽到 `presenters.rs`。
- 最后把实时 observer 和 approval handler 移出 UI 主文件。

#### `apps/code-agent`

建议拆分：

- `main.rs` -> `cli.rs`
- `main.rs` -> `options.rs`
- `main.rs` -> `runtime_factory.rs`
- `main.rs` -> `provider.rs`
- `main.rs` -> `plugins.rs`
- `main.rs` -> `tool_registry.rs`

先做：

- 先把 CLI/options 和 provider 选择逻辑抽掉。
- 保留 `async_main` 作为 facade，避免第一步就打散启动流程。

### Wave 2: 工具层按“请求管线”和“状态服务”拆开

目标：让 `tools` 从“大工具文件”收敛为“薄工具 facade + 内部服务模块”。

#### `crates/tools/src/web/search.rs`

建议拆分：

- `request.rs`
- `backend_bootstrap.rs`
- `transport.rs`
- `pipeline.rs`
- `present.rs`
- `catalog.rs`
- `backends_tool.rs`

先做：

- 优先抽 `domains`、`freshness`、`result/source` 生成、分页和展示逻辑到 `pipeline.rs` / `present.rs`。
- 第二步再把 backend 装配和 HTTP 传输搬走。
- 最后把 `web_search_backends` 的 catalog surface 独立出去。

#### `crates/tools/src/process/bash.rs`

建议拆分：

- `protocol.rs`
- `session.rs`
- `service.rs`
- `render.rs`

先做：

- 提取共享的 `ResolvedBashInvocation` / `build_exec_request(...)`。
- 把 DTO 和 render 逻辑抽离，保留执行控制和 session 状态机在原文件。
- 在 poll/cancel 语义有独立测试后，再拆 session/service。

#### `crates/tools/src/code_intel/lsp.rs`

建议拆分：

- `runtime.rs`
- `session.rs`
- `protocol.rs`
- `commands.rs`

先做：

- 先把协议辅助与命令解析从 `ManagedLspRuntime` 中剥离。
- 避免第一步就同时移动 session 生命周期和消息协议。

### Wave 3: 收敛共享 boot 与 plugin activation

目标：减少 host app 间重复装配，并先处理边界天然的 `plugin_boot`。

#### `crates/core/src/plugin_boot.rs`

状态：已部分落地（`drivers.rs`、`driver_env.rs`、`background_sync.rs` 已抽出）

建议拆分：

- `plugin_roots.rs`
- `activation_plan.rs`
- `drivers/mod.rs`
- `drivers/memory_core.rs`
- `drivers/memory_embed.rs`
- `driver_env.rs`
- `background_sync.rs`

先做：

- 先把 `activate_driver_requests` 变成 `dispatch + driver-specific activator`。
- 再分离 builtin root 解析和 activation plan 生成。

#### 宿主 boot 去重

重复热点主要在：

- `apps/reference-tui/src/boot.rs`
- `apps/code-agent/src/main.rs`

建议在 `crates/core` 收敛共享装配路径：

- provider backend 构造
- sandbox policy 构造
- runtime preamble / base instructions
- plugin activation plan
- tool registry 装配

注意：

- 不要在 Wave 1 尚未收口时就提前合并全部 boot 路径。
- 先把重复逻辑提取为内部 builder，再决定是否暴露更高层公共接口。

### Wave 4: 拆 `runtime` 的 turn orchestration

目标：让 `AgentRuntime` 回到“编排者”而不是“全知实现体”。

#### `crates/runtime/src/runtime.rs`

建议拆分：

- `turn_bootstrap.rs`
- `turn_stream.rs`
- `turn_stop.rs`
- `tool_dispatch.rs`
- `tool_approval.rs`
- `tool_executor.rs`
- `tool_failure.rs`
- `history_window.rs`
- `provider_state.rs`

先做：

- 第一阶段只移动 helper 与纯子流程，不改 `AgentRuntime` 的公开入口。
- 第二阶段把 `handle_tool_call` 周围逻辑拆成 approval / execution / failure 三层。
- 第三阶段再处理 transcript 可见窗口、continuation 游标和 compaction 状态。

必须保留的集中控制点：

- append-only transcript 追加
- approval 最终裁决
- loop detection 阻断
- turn 结束条件

### Wave 5: 处理高状态密度后端

目标：拆掉最难维护的“策略 + IO + 缓存 + 协议”混合文件。

#### `crates/memory/src/memory_embed.rs`

建议拆分：

- `index_sync.rs`
- `chunk_index_sync.rs`
- `lifecycle_store.rs`
- `search_service.rs`
- `embedding_payload.rs`
- `query_expansion.rs`
- `ranking.rs`

先做：

- 先把 ranking、MMR、temporal scoring、embedding prompt 这类纯逻辑抽出。
- 再拆索引维护与 sidecar 状态写入。
- 最后才拆 `search()` / `sync()` 主流程。

#### `crates/provider/src/openai.rs`

状态：已部分落地（`payload.rs`、`message_codec.rs` 已抽出）

建议拆分：

- `transport.rs`
- `responses_stream.rs`
- `realtime_stream.rs`
- `payload.rs`
- `message_codec.rs`
- `error.rs`

先做：

- 先抽 `payload.rs` 和 `message_codec.rs`。
- 这一步完成后，再把 Responses SSE 和 Realtime WebSocket transport 拆开。

#### `crates/sandbox`

建议拆分重点：

- `manager.rs` 保留最终策略裁决与 `Command` 准备
- `network_proxy.rs` 向 `proxy_manager.rs`、`accept_loop.rs`、`protocol.rs`、`relay.rs` 收敛

先做：

- 先把 proxy 协议解析和 relay 细节移出 `network_proxy.rs`。
- `manager.rs` 只保留 backend probing、policy 解释、最终 attach 决策。

## 建议的执行顺序

按风险和收益排序，建议采用以下顺序：

1. `apps/reference-tui/src/app.rs` 的命令拆分与 presenter 抽离
2. `crates/tools/src/web/search.rs` 的 pipeline / present 抽离
3. `crates/provider/src/openai.rs` 的 payload / message codec 抽离
4. `crates/core/src/plugin_boot.rs` 的 driver dispatch 拆分
5. `apps/code-agent/src/main.rs` 与 `apps/reference-tui/src/boot.rs` 的共享 boot 收敛
6. `crates/runtime/src/runtime.rs` 的 tool dispatch 和 turn flow 拆分
7. `crates/tools/src/process/bash.rs` 与 `crates/tools/src/code_intel/lsp.rs` 的状态服务拆分
8. `crates/memory/src/memory_embed.rs` 与 `crates/sandbox` 的高状态密度拆分

## 风险与约束

- 最大风险是行为漂移，不是编译失败。`runtime`、`bash`、`memory`、`sandbox` 都有显式状态机或关键策略边界。
- 不要在同一批改动里同时做“模块拆分 + 语义修正 + 路径去重”。
- `runtime` 和 `sandbox` 的控制点必须继续集中，否则 append-only 和 fail-closed 语义会被削弱。
- `provider` 和 `memory` 要按协议层 / transport 层 / 排序层切，不要按“代码量平均分块”切。
- 每完成一个 meaningful slice，就补对应设计说明，避免再次形成“代码已经变了，文档仍停留旧状态”的问题。

## 完成定义

当下面四项同时成立时，可以认为本轮“结构收敛”基本完成：

1. 主要神文件被拆成 facade + 内部模块，且核心语义没有漂移。
2. 两个 host app 的启动装配不再各自复制完整链路。
3. `runtime`、`tools`、`memory`、`provider`、`sandbox` 的关键模块边界可在 crate doc 或设计说明中直接解释清楚。
4. 代码审阅时不再需要从单个千行文件逆向理解整条控制流。
