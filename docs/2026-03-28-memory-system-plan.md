# Memory 系统详细 Plan

日期：2026-03-28

状态：Planning + Reviewed

## 1. 目标

当前仓库的 memory 已经不是空白状态：

- 有 `memory-core`
- 有 `memory-embed`
- 有 Markdown source of truth
- 有 `memory_search` / `memory_get`
- 有 runtime export 的雏形

但它仍然更像“代码检索记忆”，还不像“通用 Agent 的 memory system”。本计划的目标是把 memory 从单一检索后端，升级为分层、可演化、可审计的 agent memory substrate。

额外约束：

- 当前是开发版本，允许直接重塑 memory taxonomy、tool schema、sidecar 结构和目录布局，不保留旧查询参数或旧导出格式兼容层。

## 2. 外部参考材料

### 2.1 工业实现

- Claude Code memory
  - <https://docs.anthropic.com/en/docs/claude-code/memory>
  - 关键点：
    - `CLAUDE.md` 与 auto memory 分层共存。
    - auto memory 是项目级目录，按需读写 markdown。
    - subagent 可以维护自己的 auto memory。
- Codex
  - AGENTS / Rules / Skills / Hooks / Subagents 是“程序性记忆”的输入面：
    - <https://developers.openai.com/codex/config-reference>
    - <https://developers.openai.com/codex/subagents>
    - <https://developers.openai.com/codex/hooks>
- OpenCode
  - agents、permissions、plugin hooks 强调运行时状态和 agent specialization：
    - <https://opencode.ai/docs/agents/>
    - <https://opencode.ai/docs/plugins/>

### 2.2 论文与研究

- ReSum：<https://arxiv.org/abs/2509.13313>
  - 结论：长时程 agent 需要 context summarization，而不是无限堆上下文。
- ProcMEM：<https://arxiv.org/abs/2602.01869>
  - 结论：程序性记忆应该是可触发、可执行、可验证的 skill，而不仅是摘要文本。
- MemoryArena：<https://arxiv.org/abs/2602.16313>
  - 结论：真正的 agent memory 要支持跨 session、跨阶段依赖，而不是单轮 recall。
- MemOS：<https://arxiv.org/abs/2507.03724>
  - 结论：memory 应该被提升为有生命周期管理、调度与治理的一等系统资源。
- LiCoMemory：<https://arxiv.org/abs/2511.01448>
  - 结论：层次化索引、时间感知检索、结构化 metadata 对长期记忆的可用性很关键。

## 3. 当前仓库现状

关键模块：

- `crates/memory/src/backend.rs`
- `crates/memory/src/config.rs`
- `crates/memory/src/corpus.rs`
- `crates/memory/src/runtime_exports.rs`
- `crates/memory/src/memory_core.rs`
- `crates/memory/src/memory_embed.rs`
- `crates/store/src/traits.rs`
- `crates/store/src/memory.rs`

当前已经具备：

- 基于 Markdown 的 corpus
- lexical / hybrid retrieval
- runtime export sidecar
- lifecycle manifest
- background sync

当前核心缺口：

- 没有明确区分 `procedural / semantic / episodic / working / coordination` memory。
- 没有 session / subagent / work item 级别的 memory 建模。
- 没有显式的 promotion / retention / forgetting 策略。
- 现有 tools 仍以 `search/get` 为主，写入面过于间接。
- runtime export 目前更像 run summary，不足以支撑通用 agent 的跨回合与多 agent 协作。

## 4. 设计理念

### 4.1 Memory 不是单个索引，而是五层记忆

建议把 memory 明确拆成五层：

- `procedural`
  - 做事方法、执行规则、稳定 workflow、可复用策略
  - 来源：AGENTS / Skills / curated notes / promoted successful workflows
- `semantic`
  - 项目事实、术语、架构、约定、接口说明
  - 来源：`MEMORY.md`、`memory/**/*.md`、人工维护知识
- `episodic`
  - run / session / subagent 的经历、结论、故障、观察
  - 来源：runtime export sidecar
- `working`
  - 当前任务进行中的暂存状态
  - 来源：当前 session / task 的 scratchpad
- `coordination`
  - 多 agent 共享的任务分工、文件 claim、阻塞关系、产物清单
  - 来源：multi-agent coordination channel

### 4.2 Source of truth 必须仍然是 Markdown

原因：

- 可审计
- 可人工修正
- 易于 diff
- 易于与 repo 同步
- 延续当前仓库“Markdown 是 source of truth、索引是 derived state”的核心方向

索引只是 derived state。

### 4.3 记忆写入必须显式分层，不能“全都写到 MEMORY.md”

推荐目录：

```text
.nanoclaw/memory/
├── procedural/
├── semantic/
├── episodic/
│   ├── runs/
│   ├── sessions/
│   └── subagents/
├── working/
│   ├── sessions/
│   └── tasks/
└── coordination/
    ├── plans/
    ├── claims/
    └── handoffs/
```

同时继续支持工作区内现有入口：

- `MEMORY.md`
- `memory/**/*.md`

### 4.4 检索策略必须理解“层”与“时间”

检索不应只看相似度，还应考虑：

- layer / scope
- recency
- producer（主 agent / subagent / hook）
- source reliability
- promotion state
- stale / superseded 标记

## 5. 目标数据模型

### 5.1 文档元数据

建议给 memory document 增加结构化元数据：

```rust
pub struct MemoryDocumentMetadata {
    pub scope: MemoryScope,        // procedural / semantic / episodic / working / coordination
    pub layer: String,             // curated, runtime-session, task-claim, subagent-report...
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub agent_name: Option<String>,
    pub task_id: Option<String>,
    pub updated_at_ms: Option<u64>,
    pub promoted_from: Option<String>,
    pub supersedes: Vec<String>,
    pub tags: Vec<String>,
}
```

第一版可以不把这些都存在 frontmatter 里，但索引层必须能拿到。

### 5.2 查询模型

`memory_search` 建议扩展参数：

```rust
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: Option<usize>,
    pub path_prefix: Option<String>,
    pub scopes: Option<Vec<MemoryScope>>,
    pub tags: Option<Vec<String>>,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub agent_name: Option<String>,
    pub include_stale: Option<bool>,
}
```

### 5.3 新工具面

保留：

- `memory_search`
- `memory_get`

新增：

- `memory_record`
  - 把工作中间结论写入 `working` 或 `coordination`
- `memory_promote`
  - 把 working / episodic 中已验证的内容提升到 procedural / semantic
- `memory_forget`
  - 标记某条记忆为 superseded / archived，而不是物理删除
- `memory_list`
  - 让 agent 能理解当前 memory inventory

这几个工具不是为了让模型“随便写记忆”，而是为了把写入路径显式化和可审计化。

## 6. 与当前仓库的落地映射

### 6.1 procedural + semantic

继续由现有 workspace corpus 承担：

- `MEMORY.md`
- `memory/**/*.md`
- optional extra paths

新增：

- `.nanoclaw/memory/procedural/**/*.md`
- `.nanoclaw/memory/semantic/**/*.md`

目的：

- 把“项目知识”与“代理自己总结出的执行策略”分开。

### 6.2 episodic

扩展 `runtime_exports.rs`，不仅导出 run summary，还导出：

- run 级 sidecar
- session 级 sidecar
- subagent 级 sidecar
- task/job 级 sidecar

每个 sidecar 保留：

- prompt
- relevant tool use summary
- decisions
- failures
- produced artifacts
- follow-up

### 6.3 working

working memory 不应走全局共享目录，而应按 session / task 隔离：

- `.nanoclaw/memory/working/sessions/<session>.md`
- `.nanoclaw/memory/working/tasks/<task>.md`

写入来源：

- planner
- parent agent
- subagent handoff
- compaction hook

### 6.4 coordination

这是当前仓库最缺的一层。

建议写入：

- 任务拆分结果
- 文件 claim
- handoff 摘要
- block / unblock 状态
- review 结果

目录示例：

- `.nanoclaw/memory/coordination/claims/*.md`
- `.nanoclaw/memory/coordination/handoffs/*.md`
- `.nanoclaw/memory/coordination/plans/*.md`

## 7. 检索与索引策略

### 7.1 基础原则

- `procedural` 默认高优先级，但必须短小且稳定。
- `semantic` 是项目事实层。
- `episodic` 要带时间衰减，但最近任务和当前 session 可加权。
- `working` 仅在当前 session / task 强优先。
- `coordination` 在多 agent 模式下优先于通用 episodic。

### 7.2 推荐排序函数

总分建议由以下项组成：

- lexical / vector / rerank 基础分
- `scope_weight`
- `recency_multiplier`
- `session_match_bonus`
- `agent_match_bonus`
- `stale_penalty`

### 7.3 Staleness 管理

新增概念：

- `ready`
- `stale`
- `superseded`
- `archived`

不建议直接删除历史记忆，除非 operator 明确要求。

## 8. 写入与晋升策略

### 8.1 默认写入路径

- 运行时自动生成的都先写到：
  - `episodic`
  - `working`
  - `coordination`

### 8.2 晋升

只有满足这些条件的内容才能晋升到 `procedural` 或 `semantic`：

- 至少被重复验证过
- 不是一次性 incident
- 不依赖易变上下文
- 有明确 owner / source

晋升来源：

- 人工确认
- review hook
- explicit `memory_promote`

### 8.3 遗忘

遗忘不是删除，而是：

- 标记 superseded
- 降低检索权重
- 移到 archive

## 9. 对当前仓库的具体改造方案

### Phase 0：memory taxonomy 与协议

文件：

- `crates/memory/src/backend.rs`
- `crates/memory/src/config.rs`
- `crates/memory/src/corpus.rs`
- `crates/memory/src/retrieval_policy.rs`

交付：

- `MemoryScope`
- document metadata
- search filter 扩展

### Phase 1：runtime export 扩展

文件：

- `crates/memory/src/runtime_exports.rs`
- `crates/store/src/traits.rs`
- `crates/store/src/memory.rs`

交付：

- run/session/subagent/task sidecar 导出
- lifecycle manifest 记录更多统计

### Phase 2：working / coordination memory

文件：

- `crates/memory/src/config.rs`
- `crates/memory/src/state.rs`
- `crates/memory/src/tools.rs`

交付：

- `memory_record`
- `memory_list`
- working/coordination path policy

### Phase 3：promotion / forgetting

文件：

- 新增 `crates/memory/src/promotion.rs`
- 新增 `crates/memory/src/retention.rs`
- `crates/memory/src/tools.rs`

交付：

- `memory_promote`
- `memory_forget`
- stale/superseded 标记

### Phase 4：与 hooks / multi-agent 联动

文件：

- `crates/runtime/**`
- `crates/tools/src/agentic/**`
- `crates/memory/src/runtime_exports.rs`

交付：

- subagent stop 自动写 episodic / coordination memory
- compaction 前后写 working memory snapshot
- planner / reviewer / worker handoff 进入 coordination memory

## 10. 验收标准

必须满足：

- memory 能按 scope 检索。
- 当前 session 的 working memory 可被优先命中。
- subagent 与 task 的 sidecar 能被检索与追溯。
- procedural / semantic / episodic / coordination 之间边界清晰。
- memory 写入与晋升都有审计记录。
- derived index 可丢弃并重建，source of truth 不丢。

## 11. 风险与回滚

主要风险：

- working memory 如果无限增长，会重新制造 context rot。
- coordination memory 如果没有去重与 supersede 机制，会迅速污染检索。
- promotion 做得太激进，会把一次性结论误升为长期规则。

回滚策略：

- 新工具 behind feature flag。
- procedural / semantic / episodic / working / coordination 可以分阶段启用，但协议与目录布局按目标模型一次到位。
- promotion 默认为手动确认。

## 12. 推荐执行顺序

建议顺序：

1. 先做 taxonomy + metadata
2. 再做 runtime/session/subagent exports
3. 再做 working / coordination
4. 最后做 promotion / forgetting

原因：

- 没有 taxonomy，后面所有工具都会变成“把更多文本塞进同一个桶”。
- Memory 的结构必须先于 multi-agent 的结果写入，否则会把多 agent 产物直接污染现有检索层。

## 13. 第一批可直接立项的 Issue

### Issue M1：引入 `MemoryScope` 与 metadata

- 目标文件：
  - `crates/memory/src/backend.rs`
  - `crates/memory/src/corpus.rs`
  - `crates/memory/src/retrieval_policy.rs`
- 交付：
  - scope / layer / tag / owner metadata
- 验收：
  - procedural / semantic / episodic 分类测试

### Issue M2：扩展 `memory_search`

- 目标文件：
  - `crates/memory/src/backend.rs`
  - `crates/memory/src/tools.rs`
  - `crates/memory/src/memory_core.rs`
  - `crates/memory/src/memory_embed.rs`
- 交付：
  - scope/tag/run/session/agent 过滤
- 验收：
  - 检索过滤测试

### Issue M3：run/session/subagent runtime export

- 目标文件：
  - `crates/memory/src/runtime_exports.rs`
  - `crates/store/src/traits.rs`
  - `crates/store/src/memory.rs`
- 交付：
  - 多层 runtime sidecar
- 验收：
  - sidecar 生成与 prune 测试

### Issue M4：working / coordination memory

- 目标文件：
  - `crates/memory/src/config.rs`
  - `crates/memory/src/state.rs`
  - `crates/memory/src/tools.rs`
- 交付：
  - `memory_record`
  - `memory_list`
- 验收：
  - 工作记忆写入与列举测试

### Issue M5：promotion / forget lifecycle

- 目标文件：
  - 新增 `crates/memory/src/promotion.rs`
  - 新增 `crates/memory/src/retention.rs`
  - `crates/memory/src/tools.rs`
- 交付：
  - `memory_promote`
  - `memory_forget`
- 验收：
  - superseded / archived 行为测试

### Issue M6：与 multi-agent/hook 联动

- 目标文件：
  - `crates/runtime/**`
  - `crates/tools/src/agentic/**`
  - `crates/memory/src/runtime_exports.rs`
- 交付：
  - subagent result 自动进入 episodic / coordination memory
- 验收：
  - integration test 覆盖一条 parent -> child -> memory export 链路

## 14. 模块级实施清单

### 14.1 `crates/memory/src/backend.rs`

- 新增：
  - `MemoryScope`
  - `MemoryDocumentMetadata`
  - 扩展后的 `MemorySearchRequest`
- 调整：
  - `MemorySearchResponse` 与 `MemoryDocument` 输出 metadata

### 14.2 `crates/memory/src/corpus.rs`

- 新增：
  - scope/layer 推断
  - frontmatter 或 sidecar metadata 读取
- 调整：
  - `MemoryCorpusDocument` 必须携带 metadata

### 14.3 `crates/memory/src/retrieval_policy.rs`

- 新增：
  - procedural / semantic / episodic / working / coordination 权重
  - session / agent match bonus
  - stale / superseded penalty

### 14.4 `crates/memory/src/runtime_exports.rs`

- 重写导出布局：
  - `runs/`
  - `sessions/`
  - `subagents/`
  - `tasks/`
- 每类 sidecar 都输出 markdown source of truth

### 14.5 `crates/memory/src/tools.rs`

- 扩展：
  - `memory_search`
  - `memory_get`
- 新增：
  - `memory_record`
  - `memory_list`
  - `memory_promote`
  - `memory_forget`

### 14.6 新模块

- `promotion.rs`
  - 晋升规则
- `retention.rs`
  - stale / superseded / archived 生命周期

### 14.7 `crates/store`

- `traits.rs`
  - 增加导出 episodic 记录所需的最小结构
- `memory.rs`
  - 基于 run/session/subagent/task 的导出聚合

## 15. 测试矩阵

### 15.1 单元测试

- scope 分类测试
- metadata 解析测试
- retrieval 权重测试
- stale / superseded 标记测试
- promotion 规则测试

### 15.2 集成测试

- runtime export：
  - run/session/subagent/task sidecar 全量生成
  - prune stale files 正常
- 检索：
  - scope 过滤
  - session/agent 过滤
  - coordination 优先命中
- 工具：
  - `memory_record`
  - `memory_promote`
  - `memory_forget`

### 15.3 回归测试

- `memory-core`
  - lexical path 仍可用
- `memory-embed`
  - hybrid retrieval 仍可用

### 15.4 建议验证命令

```bash
cargo test -p memory
cargo test -p store
cargo test -p agent
```

## 16. 里程碑与完成定义

### M0：taxonomy 冻结

- 完成：
  - `MemoryScope` / metadata / path layout 冻结
- DoD：
  - 后续实现只补数据，不改命名与层次

### M1：runtime export 重写

- 完成：
  - run/session/subagent/task sidecar 全部落盘
- DoD：
  - derived index 删除后可重建出同一批 source markdown

### M2：新工具面

- 完成：
  - `memory_record`
  - `memory_list`
  - `memory_promote`
  - `memory_forget`
- DoD：
  - working/coordination/procedural 生命周期完整闭环

### M3：multi-agent 联动

- 完成：
  - subagent result 自动写 episodic / coordination
- DoD：
  - parent -> child -> result -> memory_search 全链路打通

## 17. 审查校准与修复清单

### 17.1 当前完成度校准

- 估计完成度：约 `85%`

当前已经落地的部分：

- 五层 taxonomy
- Markdown source of truth
- `memory_search/get/list/record/promote/forget`
- lifecycle / promotion / retention 基础模型
- runtime export 的 `run/session/subagent/task` 渲染结构
- `RunStore::export_for_memory()` 的真实 `run/session/subagent/task` 导出链
- `memory_record` 的路径级串行化追加写
- `memory_list.include_stale` 的真实状态过滤语义
- working task 对非 ASCII `task_id` 的稳定 slug 回退
- `subagent/task export` 的 in-memory / file-backed store 真测试覆盖

当前尚未达到计划目标的部分：

- tool 默认作用域只自动继承 `run_id/session_id`
- 读路径仍然过重，并夹带 runtime export side effect

### 17.2 P0 修复项

- 当前分支已完成：
  - 补齐 production 级 `subagent/task` export
  - 修复 `memory_record` 的同文件并发丢写
  - 修复 `memory_list.include_stale` 的实际语义
  - 修复非 ASCII `task_id` 导致的空 slug 路径
  - 用真实 store 测试覆盖 `subagent/task export`

- 补齐 production 级 `subagent/task` export：
  - 已完成：
    - `RunStore::export_for_memory()` 真实聚合 subagent/task 记录
    - 共享分组 helper 会消费 `TaskCreated / TaskCompleted / SubagentStart / SubagentStop / AgentEnvelope::*`
    - `InMemoryRunStore` 与 `FileRunStore` 都已经覆盖真实 runtime event 导出
- 修复 `memory_record` 的同文件并发丢写：
  - 已完成：
    - `memory_record` 现在对目标 managed path 先拿锁再做 read-modify-write
- 修复 `memory_list.include_stale` 的实际语义：
  - 已完成：
    - `include_stale = false | None` 时只返回 `ready`
    - `include_stale = true` 时显式放开 `stale/superseded/archived`
    - retrieval policy、backend、tool 三层都已经有回归测试
- 修复非 ASCII `task_id` 导致的空 slug 路径
  - 已完成：
    - working task 路径会回退到 `task-<stable-hash>`

### 17.3 P1 对齐项

- 把 runtime export 与多 Agent 主事件真正打通：
  - subagent/task sidecar 要成为 episodic retrieval 的一等输入
- 补充 runtime -> memory context bridge：
  - 需要明确 `agent/task` 作用域是否要进入 `ToolExecutionContext`

### 17.4 P2 性能与硬化

- 把 runtime export materialization 从 `get/list/search` 读路径拆出
- 给 corpus 扫描增加增量目录快照
- 避免读请求触发多余 sidecar 重写
- 评估 `memory-core` / `memory-embed` 对同一 corpus 的共享缓存策略

### 17.5 文档修正

本路线后续文档必须明确写清：

- `run/session/subagent/task` 四类 export 中，哪些已经进入真实 store 导出链
- `working/coordination` 是否已经具备并发安全写入
- `include_stale` 的精确定义
- 读路径是否仍会触发 runtime export side effect
