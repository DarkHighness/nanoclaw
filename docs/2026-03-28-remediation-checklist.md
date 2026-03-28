# 2026-03-28 修复与实施清单

日期：2026-03-28

状态：Active

## 1. 目的

本文档把 2026-03-28 对插件系统、Memory 系统、多 Agent 系统的实现审查，整理成统一的修复与实施清单。

这份清单不替代三条路线各自的详细 Plan，而是作为当前迭代的执行总表，用来回答三个问题：

- 哪些问题必须先修，否则继续堆功能会放大错误。
- 哪些问题属于“与计划不对齐”，但不会立刻造成错误结果。
- 哪些问题主要是性能与工业化收口。

## 2. 审查依据

- 路线文档：
  - `docs/2026-03-28-plugin-system-plan.md`
  - `docs/2026-03-28-memory-system-plan.md`
  - `docs/2026-03-28-multi-agent-plan.md`
  - `docs/2026-03-28-model-config-plan.md`
- 关键代码路径：
  - `crates/plugins`
  - `crates/memory`
  - `crates/runtime`
  - `crates/store`
  - `crates/tools/src/agentic`
  - `apps/reference-tui`
  - `apps/code-agent`
- 已运行验证：
  - `cargo test --manifest-path crates/Cargo.toml -p plugins`
  - `cargo test --manifest-path crates/Cargo.toml -p memory`
  - `cargo test --manifest-path crates/Cargo.toml -p runtime`
  - `cargo test --manifest-path crates/Cargo.toml -p store`
  - `cargo test --manifest-path crates/Cargo.toml -p tools`
  - `cargo test --manifest-path crates/Cargo.toml -p agent`
  - `cargo test --manifest-path apps/Cargo.toml -p reference-tui`
  - `cargo test --manifest-path apps/Cargo.toml -p code-agent`

## 3. 当前完成度校准

- 插件系统：约 `78%`
- Memory 系统：约 `85%`
- 多 Agent 系统：约 `65% ~ 70%`

结论：

- 三条路线都已经从“设计稿”进入“可运行原型”。
- 目前最大的风险不是缺功能，而是：
  - 权限和协议边界还不够硬
  - 多 Agent / Memory 的并发正确性还不够稳
  - 若继续扩功能，后续返工成本会明显上升

## 4. 优先级定义

- `P0`
  - 正确性、安全性、协议一致性问题
  - 不修会直接导致错误结果、越权、挂死或数据损坏
- `P1`
  - 与计划核心目标不对齐
  - 当前能运行，但不是目标模型
- `P2`
  - 性能、可观测性、工业化收口
  - 不会马上出错，但会限制规模化使用

## 5. 执行顺序

建议按下面顺序推进，而不是按子系统各自闭门做完：

1. 先修 `P0 正确性与安全边界`
2. 再补 `P1 计划对齐缺口`
3. 最后做 `P2 性能与硬化`

原因：

- 多 Agent 的等待与终态问题，会直接影响 Memory runtime export 的正确性。
- 插件权限边界不收紧，后面补更多 WASM host API 只会扩大攻击面。
- Memory 的并发写和导出链不修，多 Agent 的 coordination/episodic 联动会继续写错。

## 6. P0 修复清单

### 6.1 多 Agent

- 当前分支已完成：
  - child 非终态结果归一化，避免自然结束后把 agent 留在 `Queued/Running/Waiting*`
  - `AgentSessionManager::wait()` 的无丢通知修复
  - `send / wait / cancel / list` 的 parent-child scope 收紧
  - batch spawn 的全量 preflight + write lease 预申请，失败时不留下部分已启动 child

- 修复 child 终态收敛：
  - 子代理自然结束后，不能把 `Queued/Running/Waiting*` 写回 `AgentResultEnvelope.status`
  - `finish_success()` 只能接受终态；非终态应被归一化为 `Completed` 或直接判错
  - 目标文件：
    - `crates/runtime/src/subagent_impl.rs`

- 修复 `agent_wait()` 的等待竞态：
  - `AgentSessionManager::wait()` 不能继续使用当前的 “snapshot -> notified().await” 结构
  - 改成无丢通知语义的等待模型
  - 可选实现：
    - 先拿 `notified()` future，再 snapshot，再 await
    - 或直接换成 `watch` / `broadcast` / 条件变量式版本号
  - 目标文件：
    - `crates/runtime/src/agent_session_manager.rs`

- 加父子作用域校验：
  - `agent_send`
  - `agent_wait`
  - `agent_cancel`
  - 都必须验证调用方是否有权操作目标 child
  - 顶层 root runtime 是否允许全局查看，需要单独定义，不应默认放开所有控制面
  - 目标文件：
    - `crates/runtime/src/subagent_impl.rs`
    - `crates/tools/src/agentic/task.rs`

- 修复批量 spawn 的部分成功副作用：
  - 要么先做全量 preflight：
    - tool resolution
    - write lease claim
    - task validation
  - 要么在中途失败时回滚前面已启动 child
  - 目标文件：
    - `crates/runtime/src/subagent_impl.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - 先完成全部子任务的 tool resolution / task normalization / write set 解析
    - 再统一申请本批次所有 write lease
    - 任一 claim 失败时释放已申请 lease，并且不创建任何 child session / worker
    - claim 成功后先写入生命周期事件，再启动 child runtime

### 6.2 Memory

- 当前分支已完成：
  - `RunStore::export_for_memory()` 现在会真实聚合 `run/session/subagent/task`
  - `InMemoryRunStore` / `FileRunStore` 都补了真实 `subagent/task export` 测试
  - `memory_record` 对同一 managed file 的追加改为路径级串行化
  - `memory_list.include_stale` 现在会真实控制非 `ready` 记录可见性
  - 非 ASCII `task_id` 的 working task 路径已回退到稳定 hash slug

- 补齐 production 级 `subagent/task` runtime export：
  - `RunStore::export_for_memory()` 必须真正聚合：
    - `TaskCreated`
    - `TaskCompleted`
    - `SubagentStart`
    - `SubagentStop`
    - `AgentEnvelope::*`
  - 不能只让 `runtime_exports.rs` 支持渲染，但 store 侧不产出记录
  - 目标文件：
    - `crates/store/src/traits.rs`
    - `crates/store/src/memory.rs`
    - `crates/store/src/file.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - 共享分组 helper 已按 `TaskCreated / TaskCompleted / SubagentStart / SubagentStop / AgentEnvelope::*` 聚合
    - `run/session/subagent/task` 四类记录都进入真实 store 导出链
    - file-backed store 已有真实事件回放测试，不再只靠 fixture bundle

- 修复 `memory_record` 并发丢写：
  - `working` / `coordination` 同文件写入不能继续使用裸 `read-modify-write`
  - 至少实现文件级串行化，或版本校验失败后重试
  - 目标文件：
    - `crates/memory/src/managed_files.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - `memory_record` 现在在目标 managed path 上先拿锁再读写
    - 单进程内并发追加不会再因为同一旧快照而互相覆盖

- 修复 `memory_list.include_stale` 语义：
  - 要么真正执行 `stale/superseded/archived` 过滤
  - 要么删掉该参数，避免虚假语义
  - 目标文件：
    - `crates/memory/src/retrieval_policy.rs`
    - `crates/memory/src/tools.rs`
    - `crates/memory/src/memory_core.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - `include_stale = false | None` 时只返回 `ready`
    - `include_stale = true` 时显式放开 `stale/superseded/archived`
    - retrieval policy、backend、tool 三层都已补测试

- 修复非 ASCII `task_id` 的 working 路径生成：
  - `slugify(task_id)` 为空时必须回退到稳定文件名策略
  - 不能生成 `.../tasks/.md`
  - 目标文件：
    - `crates/memory/src/managed_files.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - working task 文件名会回退到 `task-<stable-hash>`，避免空 slug 路径

### 6.3 插件系统

- 当前分支已完成：
  - WASM hook 不会再因为 handler 类型是 `wasm` 就自动获得 gate 权限
  - `prompt` / `agent` hook 默认改为 fail-closed
  - host app 的默认 HookRunner wiring 已同步切到 fail-closed evaluator
  - `message_mutation = review_required` 现在会在 activation 阶段直接判为不支持

- 收紧 WASM hook 的 gate 权限：
  - `allow_gate_decision` 不能因为 `handler_kind == Wasm` 就自动放开
  - gate/permission 相关 effect 必须绑定 capability + granted permission
  - 目标文件：
    - `crates/plugins/src/resolution.rs`
    - `crates/runtime/src/runtime/hook_effects.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - `allow_gate_decision` 现在只来自显式 `Gate` capability
    - WASM handler 若要实际发出 effect，仍必须具备 `emit_hook_effect` host API grant
    - activation plan 已补测试，覆盖“无 capability 不放开 / 有 capability 才放开”

- `prompt` / `agent` hook 未实现前必须 fail-closed：
  - 当前 silent noop 会制造“配置成功但没有效果”的假象
  - 在未实现前，注册时或执行时必须显式报错
  - 目标文件：
    - `crates/runtime/src/hooks/handlers/prompt.rs`
    - `crates/runtime/src/hooks/handlers/agent.rs`
    - `crates/runtime/src/hooks/runner.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - `prompt` / `agent` evaluator 默认返回 hook error
    - 默认 HookRunner 与 host app wiring 都已切到 fail-closed evaluator
    - handler 单测与 runner 集成测试都已补上

- 明确 `ReviewRequired` 的语义：
  - 如果当前没有 host review 流程，就不要把它伪装成可用能力
  - 目标文件：
    - `crates/plugins/src/resolution.rs`
    - `crates/plugins/src/lib.rs`
  - 状态：
    - `completed`
  - 已落地语义：
    - `message_mutation = review_required` 在 manifest 或 resolver grant 中都会触发 activation diagnostic
    - 插件会在 activation plan 阶段被禁用，而不是留到运行时再失败

## 7. P1 计划对齐清单

### 7.1 多 Agent

- 让 `dependency_ids` 真正进入调度：
  - 状态：
    - `completed`
  - 已落地语义：
    - `task_batch` / `agent_spawn` 已不再是无条件 fan-out
    - batch 内依赖会进入 ready set / blocked set 调度
    - 依赖完成会触发 completion propagation
    - 依赖失败会阻断 downstream child，并产出明确 result envelope
    - 缺失依赖 / 自依赖 / 循环依赖现在都会报错
  - 目标文件：
    - `crates/tools/src/agentic/task.rs`
    - `crates/runtime/src/subagent_impl.rs`

- 统一终态收尾所有权：
  - 当前 manager 和 worker 两边都能结束 child
  - 需要单一 owner 负责：
    - 状态落盘
    - lease release
    - `TaskCompleted`
    - `SubagentStop`
  - 目标文件：
    - `crates/runtime/src/agent_session_manager.rs`
    - `crates/runtime/src/subagent_impl.rs`

- 把模型配置从“单一 provider/model”升级为“模型目录 + agent profile”：
  - 当前是开发版本，这项重构**不要求旧配置兼容**
  - 仓库默认 lane 固定为：
    - `gpt_5_4_default`
    - `openai / gpt-5.4`
    - `400k` working context
    - `128k` max output
    - `320k` compact trigger
  - 目标不是只加字段，而是把：
    - 模型 alias
    - 主 agent / subagent profile
    - role -> profile 路由
    - summary / memory internal profile
    - thinking / system prompt / compact / sandbox 覆盖
    统一落到 resolved config
  - 详细方案见：
    - `docs/2026-03-28-model-config-plan.md`
  - 实施时按文档中的 Phase A -> H 顺序推进，不并行跳步
  - 这项工作横跨：
    - `crates/config`
    - `apps/reference-tui`
    - `apps/code-agent`
    - `crates/runtime/src/subagent_impl.rs`
    - `crates/tools/src/context.rs`
    - `crates/sandbox`

- 补统一 token usage ledger 与展示面：
  - 这是 `P1`，不是单纯的 `P2` 可观测性收尾
  - 最低要求必须统一输出：
    - `context used / limit / trigger`
    - `input`
    - `output`
    - `prefill`
    - `decode`
    - `cache read`
  - 统计口径必须固定为：
    - request build 时估算上下文占用
    - response complete 时归一化 provider usage
    - runtime / store / UI 共用同一份 ledger
  - 详细方案见：
    - `docs/2026-03-28-model-config-plan.md`
  - 这项工作横跨：
    - `crates/types`
    - `crates/runtime`
    - `crates/provider`
    - `crates/store`
    - `apps/reference-tui`
    - `apps/code-agent`

### 7.2 Memory

- 把 runtime export 与多 Agent 主事件打通：
  - 状态：
    - `completed`
  - 已落地语义：
    - `RunStore::export_for_memory()` 已真实导出 `run/session/subagent/task`
    - runtime sidecar 已进入 memory corpus，可被 `memory_search/list/get` 消费
    - `subagent/task` 记录现在是 episodic retrieval 的真实输入，而不是只落盘不参与检索
  - 目标文件：
    - `crates/store`
    - `crates/memory/src/runtime_exports.rs`
    - `crates/memory/src/memory_core.rs`
    - `crates/memory/src/memory_embed.rs`

- 补充 runtime -> memory scope bridge：
  - 状态：
    - `completed`
  - 已落地语义：
    - `ToolExecutionContext` 已携带 `agent_name/task_id`
    - `memory_search/list/record` 默认会继承当前 agent/task 作用域
    - 子代理上下文现在可以直接检索和记录自己对应的 working / episodic memory
  - 目标文件：
    - `crates/tools/src/context.rs`
    - `crates/memory/src/tools.rs`

### 7.3 插件系统

- 打通 `DriverActivationOutcome` 的宿主消费闭环：
  - 状态：
    - `completed`
  - 已落地语义：
    - `hooks / mcp_servers / instructions / diagnostics` 都已进入宿主 build pipeline
    - `reference-tui` 与 `code-agent` 都会消费 driver outcome
    - `code-agent` 新接入的 driver MCP 也已补齐路径解析、去重和沙箱策略对齐
  - 目标文件：
    - `crates/core/src/plugin_boot/registry.rs`
    - `apps/reference-tui/src/boot.rs`
    - `apps/code-agent/src/main.rs`

- 明确消息 mutation 的目标能力：
  - 状态：
    - `completed`
  - 已落地语义：
    - `MessageSelector` 现在支持 `Current`、`MessageId` 与 `LastOfRole`
    - `MessageId` 与 `LastOfRole` 都只允许命中当前可见 transcript，不能改已被 compaction 隐藏的历史
    - `LastOfRole` 只解析已落盘的可见 transcript，不会命中当前 in-flight 的 `Current` 消息
    - 历史 mutation 通过 append-only `TranscriptMessagePatched/Removed` 事件落盘
    - 任意 `MessageId` / `LastOfRole` mutation 都会显式失效 provider continuation
  - 目标文件：
    - `crates/types/src/hook.rs`
    - `crates/runtime/src/runtime/hook_effects.rs`

- 明确 `builtin.wasm-hook-validator` 的职责：
  - 状态：
    - `completed`
  - 已落地语义：
    - 内建项已从 `builtin.wasm-hook-runtime` 更名为 `builtin.wasm-hook-validator`
    - 它只负责 `runtime.module` 与 `exec_roots` 的校验，不再伪装成能提供 runtime contributions 的通用 driver
    - host diagnostic 文案已统一为 `validated wasm hook module ...`
  - 后续若要提供真正的 runtime contributions，应新增独立 executable runtime driver
  - 目标文件：
    - `crates/core/src/plugin_boot/drivers.rs`
    - `docs/2026-03-28-plugin-system-plan.md`

## 8. P2 性能与硬化清单

### 8.1 多 Agent

- 批量 spawn 改成有界并行冷启动
- `WriteLeaseManager` 优化冲突检测数据结构
- store append 降低生命周期事件写放大

### 8.2 Memory

- 把 runtime export materialization 从 `get/list/search` 读路径中拆出
  - 状态：
    - `completed`
  - 已落地语义：
    - `sync()` 仍负责刷新 runtime export sidecar
    - `get/list/search` 改为只读取既有 sidecar 与 lifecycle 统计
    - 读请求不再触发 `export_for_memory()` 或 sidecar 重写
  - 目标文件：
    - `crates/memory/src/runtime_exports.rs`
    - `crates/memory/src/memory_core.rs`
    - `crates/memory/src/memory_embed.rs`
- 给 corpus 扫描增加增量目录快照
- 避免读请求触发不必要的 sidecar 重写

### 8.3 插件系统

- 缓存 WASM `Engine/Module`
  - 状态：
    - `completed`
  - 已落地语义：
    - `DefaultWasmHookExecutor` 已按 module path 缓存 `Engine/Module`
    - 缓存会根据 wasm 文件长度与修改时间自动失效重载
  - 目标文件：
    - `crates/runtime/src/hooks/handlers/wasm.rs`
- 去掉每次 hook 单独创建 timer 线程的实现
  - 状态：
    - `completed`
  - 已落地语义：
    - 超时控制改为 tokio watchdog task + epoch interrupt
    - 同一模块执行先串行化，再启动 watchdog，避免共享 engine 下的误中断
  - 目标文件：
    - `crates/runtime/src/hooks/handlers/wasm.rs`
- 统一 command/http/wasm 的网络与审计平面
- 收紧 `DefaultCommandHookExecutor::default()` 的默认安全姿态

## 9. 文档更新要求

本轮整改后，文档必须同步更新到以下状态：

- 插件文档要明确：
  - 现状完成度
  - 未完成的 handler/runtime contribution
  - 权限边界和 effect policy 的最终形状

- Memory 文档要明确：
  - `subagent/task` export 是否已进入生产链
  - `working/coordination` 是否具备并发安全写入
  - 读路径是否仍会触发 runtime export side effect

- 多 Agent 文档要明确：
  - `dependency_ids` 是否真的参与调度
  - `agent_wait` 是否具备无丢通知语义
  - parent-child scope 是否强制执行

- 模型配置文档要明确：
  - 默认 lane 是否固定为 `gpt_5_4_default -> gpt-5.4 @ 400k`
  - `primary/subagent_defaults/roles/summary/memory` 是否都是独立解析入口
  - token usage ledger 的统计口径与持久化边界是否已经定稿

## 10. 当前迭代的完成定义

本轮整改只有在下面条件同时成立时，才能算完成：

- 多 Agent：
  - `agent_wait` 无挂死竞态
  - child 终态不可回写为非终态
  - `send/wait/cancel` 有父子作用域校验
  - batch spawn 无部分成功脏状态

- Memory：
  - `subagent/task` export 进入真实 store 导出链
  - `memory_record` 对同文件并发写不丢数据
  - `memory_list.include_stale` 语义明确且测试覆盖

- 插件：
  - WASM hook 不再自动获得 gate 权限
  - `prompt/agent` hook 不再 silent noop
  - `DriverActivationOutcome` 至少在一个宿主里形成完整闭环

- 模型配置与可观测性：
  - 默认 lane 固定为 `gpt_5_4_default -> gpt-5.4 @ 400k`
  - `summary` / `memory` 已是独立 internal profile
  - token usage 可在 runtime / store / UI 三处统一观测

## 11. 建议实施批次

### Batch 1：先止血

- 多 Agent：
  - `wait()` 竞态
  - child 终态归一化
  - parent scope 校验
  - batch spawn 原子化
- Memory：
  - 并发写保护
  - `subagent/task` export 生产链
- 插件：
  - WASM gate 权限收紧
  - `prompt/agent` fail-closed

### Batch 2：补核心对齐

- `dependency_ids` 调度
  - 状态：`completed`
- `DriverActivationOutcome` 闭环
  - 状态：`completed`
- runtime export 与多 Agent 联动
  - 状态：`completed`
- message mutation 协议/实现统一
  - 状态：`completed`
- 模型目录 + agent/internal profile 重构
  - 状态：`planned`
- token usage ledger 与 UI 展示
  - 状态：`planned`

### Batch 3：做性能与收口

- Memory 读路径增量化
- WASM runtime 缓存与 timer 优化
- write lease / store append 性能优化
