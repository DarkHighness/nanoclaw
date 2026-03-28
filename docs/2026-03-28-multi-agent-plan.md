# 多 Agent 协作详细 Plan

日期：2026-03-28

状态：Planning + Reviewed

## 1. 目标

当前仓库已经有：

- `task` 工具
- `RuntimeSubagentExecutor`
- 子代理工具白名单

但这离一个工业级的 multi-agent substrate 还差得很远。当前模型更像“一次性子任务委派”，而不是“有结构化并发、消息协议、状态管理、产物管理、冲突控制”的多 agent 系统。

本计划的目标是把多 agent 能力升级为：

- 可规划
- 可并发
- 可等待
- 可中断
- 可追踪
- 可回放
- 可审计
- 可与 Memory / Hook / Sandbox / MCP 联动

额外约束：

- 当前是开发版本，允许直接替换现有 `task` / `SubagentExecutor` 协议与结果模型，不保留旧输入输出 schema 的兼容层。

## 2. 外部参考材料

### 2.1 工业实现

- Codex subagents
  - <https://developers.openai.com/codex/subagents>
  - 关键点：
    - 显式请求才会 spawn
    - 可以并行 spawn 并汇总结果
    - 子代理继承父级 sandbox / approval
    - 自定义 agent 用 TOML 定义
    - 有批处理 CSV fan-out 任务
- Claude Code hooks / subagents
  - <https://docs.anthropic.com/en/docs/claude-code/hooks>
  - <https://docs.anthropic.com/en/docs/claude-code/sub-agents>
  - 关键点：
    - `SubagentStart` / `SubagentStop` 是正式 hook event
    - 子代理是可共享、可版本化的组件
- OpenCode agents
  - <https://opencode.ai/docs/agents/>
  - 关键点：
    - primary agent / subagent 区分明确
    - 子会话可导航
    - 权限、步骤数、模型都能按 agent 定制

### 2.2 论文与研究

- AFlow：<https://arxiv.org/abs/2410.10762>
  - 结论：workflow 需要成为显式图结构，而不是全靠 prompt 描述。
- Agent Context Protocols Enhance Collective Inference：<https://arxiv.org/abs/2505.14569>
  - 结论：多 agent 的关键增益来自结构化通信协议，而不是单纯增加 agent 数量。
- SWE-Gym：<https://arxiv.org/abs/2412.21139>
  - 结论：软件工程 agent 的训练与验证都依赖真实环境、真实反馈与 verifier。
- AgentMesh：<https://arxiv.org/abs/2507.19902>
  - 结论：角色分化有价值，但如果没有通信协议、状态机和错误传播模型，会很脆弱。

## 3. 当前仓库现状

关键模块：

- `crates/tools/src/agentic/task.rs`
- `crates/runtime/src/subagent.rs`
- `crates/runtime/src/runtime.rs`
- `crates/types/src/event.rs`
- `crates/store/src/*`

现状能力：

- 可委派一次子任务
- 可限制子代理可用工具
- 子代理运行在新的 runtime 中

现状缺口：

- 没有 agent 生命周期状态机。
- 没有 mailbox / event bus / artifact bus。
- 没有批量 fan-out / join 原语。
- 没有文件 claim 或写冲突控制。
- 没有 parent-child 协议对象，更多是“文本结果回传”。
- 没有和 memory / hook 的完整联动。

## 4. 设计理念

### 4.1 多 Agent 首先是“结构化并发”，不是“多开几个模型”

必须把执行过程建模成：

```text
Plan
  -> Spawn
      -> Run
      -> Report
  -> Review
  -> Join
  -> Next wave / Finish
```

并发必须显式携带：

- 依赖关系
- 最大并行度
- 失败策略
- 写入边界
- 汇总策略

### 4.2 子代理不是文本黑盒，而是协议端点

每个子代理都应该有：

- agent id
- parent id
- run id / session id
- role
- status
- requested write set
- produced artifacts
- structured result
- event stream

### 4.3 Parent 与 child 之间必须有最小通信协议

父子之间至少要能表达：

- spawn
- steer / send message
- wait
- cancel
- report result
- heartbeat / idle
- claim files
- release files

### 4.4 多 Agent 必须默认防冲突

工业级 Code Agent 的多 agent 真问题不是“怎么并发”，而是：

- 两个 agent 改了同一文件
- 一个 agent 在旧上下文里继续写
- 一个 agent 误以为另一个已经做完

所以必须引入最小协调面。

## 5. 总体架构

```text
Parent Runtime
├── Planner / Orchestrator
├── Agent Registry
├── Session Manager
├── Mailbox / Event Bus
├── Artifact Registry
└── Write-set Lease Manager
        │
        ├── Child Runtime A
        ├── Child Runtime B
        └── Child Runtime C
```

建议新增的核心能力：

- `AgentSessionManager`
- `AgentMailbox`
- `ArtifactRegistry`
- `WriteLeaseManager`

## 6. 协议设计

### 6.1 核心对象

```rust
pub struct AgentHandle {
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub role: String,
    pub status: AgentStatus,
}

pub enum AgentStatus {
    Queued,
    Running,
    WaitingApproval,
    WaitingMessage,
    Completed,
    Failed,
    Cancelled,
}

pub struct AgentEnvelope {
    pub envelope_id: EnvelopeId,
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub timestamp_ms: u64,
    pub kind: AgentEnvelopeKind,
}

pub enum AgentEnvelopeKind {
    SpawnRequested { task: AgentTaskSpec },
    Started { task: AgentTaskSpec },
    Message { channel: String, payload: serde_json::Value },
    Artifact { artifact: AgentArtifact },
    ClaimRequested { files: Vec<String> },
    ClaimGranted { files: Vec<String> },
    ClaimRejected { files: Vec<String>, owner: AgentId },
    Result { result: AgentResultEnvelope },
    Failed { error: String },
    Cancelled { reason: Option<String> },
    Heartbeat,
}
```

### 6.2 任务规格

```rust
pub struct AgentTaskSpec {
    pub task_id: String,
    pub role: String,
    pub prompt: String,
    pub steer: Option<String>,
    pub allowed_tools: Vec<ToolName>,
    pub requested_write_set: Vec<String>,
    pub dependency_ids: Vec<String>,
    pub timeout_seconds: Option<u64>,
}
```

### 6.3 结果对象

```rust
pub struct AgentResultEnvelope {
    pub status: String,
    pub summary: String,
    pub text: String,
    pub artifacts: Vec<AgentArtifact>,
    pub claimed_files: Vec<String>,
    pub structured_payload: Option<serde_json::Value>,
}
```

## 7. 工具面设计

现有 `task` 会重写为高层便捷入口。底层建议补齐以下工具：

### 7.1 `agent_spawn`

用途：

- 创建一个或多个子代理

输入：

- task spec
- role
- tool allowlist
- requested write set
- dependency ids
- timeout

输出：

- agent ids
- queued/running 状态

### 7.2 `agent_send`

用途：

- 给运行中的子代理发送 steering / context / decision

### 7.3 `agent_wait`

用途：

- 等待一个或一组 agent 到达终态

### 7.4 `agent_list`

用途：

- 查看当前 agent 树、状态、等待项、claim 情况

### 7.5 `agent_cancel`

用途：

- 终止指定 agent

### 7.6 `task_batch`

用途：

- 作为高层 fan-out/join convenience tool
- 对常见“同模板多任务”场景直接可用

### 7.7 `agent_claim_files`

用途：

- 显式申请文件写租约

### 7.8 `agent_report_result`

用途：

- 明确要求子代理提交结构化结果

这套工具里，`task_batch` 是给模型易用的入口，其余是底层控制原语。

## 8. 写冲突控制

### 8.1 为什么必须做

如果不做：

- 多 agent 会把 patch 成功率直接打崩
- parent 难以判断哪个结果可合并
- memory 里会积累互相矛盾的 handoff

### 8.2 最小可行方案

引入 `WriteLeaseManager`：

- 申请：`requested_write_set`
- 决定：
  - `granted`
  - `rejected`
  - `shared_read_only`
- 生命周期：
  - spawn 前可预申请
  - 运行中可追加申请
  - stop / cancel 自动释放

### 8.3 策略

- 默认同一路径独占写
- 目录级 claim 会阻塞子路径写
- 只读 agent 不需要 claim
- `patch` 执行前再做一次 freshness check

## 9. Hooks、Memory、MCP 的联动

### 9.1 Hooks

必须新增并真正使用：

- `SubagentStart`
- `SubagentStop`
- `TaskCreated`
- `TaskCompleted`

作用：

- 审计
- 阻断不合法 fan-out
- 为 parent 注入注意事项
- 自动触发 review / verification hook

### 9.2 Memory

多 agent 结果不应只回 parent 文本窗口，还应写入：

- episodic memory
- coordination memory

内容包括：

- 子代理摘要
- artifact 列表
- claimed files
- blocker / next step

### 9.3 MCP

子代理可以带自己的 MCP，但必须遵守：

- parent policy 继承
- agent-specific override
- 插件 grants
- allowlist/denylist

## 10. 对当前仓库的具体改造方案

### Phase 0：协议与事件层

文件：

- `crates/types/src/event.rs`
- `crates/tools/src/agentic/task.rs`
- `crates/runtime/src/subagent.rs`
- `crates/store/src/traits.rs`

交付：

- subagent lifecycle events
- parent/child scope
- structured result envelope

### Phase 1：会话与状态管理

建议新增：

- `crates/runtime/src/agent_session_manager.rs`
- `crates/runtime/src/agent_mailbox.rs`

交付：

- agent registry
- mailbox
- status transitions

### Phase 2：工具面

文件：

- `crates/tools/src/agentic/task.rs`
- 新增 `crates/tools/src/agentic/agent_control.rs`

交付：

- `agent_spawn`
- `agent_send`
- `agent_wait`
- `agent_list`
- `agent_cancel`
- `task_batch`

### Phase 3：写入租约

建议新增：

- `crates/runtime/src/write_lease.rs`

交付：

- requested write set
- lease grant/reject
- tool execution 前 freshness + lease check

### Phase 4：Memory / Hook 联动

文件：

- `crates/memory/src/runtime_exports.rs`
- `crates/runtime/src/subagent.rs`
- `crates/runtime/src/hooks/*`

交付：

- subagent summaries 入 memory
- hook 能观察与控制 multi-agent lifecycle

### Phase 5：宿主呈现

文件：

- `apps/reference-tui/src/app/*`
- `apps/code-agent/src/tui/*`

交付：

- agent tree
- waiting / approval / claims 视图
- child session navigation

## 11. 验收标准

必须满足：

- 可以一次 fan-out 多个子代理并 join。
- parent 可以向运行中子代理追加 steering。
- 每个子代理都有结构化状态与结果。
- 文件写冲突可以被显式阻止。
- hooks 能观察 `SubagentStart/Stop`。
- memory 能记录子代理产物与 handoff。

## 12. 风险与回滚

主要风险：

- 没有 lease manager 的并发写会制造大规模脏 patch。
- protocol 如果只传文本，不传结构化状态，最终会退回 prompt 工程。
- agent tree 深度失控会带来 token、延迟、资源爆炸。

回滚策略：

- 新控制工具 behind feature flag。
- `agents.max_threads` 与 `agents.max_depth` 默认保守。
- 若新协议尚未稳定，可先只开放 `task_batch + agent_wait`，但不保留旧 `task` schema。

## 13. 推荐执行顺序

建议顺序：

1. 事件与协议
2. `task_batch` + `agent_wait`
3. mailbox / session manager
4. write lease
5. hooks / memory 联动
6. host UI

原因：

- 没有事件与协议，后面任何“多 agent”都只是并发文本调用。
- 没有 `wait/join` 和状态管理，structured concurrency 落不了地。
- 没有 lease manager，代码修改类并发基本不可用。

## 14. 与另外两条路线的依赖

它依赖：

- 插件系统路线提供：
  - agent-specific hooks
  - wasm hook runtime
  - agent-specific MCP / skills
- memory 路线提供：
  - episodic / coordination memory
  - session / subagent sidecar

它反过来会为另外两条提供：

- hooks 的生命周期事件
- memory 的结构化写入来源
- 更可审计的 agent 行为轨迹

## 15. 第一批可直接立项的 Issue

### Issue A1：补 parent/child 协议对象

- 目标文件：
  - `crates/tools/src/agentic/task.rs`
  - `crates/types/src/event.rs`
  - `crates/runtime/src/subagent.rs`
- 交付：
  - parent scope
  - task id
  - structured result envelope
- 验收：
  - `task` 与底层协议统一为新 schema

### Issue A2：实现 `task_batch`

- 目标文件：
  - `crates/tools/src/agentic/task.rs`
- 交付：
  - fan-out / join convenience tool
- 验收：
  - 并发批处理测试
  - stop-on-error 测试

### Issue A3：subagent lifecycle events

- 目标文件：
  - `crates/types/src/event.rs`
  - `crates/store/src/traits.rs`
  - `crates/runtime/src/subagent.rs`
- 交付：
  - `SubagentStart` / `SubagentStop` 事件
- 验收：
  - run store replay / search 可见这些事件

### Issue A4：AgentSessionManager + mailbox

- 目标文件：
  - 新增 `crates/runtime/src/agent_session_manager.rs`
  - 新增 `crates/runtime/src/agent_mailbox.rs`
- 交付：
  - agent registry
  - message routing
- 验收：
  - spawn/send/wait 单测

### Issue A5：write lease manager

- 目标文件：
  - 新增 `crates/runtime/src/write_lease.rs`
  - `crates/runtime/src/subagent.rs`
  - `crates/tools/src/fs/patch.rs`
- 交付：
  - 文件 claim / release
- 验收：
  - 冲突 claim 拒绝测试

### Issue A6：host 侧 agent tree / status 呈现

- 目标文件：
  - `apps/reference-tui/src/app/*`
  - `apps/code-agent/src/tui/*`
- 交付：
  - agent tree / waiting / claims / artifacts 视图
- 验收：
  - UI presenter tests 覆盖状态渲染

## 16. 模块级实施清单

### 16.1 `crates/types/src/event.rs`

- 新增：
  - `SubagentStart`
  - `SubagentStop`
  - `AgentEnvelope`
  - `AgentResultEnvelope`
- 要求：
  - parent/child 关系、task id、artifact list、claim list 都能落事件

### 16.2 `crates/tools/src/agentic/task.rs`

- 重写：
  - `task` 输入/输出 schema
- 新增：
  - `task_batch`
  - 与底层协议统一的 structured result 解析

### 16.3 新增 `crates/tools/src/agentic/agent_control.rs`

- 定义：
  - `agent_spawn`
  - `agent_send`
  - `agent_wait`
  - `agent_list`
  - `agent_cancel`

### 16.4 `crates/runtime/src/subagent.rs`

- 扩展：
  - parent scope
  - lifecycle events
  - message bus 接入点
- 不再只返回纯文本结果

### 16.5 新运行时模块

- `agent_session_manager.rs`
  - agent registry / status transitions
- `agent_mailbox.rs`
  - send / receive / heartbeat
- `write_lease.rs`
  - 文件 claim / release / conflict detection

### 16.6 `crates/store`

- run store 需要能索引：
  - subagent lifecycle
  - structured result
  - claim/release 事件

### 16.7 宿主层

- `apps/reference-tui`
  - agent tree
  - pending approvals
  - claims / artifacts
- `apps/code-agent`
  - 子会话导航
  - fan-out / join 状态

## 17. 测试矩阵

### 17.1 单元测试

- agent status transition
- envelope 序列化/反序列化
- write lease grant/reject
- mailbox send/wait/cancel

### 17.2 集成测试

- `task_batch`
  - 多任务并发完成
  - stop-on-error
  - timeout
- `agent_send/agent_wait`
  - parent 向运行中 child 追加 steering
- `write_lease`
  - 文件冲突拦截

### 17.3 联动测试

- hooks 观察 `SubagentStart/Stop`
- memory 能收集 subagent handoff
- parent 最终能汇总 child artifacts

### 17.4 建议验证命令

```bash
cargo test -p runtime
cargo test -p tools
cargo test -p store
cargo test -p agent
cargo test -p code-agent
```

## 18. 里程碑与完成定义

### M0：协议冻结

- 完成：
  - `AgentEnvelope`
  - `AgentResultEnvelope`
  - 生命周期事件
- DoD：
  - 事件字段与 tool schema 冻结

### M1：结构化并发最小闭环

- 完成：
  - `task_batch`
  - `agent_wait`
  - subagent lifecycle events
- DoD：
  - fan-out / join 已可用

### M2：控制面与冲突控制

- 完成：
  - `agent_send`
  - `agent_cancel`
  - `write_lease`
- DoD：
  - 多 agent 写同一文件时会被 deterministic 阻止

### M3：可观测性与联动

- 完成：
  - host agent tree
  - hooks / memory 联动
- DoD：
  - agent tree、artifact、claim、handoff 都可回放与检索

## 19. 审查校准与修复清单

### 19.1 当前完成度校准

- 估计完成度：约 `70% ~ 75%`

当前已经落地的部分：

- `task / task_batch / agent_spawn / agent_send / agent_wait / agent_list / agent_cancel`
- `AgentHandle / AgentEnvelope / AgentResultEnvelope`
- `AgentSessionManager`
- `AgentMailbox`
- `WriteLeaseManager`
- 生命周期事件与基础 store 持久化
- `agent_wait` 的无丢通知语义
- parent-child 控制面隔离
- batch spawn 的原子化 preflight / claim / start 流程

当前尚未达到计划目标的部分：

- `dependency_ids` 已进入 batch 内依赖调度，但还不是完整的通用 DAG/工作流执行器
- 终态收尾 owner 还没有统一
- root runtime 与 parent runtime 的控制面授权边界还没有写成完整策略

### 19.2 P0 修复项

- 当前分支已完成：
  - 修复 child 非终态结果归一化
  - 修复 `agent_wait()` 的无丢通知语义
  - 给 `send / wait / cancel / list` 加了 parent-child scope 收紧
  - 修复批量 spawn 的部分成功副作用，改为全量 preflight + claim + 再启动

- 修复 child 终态收敛：
  - 自然完成的 child 不允许回写非终态 status
- 修复 `agent_wait()` 的无丢通知语义：
  - `snapshot -> notified().await` 结构必须替换
- 给 `send / wait / cancel` 增加父子作用域校验
- 修复批量 spawn 的部分成功副作用：
  - 已完成：
    - 全量预解析 task / tool / write set
    - 全批次先 claim write lease
    - claim 失败时释放已申请 lease，不留下 child session / worker
    - 只有事件追加成功后才真正启动 child runtime

### 19.3 P1 对齐项

- 让 `dependency_ids` 真正参与调度：
  - 已完成：
    - `task_batch` / `agent_spawn` 现在会消费 `dependency_ids`
    - runtime 已引入 ready set / blocked set / completion propagation
    - 缺失依赖 / 自依赖 / 循环依赖会在调度前失败
    - upstream 失败会阻断 downstream child，并产出明确终态结果
- 统一终态收尾 owner：
  - 状态、事件、lease release 不能继续由 manager 和 worker 分别收尾
- 明确 root runtime 与 parent runtime 的控制面边界：
  - `list/send/wait/cancel` 的授权模型要写清楚

### 19.4 P2 性能与硬化

- 批量 spawn 改为有界并行冷启动
- 优化 `WriteLeaseManager` 的冲突检测结构
- 降低生命周期事件 append 写放大
- 评估 mailbox / session manager 是否需要从单点 `Mutex<BTreeMap<...>>` 升级

### 19.5 文档修正

本路线后续文档必须明确写清：

- `dependency_ids` 已经是 runtime-enforced，但目前只覆盖 batch 内最小依赖调度
- `agent_wait` 是否已经具备无丢通知语义
- `send / wait / cancel` 是否已强制 parent-child scope
- `task_batch` 当前是最小依赖调度器，还不是完整 workflow engine
