# GPT-5.4 默认模型、Agent Profile 与 Internal Workload 实施计划

日期：2026-03-28

状态：Planning

负责人：待实施

## 1. 结论先行

本计划采用以下明确口径：

- 当前是开发版本
- **不保留旧配置兼容层**
- 允许直接删除旧的单模型入口：
  - `provider.*`
  - `runtime.*` 中与单模型绑定的字段
  - `system_prompt`
- 仓库引入一组**带日期说明的默认 lane**

默认 lane 定义为：

- provider：`openai`
- model：`gpt-5.4`
- working context：`400_000`
- max output：`128_000`
- compact trigger：`320_000`
- reasoning：`medium`

说明：

- 这里的 `400k` 是**产品默认工作窗口**
- 不是 `gpt-5.4` 的模型硬上限
- 截至 **2026-03-28**，OpenAI 官方文档公开的 `gpt-5.4` 硬上限是：
  - `1,050,000` context window
  - `128,000` max output tokens
- 但本仓库默认**不**直接用 `1.05M` 作为日常工作窗口

原因：

- `1.05M` 是能力上限，不是工程默认值
- 默认跑到极限窗口会显著放大：
  - 成本
  - 延迟
  - compact 压力
  - subagent fan-out 总 token 消耗

因此，本次方案不是“拒绝默认值”，而是：

- 只保留一组**明确命名、明确日期、可审计**的默认 lane
- 禁止在代码里散落过时模型名和拍脑袋 token 常量

## 2. 外部事实依据

### 2.1 OpenAI 官方模型事实

参考：

- <https://developers.openai.com/api/docs/models/gpt-5.4>

截至 **2026-03-28**：

- OpenAI 官方模型页当前提供 `gpt-5.4`
- 当前公开：
  - `1,050,000` context window
  - `128,000` max output tokens

这给了我们两个结论：

1. `gpt-5.4` 可以作为当前仓库的默认主模型
2. 仓库需要区分：
   - 模型硬上限
   - 产品默认工作窗口

### 2.2 OpenCode 的配置启发

参考：

- <https://open-code.ai/en/docs/models>

OpenCode 文档明确说明：

- 推荐模型列表不是 exhaustive
- 也不保证永远最新

因此：

- 默认 lane 可以有
- 但必须集中定义
- 必须带日期
- 必须能被配置覆盖

### 2.3 Codex 的能力启发

参考：

- <https://developers.openai.com/codex/subagents>

Codex 的启发不只是 subagent 模型切换，还包括：

- internal workload 不一定复用主 agent 模型
- summary / background / specialized worker 本质上都应有独立配置入口

所以这次方案除了：

- primary agent
- subagent role profile

还要补：

- `internal.summary`
- `internal.memory`

## 3. 本次要解决的问题

当前仓库的问题不是“缺字段”，而是“配置抽象不对”。

### 3.1 模型没有目录，只有一个当前值

现在只能表达：

- 当前 provider
- 当前 model

不能表达：

- 哪些模型允许被 agent 使用
- alias 指向哪个模型
- 哪个模型支持：
  - tool calls
  - vision
  - image generation
  - tts

### 3.2 token 预算挂在 runtime，而不是挂在模型

当前：

- `context_tokens`
- `compact_trigger_tokens`

挂在 runtime 上，而不是挂在模型上。

这会导致：

- 切换模型时，token 预算没有同步切换
- primary / subagent / summary / memory 不能独立配置上下文窗口

### 3.3 subagent 还是“继承父 runtime”

当前 child runtime 基本继承父级：

- backend
- compaction config
- instructions
- approval policy
- tool context

结果：

- role 只是标签，不是 profile 入口
- explorer / implementer / reviewer 不能切不同模型

### 3.4 summary / memory 没有正式配置入口

当前：

- compact 使用主模型
- memory 推理路径没有和主 agent 模型清晰解耦

这会让：

- 主对话模型
- 会话总结模型
- 记忆提炼模型

绑死在一套 backend 上。

### 3.5 sandbox 不是 profile 的正式组成部分

当前还没有“根据 profile 生成 effective sandbox policy”的正式入口。

所以如果只在配置里加：

- `sandbox = "read_only"`

但 runtime / tool / process 装配不变，最后会得到假配置。

### 3.6 缺少正式的 token 用量统计机制

当前框架只有零散的 token 相关能力：

- `estimate_prompt_tokens()` 的启发式估算
- `model_context_window_tokens` 这种只传预算、不传占用的字段
- provider 响应里没有统一的 usage 解析与事件输出

这意味着系统现在做不到下面这些基础能力：

- 展示当前上下文窗口占用量
- 展示本轮：
  - input
  - output
  - prefill
  - decode
  - cache read
- 让 UI、observer、store 使用同一份 token 统计
- 用真实 usage 辅助 compact 决策和 long-context 调试

这不是锦上添花能力，而是 agent runtime 的基础可观测性。

## 4. 本次必须达到的能力

改造完成后，系统必须能表达并正确执行：

- 声明多个可用模型
- 通过 alias 引用模型
- 主 agent 默认使用 `gpt-5.4`
- 默认工作上下文使用 `400k`
- subagent 默认可以使用同一默认 lane，也可以被 role 覆盖
- summary 模型可独立配置
- memory 模型可独立配置
- token 用量可统一统计并可视化
- 每个 profile 可覆盖：
  - model alias
  - reasoning
  - prompt
  - context window
  - max output
  - compact trigger
  - sandbox
- runtime 能根据模型能力决定：
  - 是否允许工具调用
  - 是否允许视觉输入

## 5. 非目标

这次不做：

- provider registry 大一统重写
- 动态远程同步模型 catalog
- image generation / tts 完整工具链实现
- 复杂 sandbox DSL
- 旧配置兼容

## 6. 新配置结构

## 6.1 顶层结构

统一采用：

```toml
global_system_prompt = "项目级公共提示词"

[host]
workspace_only = true
sandbox_fail_if_unavailable = false
store_dir = ".nanoclaw/store"
# tokio_worker_threads = 4
# tokio_max_blocking_threads = 16

[models.gpt_5_4_default]
provider = "openai"
model = "gpt-5.4"
context_window_tokens = 400000
max_output_tokens = 128000
compact_trigger_tokens = 320000
temperature = 0.2
reasoning_effort = "medium"
capabilities = { tool_calls = true, vision = true, image_generation = true, audio_input = false, tts = true }

[models.fast]
provider = "openai"
model = "REPLACE_WITH_VERIFIED_FAST_MODEL_ID"
context_window_tokens = 200000
max_output_tokens = 32000
compact_trigger_tokens = 160000
reasoning_effort = "low"
capabilities = { tool_calls = true, vision = true, image_generation = false, audio_input = false, tts = false }

[models.vision]
provider = "openai"
model = "REPLACE_WITH_VERIFIED_VISION_MODEL_ID"
context_window_tokens = 200000
max_output_tokens = 32000
compact_trigger_tokens = 160000
reasoning_effort = "medium"
capabilities = { tool_calls = true, vision = true, image_generation = true, audio_input = false, tts = false }

[agents.primary]
model = "gpt_5_4_default"
system_prompt = "你是主编码代理"
sandbox = "workspace_write"

[agents.subagent_defaults]
model = "gpt_5_4_default"
reasoning_effort = "medium"
sandbox = "read_only"

[agents.roles.explorer]
model = "gpt_5_4_default"
system_prompt = "你是只读探索代理"
reasoning_effort = "low"
sandbox = "read_only"

[agents.roles.implementer]
model = "gpt_5_4_default"
system_prompt = "你是实现代理"
reasoning_effort = "medium"
sandbox = "workspace_write"

[agents.roles.vision]
model = "vision"
system_prompt = "你负责视觉相关任务"
sandbox = "read_only"

[internal.summary]
model = "gpt_5_4_default"
reasoning_effort = "low"
max_output_tokens = 32000

[internal.memory]
model = "gpt_5_4_default"
reasoning_effort = "medium"
max_output_tokens = 32000
```

## 6.2 字段职责

### `models.<alias>`

负责定义：

- provider
- model id
- base_url
- env
- context_window_tokens
- max_output_tokens
- compact_trigger_tokens
- temperature
- reasoning_effort
- additional_params
- capabilities

### `agents.primary`

负责定义主 agent profile。

### `agents.subagent_defaults`

负责定义所有 child agent 的通用默认值。

### `agents.roles.<role>`

负责定义按 `task.role` 路由的特化 profile。

### `internal.summary`

负责：

- 自动 compact
- 手动 compact
- 任何“把长会话压成可继续摘要”的内部调用

### `internal.memory`

负责：

- 记忆提炼
- 记忆摘要
- 记忆晋升 / 压缩
- 其他 memory reasoning 路径

说明：

- 这里指 memory 的**推理模型**
- 不是 embedding model
- embedding / rerank / query expansion 仍然保留 memory 子系统自己的现有配置面

## 6.3 覆盖规则

### 主 agent

```text
models.<agents.primary.model>
  -> agents.primary
```

### child agent

```text
models.<resolved-model>
  -> agents.subagent_defaults
  -> agents.roles.<task.role>
```

### internal workload

```text
models.<internal.summary.model>
  -> internal.summary

models.<internal.memory.model>
  -> internal.memory
```

### 关键约束

- child 不隐式回退到 primary profile
- `summary` / `memory` 不走 role 继承链
- primary / child / summary / memory 都是独立解析入口

## 6.4 Schema Freeze：顶层结构定稿

本次定稿后，`core.toml` 的顶层结构固定为：

- `global_system_prompt`
- `host`
- `models`
- `agents`
- `internal`
- `mcp_servers`
- `hook_env`
- `skill_roots`
- `plugins`

说明：

- `provider`
- `runtime`
- `system_prompt`

这三个旧时代入口全部删除，不再存在兼容映射。

其中：

- `host` 只保留宿主进程级设置
- `models` 只描述模型目录
- `agents` 只描述用户可见 agent profile
- `internal` 只描述宿主内部 workload profile

## 6.5 字段合同定稿

### `global_system_prompt`

语义：

- 所有 primary / child / internal workload 共享的最上层提示前缀

规则：

- 可为空
- 不允许是数组
- 不允许作为 per-role prompt 的替代品

### `[host]`

允许字段：

- `workspace_only: bool`
- `sandbox_fail_if_unavailable: bool`
- `store_dir: Option<String>`
- `tokio_worker_threads: Option<usize>`
- `tokio_max_blocking_threads: Option<usize>`

说明：

- `host` 只描述宿主运行时
- 不能再放：
  - `context_tokens`
  - `compact_trigger_tokens`
  - `max_tokens`
  - `model`

### `[models.<alias>]`

必填字段：

- `provider`
- `model`
- `context_window_tokens`
- `max_output_tokens`
- `compact_trigger_tokens`

可选字段：

- `base_url`
- `env`
- `temperature`
- `reasoning_effort`
- `additional_params`
- `compact_preserve_recent_messages`
- `capabilities`

字段规则：

- `context_window_tokens > 0`
- `max_output_tokens > 0`
- `compact_trigger_tokens > 0`
- `compact_trigger_tokens < context_window_tokens`
- `max_output_tokens <= context_window_tokens`
- `compact_preserve_recent_messages` 缺失时，统一默认 `8`

### `[agents.primary]`

必填字段：

- `model`
- `sandbox`

可选字段：

- `system_prompt`
- `reasoning_effort`
- `temperature`
- `max_output_tokens`
- `context_window_tokens`
- `compact_trigger_tokens`
- `compact_preserve_recent_messages`
- `additional_params`
- `auto_compact`

### `[agents.subagent_defaults]`

必填字段：

- `model`
- `sandbox`

可选字段：

- `system_prompt`
- `reasoning_effort`
- `temperature`
- `max_output_tokens`
- `context_window_tokens`
- `compact_trigger_tokens`
- `compact_preserve_recent_messages`
- `additional_params`
- `auto_compact`

### `[agents.roles.<role>]`

必填字段：

- 无

说明：

- `role` profile 可以是部分覆盖
- 但一旦出现，最终解析后必须得到完整 profile

### `[internal.summary]`

必填字段：

- `model`

可选字段：

- `system_prompt`
- `reasoning_effort`
- `temperature`
- `max_output_tokens`
- `additional_params`

说明：

- 不允许配置 `sandbox`
- 不允许配置 `context_window_tokens`
- `context_window_tokens` 永远继承所选模型

### `[internal.memory]`

必填字段：

- `model`

可选字段：

- `system_prompt`
- `reasoning_effort`
- `temperature`
- `max_output_tokens`
- `additional_params`

说明：

- 不允许配置 `sandbox`
- 不允许配置 `context_window_tokens`
- `context_window_tokens` 永远继承所选模型

## 6.6 Rust 类型定稿草案

配置层建议最终收敛为下面这些类型。

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCapabilitiesConfig {
    pub tool_calls: bool,
    pub vision: bool,
    pub image_generation: bool,
    pub audio_input: bool,
    pub tts: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub env: BTreeMap<String, String>,
    pub context_window_tokens: usize,
    pub max_output_tokens: u64,
    pub compact_trigger_tokens: usize,
    pub compact_preserve_recent_messages: Option<usize>,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub additional_params: Option<Value>,
    pub capabilities: ModelCapabilitiesConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentProfileConfig {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub context_window_tokens: Option<usize>,
    pub compact_trigger_tokens: Option<usize>,
    pub compact_preserve_recent_messages: Option<usize>,
    pub additional_params: Option<Value>,
    pub auto_compact: Option<bool>,
    pub sandbox: Option<AgentSandboxMode>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InternalProfileConfig {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub additional_params: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub primary: AgentProfileConfig,
    pub subagent_defaults: AgentProfileConfig,
    pub roles: BTreeMap<String, AgentProfileConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InternalProfilesConfig {
    pub summary: InternalProfileConfig,
    pub memory: InternalProfileConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HostRuntimeConfig {
    pub workspace_only: bool,
    pub sandbox_fail_if_unavailable: bool,
    pub store_dir: Option<String>,
    pub tokio_worker_threads: Option<usize>,
    pub tokio_max_blocking_threads: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NanoclawCoreConfig {
    pub global_system_prompt: Option<String>,
    pub host: HostRuntimeConfig,
    pub models: BTreeMap<String, ModelConfig>,
    pub agents: AgentsConfig,
    pub internal: InternalProfilesConfig,
    pub mcp_servers: Vec<McpServerConfig>,
    pub hook_env: BTreeMap<String, String>,
    pub skill_roots: Vec<String>,
    pub plugins: PluginsConfig,
}
```

## 6.7 解析与校验规则定稿

解析阶段必须做下面这些硬校验。

### alias 校验

- alias 必须非空
- alias 只能包含：
  - 小写字母
  - 数字
  - `_`
  - `-`
- `gpt_5_4_default` 被保留为仓库默认 lane 名称

### 模型校验

- `provider` 缺失时报错
- `model` 为空时报错
- `context_window_tokens == 0` 报错
- `max_output_tokens == 0` 报错
- `compact_trigger_tokens == 0` 报错
- `compact_trigger_tokens >= context_window_tokens` 报错
- `max_output_tokens > context_window_tokens` 报错

### primary 校验

- `agents.primary.model` 未定义时报错
- `agents.primary.sandbox` 未定义时报错
- `agents.primary.model` 指向未知 alias 报错

### subagent 校验

- `agents.subagent_defaults.model` 未定义时报错
- `agents.subagent_defaults.sandbox` 未定义时报错
- `agents.roles.<role>.model` 若存在，必须指向已定义 alias
- role profile 解析后若缺少 sandbox，继承 `subagent_defaults`

### internal workload 校验

- `internal.summary.model` 未定义时报错
- `internal.memory.model` 未定义时报错
- `internal.summary` / `internal.memory` 若包含 `sandbox` 字段，应直接报 schema 错误

### 默认 lane 校验

- `models.gpt_5_4_default` 缺失时报错
- `models.gpt_5_4_default.model != "gpt-5.4"` 时报错
- `models.gpt_5_4_default.context_window_tokens != 400000` 时不报错
  - 但需要在启动摘要里显式显示该默认 lane 已被项目覆盖

说明：

- 默认 lane 名称固定
- lane 内容允许项目覆盖
- 但仓库样例与默认实现应继续指向 `gpt-5.4 @ 400k`

## 6.8 override 策略定稿

### 环境变量

Phase A 不再扩张旧时代的大量 `NANOCLAW_CORE_*MODEL*` 覆盖。

环境变量只保留两类：

- 凭证
- endpoint/base_url

也就是：

- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- 以及模型显式引用的 provider env

结论：

- profile 选型不走 env override
- alias 切换不走 env override
- token budget 不走 env override

### CLI

`code-agent` 的 CLI 只允许覆盖 primary profile 的以下字段：

- `model alias`
- `system_prompt`
- `temperature`
- `reasoning_effort`

不允许覆盖：

- `summary profile`
- `memory profile`
- `subagent role profile`

原因：

- 这些属于工作区级配置，不应该被单次命令行随意破坏

## 6.9 与现有 memory 推理配置的边界

当前 memory 子系统已经有自己的推理配置面：

- `EmbeddingConfig`
- `QueryExpansionConfig`
- `RerankConfig`
- `LlmServiceConfig`

这些配置仍然保留。

边界定稿如下：

### 保留在 memory 子系统里的

- embedding model
- query expansion model
- rerank model

### 新增到 core config 的

- `internal.memory`

语义：

- 只负责宿主/runtime 层拥有的 memory reasoning workload
- 不替代 `memory_embed` 的专用 inference config

因此 Phase C / Phase G 的实现与测试必须明确：

- `internal.memory` 不会偷偷覆盖 embedding / rerank / query expansion
- `memory_embed` 的现有 inference config 不会偷偷复用 primary agent profile

## 7. 解析目标类型

为了让 runtime 只消费一套结构，`crates/config` 必须输出 resolved 类型。

建议暴露：

```rust
pub struct ResolvedModel {
    pub alias: String,
    pub provider: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub env: BTreeMap<String, String>,
    pub context_window_tokens: usize,
    pub max_output_tokens: Option<u64>,
    pub compact_trigger_tokens: usize,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub additional_params: Option<Value>,
    pub capabilities: ModelCapabilities,
}

pub struct ResolvedAgentProfile {
    pub profile_name: String,
    pub model: ResolvedModel,
    pub system_prompt: Option<String>,
    pub sandbox: AgentSandboxMode,
    pub auto_compact: bool,
}

pub struct ResolvedInternalProfile {
    pub profile_name: String,
    pub model: ResolvedModel,
    pub system_prompt: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
}
```

需要实现：

- `resolve_primary_agent(&CoreConfig)`
- `resolve_subagent_profile(&CoreConfig, role: Option<&str>)`
- `resolve_summary_profile(&CoreConfig)`
- `resolve_memory_profile(&CoreConfig)`
- `resolve_model(&CoreConfig, alias: &str)`

## 8. 运行时语义

## 8.1 Host boot

### `reference-tui`

启动时必须：

1. 解析 `agents.primary`
2. 根据 primary profile 创建 backend
3. 使用 primary profile 的：
   - context window
   - max output
   - compact trigger
   - prompt
   组装 runtime
4. 使用 `internal.summary` 创建 compactor

### `code-agent`

启动时必须：

1. 解析 `agents.primary`
2. CLI 只允许覆盖 resolved primary profile 的显式字段
3. 使用 `internal.summary` 创建 compactor
4. memory 相关内部推理路径使用 `internal.memory`

## 8.2 Subagent

当前 child runtime 是“继承父 runtime”。

本次必须改成：

```text
spawn(task)
  -> 根据 task.role 解析 child profile
  -> 创建 child backend
  -> 创建 child compact config
  -> 创建 child prompt
  -> 创建 child sandbox/tool context
  -> 创建 child runtime
```

也就是说，child runtime 的构造必须工厂化，而不是 clone 父 backend。

## 8.3 Summary

`internal.summary` 负责：

- 自动 compact
- 手动 compact
- 其他显式总结型内部调用

默认：

- model：`gpt_5_4_default`
- reasoning：`low`
- max output：`32k`

## 8.4 Memory

`internal.memory` 负责：

- 记忆提炼
- 记忆摘要
- 记忆晋升 / 压缩

默认：

- model：`gpt_5_4_default`
- reasoning：`medium`
- max output：`32k`

## 8.5 能力约束

### tool_calls

- profile 绑定的模型如果 `tool_calls = false`
- 且 runtime 需要给工具表
- 则直接 fail-fast

### vision

- 只有 `vision = true` 的模型可被声明为视觉 worker

### image_generation / tts / audio_input

第一阶段先进入配置与 resolved 类型。

是否扩展到真实 tool routing，留给后续迭代。

## 8.6 Token 用量统计语义

### 目标

框架需要一套**简单、低开销、可持久化**的 token 统计机制。

它必须同时服务：

- runtime compact 决策
- TUI/CLI 展示
- run store 追溯
- provider 调试

### 统计对象

至少统一到下面这些字段：

- `context_window_tokens`
- `context_used_tokens`
- `context_utilization_ratio`
- `input_tokens`
- `output_tokens`
- `prefill_tokens`
- `decode_tokens`
- `cache_read_tokens`
- `reasoning_tokens`

说明：

- `prefill_tokens` 表示提示侧处理量
- `decode_tokens` 表示生成侧处理量
- `cache_read_tokens` 表示 prefix / prompt cache 命中读取量
- `reasoning_tokens` 只在 provider 能提供时记录

### 上下文占用口径

- `context_used_tokens` 只表示**当前待发送 request 的 prompt 占用**
- 它由 runtime 在 request payload 最终定型后估算
- 它应包含：
  - system prompt
  - tool schema
  - 当前可见 transcript
  - compact summary
  - memory retrieval 注入内容
  - 其他实际进入请求体的文本/多模态片段
- 它不包含未来尚未生成的 output tokens
- `cache_read_tokens` 属于 provider usage 维度，不回灌到 `context_used_tokens`

### 聚合边界

这套机制的 MVP 不做 message-level token ledger。

MVP 只维护三层结果：

- 当前 request 的 `ContextWindowUsage`
- 最近一次模型响应的 `TokenUsage`
- 当前 session 的累计 totals

原因：

- 这是展示上下文占用与本轮 usage 的最小完备集合
- 可以避免按 message 落细粒度 token ledger 带来的高写放大
- 也避免 UI 自己从 transcript 反推 token

### 统计来源优先级

优先级固定为：

1. provider reported usage
2. provider reported usage details 派生值
3. runtime request-time estimate

规则：

- `input_tokens` / `output_tokens` 优先使用 provider 回包
- `prefill_tokens` 若 provider 未单独给出，则默认等于 `input_tokens`
- `decode_tokens` 若 provider 未单独给出，则默认等于 `output_tokens`
- `cache_read_tokens` 只有 provider 给出时才记录
- `context_used_tokens` 来自 runtime 在**发送请求前**的统一估算

归一化约束：

- 不要求所有 provider 一次性给齐所有 usage 子字段
- 统一层以 `Option<u64>` 承接稀疏 usage
- 除 `prefill=input` 与 `decode=output` 两条显式派生规则外，其他字段缺失时保持 `None`

### 性能约束

这套统计机制不能依赖：

- 每次 UI render 重新 tokenizer
- 每次 transcript 变更都全量重算

必须采用：

- 每次 model request 构建时只估算一次 prompt 占用
- 每次 provider response 完成时只落一次 usage
- UI 只消费 runtime/store 已经算好的 snapshot

### MVP 定义

MVP 阶段必须做到：

- request 前：
  - 估算 `context_used_tokens`
  - 计算 `context_utilization_ratio`
- response 后：
  - 记录 `input_tokens`
  - 记录 `output_tokens`
  - 记录 `prefill_tokens`
  - 记录 `decode_tokens`
  - 记录 `cache_read_tokens`
- UI 可展示：
  - `used / limit`
  - `input`
  - `output`
  - `prefill`
  - `decode`
  - `cache read`

### 数据模型建议

建议新增：

```rust
pub enum TokenUsageSource {
    ProviderReported,
    ProviderDerived,
    RuntimeEstimated,
}

pub struct TokenUsage {
    pub source: TokenUsageSource,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub prefill_tokens: Option<u64>,
    pub decode_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

pub struct ContextWindowUsage {
    pub limit_tokens: usize,
    pub used_tokens: usize,
    pub utilization_ratio: f64,
    pub compact_trigger_tokens: usize,
}

pub struct TokenUsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub prefill_tokens: u64,
    pub decode_tokens: u64,
    pub cache_read_tokens: u64,
    pub reasoning_tokens: u64,
}

pub struct RuntimeTokenLedger {
    pub current_context: Option<ContextWindowUsage>,
    pub last_usage: Option<TokenUsage>,
    pub session_totals: TokenUsageTotals,
}
```

### 事件面建议

建议补下面两类事件：

- `ModelRequestUsageEstimated`
  - request 发送前
  - 记录 `ContextWindowUsage`
- `ModelUsageReported`
  - response 完成后
  - 记录 `TokenUsage`

这样：

- observer 能实时看
- store 能持久化
- TUI 不需要自己推导
- runtime 也能直接维护一份轻量 ledger snapshot

## 9. 实施拆解

以下任务编号可直接作为执行 checklist。

## 9.1 Phase A：配置 schema 定型

目标：

- 删除旧单模型入口
- 落下新 schema
- 输出 resolved API

任务：

- `CFG-01`
  - 删除旧的单模型配置结构
- `CFG-01A`
  - 删除旧的 `ProviderConfig`
- `CFG-01B`
  - 删除旧的模型相关 `RuntimeConfig` 字段
- `CFG-01C`
  - 把宿主运行时字段重命名并收敛到 `HostRuntimeConfig`
- `CFG-02`
  - 增加 `ModelConfig`
- `CFG-03`
  - 增加 `ModelCapabilitiesConfig`
- `CFG-04`
  - 增加 `AgentProfileConfig`
- `CFG-05`
  - 增加 `InternalProfileConfig`
- `CFG-06`
  - 增加 `ResolvedModel`
- `CFG-07`
  - 增加 `ResolvedAgentProfile`
- `CFG-08`
  - 增加 `ResolvedInternalProfile`
- `CFG-09`
  - 实现 `resolve_primary_agent`
- `CFG-10`
  - 实现 `resolve_subagent_profile`
- `CFG-11`
  - 实现 `resolve_summary_profile`
- `CFG-12`
  - 实现 `resolve_memory_profile`
- `CFG-13`
  - 实现 `resolve_model`
- `CFG-14`
  - 更新示例配置
- `CFG-15`
  - 删除旧 `NANOCLAW_CORE_MODEL` / `NANOCLAW_CORE_PROVIDER` 等单模型 env override 逻辑

影响文件：

- `crates/config/src/core.rs`
- `crates/config/src/lib.rs`
- `apps/reference-tui/examples/*`

验收标准：

- 新 schema 是唯一入口
- 默认 lane 为 `gpt_5_4_default`
- primary / child / summary / memory 都可独立解析
- `host` 成为唯一宿主运行时配置入口

## 9.2 Phase B：provider backend 工厂化

目标：

- backend 构建入口只消费 resolved profile

任务：

- `PROV-01`
  - 提炼统一 backend builder
- `PROV-02`
  - request options 从 resolved profile 构建
- `PROV-03`
  - OpenAI / Anthropic reasoning 参数映射收敛到 provider 层
- `PROV-04`
  - provider summary 展示 alias + provider/model
- `PROV-05`
  - 默认 lane `gpt_5_4_default` 的 provider 参数只在一个入口定义

影响文件：

- `apps/reference-tui/src/boot/provider.rs`
- `apps/code-agent/src/provider.rs`
- `crates/provider/src/backend.rs`
- `crates/provider/src/openai/payload.rs`
- `crates/provider/src/anthropic.rs`

验收标准：

- app 不再直接拼 provider/model 请求参数
- 默认 lane 不再在多个 app 里重复硬编码

## 9.3 Phase C：主 agent 启动链

目标：

- 两个 host app 都从 resolved primary profile 启动

任务：

- `HOST-01`
  - `reference-tui` 使用 resolved primary profile
- `HOST-02`
  - `reference-tui` 使用 `internal.summary`
- `HOST-03`
  - `code-agent` 使用 resolved primary profile
- `HOST-04`
  - `code-agent` 使用 `internal.summary`
- `HOST-05`
  - memory 相关内部推理路径使用 `internal.memory`
- `HOST-06`
  - 删除 `code-agent` 旧默认模型逻辑
- `HOST-07`
  - 删除 host app 对旧 `provider/runtime/system_prompt` 结构的读取

影响文件：

- `apps/reference-tui/src/config.rs`
- `apps/reference-tui/src/boot.rs`
- `apps/reference-tui/src/boot/provider.rs`
- `apps/reference-tui/src/boot/runtime_settings.rs`
- `apps/code-agent/src/options.rs`
- `apps/code-agent/src/main.rs`
- `apps/code-agent/src/provider.rs`

验收标准：

- 默认主 lane 为 `gpt_5_4_default`
- 默认工作上下文为 `400k`
- compactor 不再复用 primary backend
- host app 不再消费旧单模型 config 字段

## 9.4 Phase D：subagent profile 工厂化

目标：

- child runtime 不再继承父 backend

任务：

- `AGT-01`
  - 为 `RuntimeSubagentExecutor` 注入 profile 解析能力
- `AGT-02`
  - 提供 child runtime factory
- `AGT-03`
  - `task.role` 路由到 `agents.roles.<role>`
- `AGT-04`
  - child prompt 使用 child profile
- `AGT-05`
  - child compact 使用 child profile / child model
- `AGT-06`
  - child sandbox 使用 child profile
- `AGT-07`
  - tool_calls=false 时 fail-fast

影响文件：

- `crates/runtime/src/subagent_impl.rs`
- `crates/tools/src/agentic/task.rs`
- `crates/runtime/src/builder.rs`
- `crates/runtime/src/compaction.rs`

验收标准：

- explorer / implementer / vision child 能用不同模型
- child 不再继承 primary backend

## 9.5 Phase E：sandbox 正式纳入 profile

目标：

- `read_only`
- `workspace_write`
- `danger_full_access`

真正成为 profile 的一部分。

任务：

- `SBX-01`
  - 定义 profile -> `SandboxPolicy` 派生函数
- `SBX-02`
  - `ToolExecutionContext` 支持显式 effective sandbox policy
- `SBX-03`
  - `bash` 使用 runtime 提供的 sandbox policy
- `SBX-04`
  - file tools 与 process tools 语义对齐

影响文件：

- `crates/tools/src/context.rs`
- `crates/tools/src/process/bash.rs`
- `crates/sandbox/src/policy.rs`
- `crates/sandbox/src/platform/linux/bwrap.rs`
- `crates/sandbox/src/platform/macos/profile.rs`

验收标准：

- 只读 profile 不能写文件
- child sandbox 与 primary sandbox 可不同

## 9.6 Phase F：能力元数据与 enforcement

目标：

- 模型能力不仅是注释，还要进入 runtime 约束

任务：

- `CAP-01`
  - `tool_calls` 接入 fail-fast
- `CAP-02`
  - `vision` 接入视觉 worker 选择
- `CAP-03`
  - `image_generation` / `tts` / `audio_input` 进入标准能力面
- `CAP-04`
  - provider descriptor 暴露能力

影响文件：

- `crates/provider/src/capabilities.rs`
- `crates/provider/src/backend.rs`
- host boot 相关代码

验收标准：

- 不能出现“模型不支持工具，但 runtime 还给工具表”的状态

## 9.7 Phase G：Token 统计与可视化

目标：

- 建立统一 token usage ledger
- 让 runtime / store / UI 使用同一份 usage 数据

任务：

- `TOK-01`
  - 在 `types` 中新增 `TokenUsageSource`
- `TOK-02`
  - 在 `types` 中新增 `TokenUsage`
- `TOK-03`
  - 在 `types` 中新增 `ContextWindowUsage`
- `TOK-04`
  - 在 `ModelEvent::ResponseComplete` 中补 usage
- `TOK-05`
  - 在 provider 层解析 OpenAI / Anthropic usage
- `TOK-06`
  - 在 runtime request build 阶段计算 `ContextWindowUsage`
- `TOK-07`
  - 在 runtime observer / run store 中落 usage 事件
- `TOK-08`
  - 在 host UI 展示 `used/limit`、`input/output`、`prefill/decode/cache_read`
- `TOK-09`
  - 在 runtime 中维护增量 `RuntimeTokenLedger`
- `TOK-10`
  - compact/debug 面板消费统一 ledger，而不是各自重算

影响文件：

- `crates/types/src/event.rs`
- `crates/runtime/src/observer.rs`
- `crates/runtime/src/runtime/turn_loop.rs`
- `crates/runtime/src/runtime/history.rs`
- `crates/runtime/src/runtime/provider_state.rs`
- `crates/provider/src/openai.rs`
- `crates/provider/src/anthropic.rs`
- `apps/reference-tui`
- `apps/code-agent`

验收标准：

- 每次请求前都能看到 `context used / limit / trigger`
- 每次响应后都能看到 `input/output/prefill/decode/cache_read`
- session 级累计 totals 可由 runtime 直接读取
- UI 不再需要自己做 token 推导

## 9.8 Phase H：测试、文档与收尾

目标：

- 形成可回归验证的闭环

任务：

- `TEST-01`
  - config 解析测试
- `TEST-02`
  - host boot 测试
- `TEST-03`
  - subagent role routing 测试
- `TEST-04`
  - sandbox 行为测试
- `TEST-05`
  - README / 示例配置 / 文档同步
- `TEST-06`
  - `internal.memory` 与 `memory_embed` inference config 边界测试
- `TEST-07`
  - 旧 env override 移除后的失败用例测试
- `TEST-08`
  - token usage 事件与 UI 展示测试

建议命令：

```bash
cargo test --manifest-path crates/Cargo.toml -p nanoclaw-config
cargo test --manifest-path crates/Cargo.toml -p runtime
cargo test --manifest-path crates/Cargo.toml -p tools
cargo test --manifest-path crates/Cargo.toml -p provider
cargo test --manifest-path apps/Cargo.toml -p reference-tui
cargo test --manifest-path apps/Cargo.toml -p code-agent
```

## 10. 追溯矩阵

- `R-01`
  - 多模型目录 + alias
  - 对应：
    - `CFG-02`
    - `CFG-13`

- `R-02`
  - primary agent 默认使用 `gpt-5.4`
  - 对应：
    - `CFG-09`
    - `HOST-01`
    - `HOST-03`

- `R-03`
  - 默认工作上下文 `400k`
  - 对应：
    - `CFG-13`
    - `HOST-01`
    - `HOST-03`

- `R-04`
  - subagent 按 role 选择 profile
  - 对应：
    - `CFG-10`
    - `AGT-01`
    - `AGT-03`

- `R-05`
  - summary 模型可独立配置
  - 对应：
    - `CFG-11`
    - `HOST-02`

- `R-06`
  - memory 模型可独立配置
  - 对应：
    - `CFG-12`
    - `HOST-05`

- `R-07`
  - sandbox 成为 profile 正式字段
  - 对应：
    - `SBX-01`
    - `SBX-02`
    - `SBX-03`

- `R-08`
  - `host` 成为唯一宿主运行时配置块
  - 对应：
    - `CFG-01C`
    - `HOST-07`

- `R-09`
  - 旧单模型 env override 被移除
  - 对应：
    - `CFG-15`
    - `TEST-07`

- `R-10`
  - `internal.memory` 与 `memory_embed` inference config 边界清晰
  - 对应：
    - `HOST-05`
    - `TEST-06`

- `R-11`
  - 框架具备统一 token 用量统计与展示能力
  - 对应：
    - `TOK-01`
    - `TOK-04`
    - `TOK-05`
    - `TOK-06`
    - `TOK-07`
    - `TOK-08`
    - `TOK-09`
    - `TOK-10`

## 11. 风险

### 11.1 默认 lane 漂移风险

- `gpt-5.4` 是按 **2026-03-28** 设定的默认 lane
- 后续若 OpenAI 更新默认推荐模型，需要单点刷新

### 11.2 subagent 风险

- 如果 child 还是 clone 父 backend，本次重构就算失败

### 11.3 sandbox 风险

- 这是最容易出现“文档支持，行为不支持”的区域
- 必须靠测试证明

### 11.4 summary / memory 风险

- 如果 compact 与 memory 仍偷偷复用 primary backend
- 那么“独立可配置”只是表面能力

## 12. Definition of Done

当且仅当下面条件全部满足，这次重构才算完成：

- 旧单模型入口已删除
- 新 schema 成为唯一入口
- 默认 lane 为 `gpt_5_4_default`
- 默认工作上下文为 `400k`
- primary / child / summary / memory 都有独立解析入口
- `host` 成为唯一宿主运行时配置块
- `reference-tui` 与 `code-agent` 都从 resolved primary profile 启动
- `summary` 与 `memory` 已改为独立 profile
- child runtime 不再继承 primary backend
- sandbox 成为 profile 的正式字段并真实生效
- token usage 可在 runtime / store / UI 三处统一观测
- tests / README / 示例配置已同步

## 13. 推荐执行顺序

严格按下面顺序做，不要跳步：

1. `Phase A`
2. `Phase B`
3. `Phase C`
4. `Phase D`
5. `Phase E`
6. `Phase F`
7. `Phase G`
8. `Phase H`

原因：

- schema 不先定，后面全部返工
- host boot 不先稳定，subagent factory 无法复用
- summary / memory 不先独立，后面 compactor 与 memory runtime 还会继续绑主模型
- token usage 应在 profile/runtime 结构稳定后接入，否则会在旧事件模型上返工一次
