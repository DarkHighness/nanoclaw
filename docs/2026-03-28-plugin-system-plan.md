# 插件系统详细 Plan

日期：2026-03-28

状态：Planning + Reviewed

## 1. 目标

本计划要把当前仓库里的插件系统，从“能发现 TOML manifest、能激活少量内建 driver 的控制面”，推进到一个更接近工业级 Code Agent 的扩展框架。这里的“工业级”不是指功能堆叠，而是指以下几点同时成立：

- 配置是声明式、可审计、可复现的。
- 扩展既支持静态配置，也支持受限的可执行逻辑。
- 当前是开发版本，允许直接替换旧 manifest、旧 hook 协议与旧 driver 接线，不保留兼容层。
- Hook 不是单一脚本回调，而是明确分层：
  - `Matcher + Command/HTTP/Prompt/Agent` 这一条，参考 Claude Code。
  - `WASM Hook Runtime` 这一条，用于代码化执行、消息变换、策略决策。
- 插件能同时贡献 `Hook / MCP / Skill / Instructions / Tool policy / Context transform`。
- 读、写、执行、网络、消息变更等权限都必须显式授予，默认最小化。

## 2. 外部参考材料

### 2.1 工业实现

- OpenAI Codex
  - Hooks: <https://developers.openai.com/codex/hooks>
  - Subagents: <https://developers.openai.com/codex/subagents>
  - Config reference: <https://developers.openai.com/codex/config-reference>
  - Build plugins: <https://developers.openai.com/codex/plugins/build>
- Claude Code
  - Hooks reference: <https://docs.anthropic.com/en/docs/claude-code/hooks>
  - Memory: <https://docs.anthropic.com/en/docs/claude-code/memory>
  - Sub-agents: <https://docs.anthropic.com/en/docs/claude-code/sub-agents>
  - MCP: <https://docs.anthropic.com/en/docs/claude-code/mcp>
- OpenCode
  - Plugins: <https://opencode.ai/docs/plugins/>
  - Agents: <https://opencode.ai/docs/agents/>
  - Config / Permissions: <https://opencode.ai/docs/config/>
- Oh My OpenCode
  - 生态入口：<https://ohmyopencode.com/>
  - 仓库：<https://github.com/code-yeongyu/oh-my-opencode>

### 2.2 论文与研究

- OpaqueToolsBench：<https://arxiv.org/abs/2602.15197>
  - 结论：工具行为细节不透明会直接拉低 agent 的稳定性与恢复能力。
- ToolComp：<https://arxiv.org/abs/2501.01290>
  - 结论：工具使用的“过程质量”本身需要被建模和评估。
- AFlow：<https://arxiv.org/abs/2410.10762>
  - 结论：workflow 应该成为显式、可搜索、可修改的结构，而不是只藏在 prompt 里。
- SWE-agent：<https://arxiv.org/abs/2405.15793>
  - 结论：软件工程 agent 的关键不是“会调用工具”，而是“工具接口、环境约束、反馈循环”三者一起闭环。

备注：这里引用论文不是为了照搬算法，而是为了给插件系统的“接口形状”“执行边界”“可验证性”提供依据。

## 3. 当前仓库现状

基于当前代码，插件系统已经有这些基础：

- `crates/plugins`
  - 支持 plugin manifest 的发现、校验、激活计划生成。
  - 支持 `skills / hooks / mcp / driver` 四类贡献。
- `crates/core/src/plugin_boot.rs`
  - 负责把 activation plan 接到宿主启动过程。
- `crates/core/src/plugin_boot/drivers.rs`
  - 目前通过 host 侧 `match` 激活 `builtin.memory-core` 与 `builtin.memory-embed`。
- `crates/runtime/src/hooks/*`
  - 已有 command / http / prompt / agent 四类 hook handler。
- `crates/types/src/hook.rs`
  - 已有 HookEvent、HookHandler、HookOutput 等基础协议。

当前缺口也很明确：

- 插件 executable surface 仍然是“内建 Rust driver + host match”，还不是可扩展注册表。
- Hook 只能返回有限的控制输出，还没有“消息变换”这一层。
- 没有受限的插件代码执行模型，第三方代码扩展边界不清晰。
- 权限模型只到 workspace/sandbox 粒度，没有插件自己的 read/write/exec 授权面。
- `config Hook` 与 `code Hook` 还没有统一的 effect model。

## 4. 设计理念

### 4.1 插件系统必须是控制面 + 执行面分离

控制面负责：

- 发现插件
- 解析 manifest / config
- 校验权限、路径、依赖
- 生成 deterministic activation plan

执行面负责：

- 注册工具
- 执行 hook
- 启动 MCP
- 运行 WASM plugin / WASM hook

这意味着：

- `crates/plugins` 继续只做控制面。
- 新增可执行扩展运行时，不污染 `AgentRuntime` 本身。

### 4.2 Hook 必须拆成两条执行链

#### A. Matcher + Handler 链

目标：覆盖 Claude Code 风格的 deterministic hook。

特点：

- 配置简单
- 适合审计
- 适合做 guard、审批、日志、通知、简单阻断
- 默认不允许直接任意修改消息，只允许返回受限结构化 effect

支持 handler：

- `command`
- `http`
- `prompt`
- `agent`

#### B. WASM Hook Runtime 链

目标：支持真正的“代码化 Hook”，包括：

- 追加消息
- 改写消息
- 删除消息
- 插入额外上下文
- 修改工具参数
- 修改审批策略
- 注入 MCP/Skill/Instruction

这条链必须是受限执行，不允许直接拥有宿主进程的任意能力。

### 4.3 权限必须显式授予，而不是靠文档约定

每个插件最终生效的权限必须拆成：

- 读目录
- 写目录
- 执行目录 / 可执行模块
- 网络权限
- 消息变换权限
- Hook 事件订阅权限
- 可用 host API 列表

权限必须区分：

- `requested`：插件声明自己想要什么
- `granted`：宿主实际授予什么

### 4.4 Skill / MCP / Hook / Tool 不应各自为政

一个工业级插件应该能在一个 bundle 里同时表达：

- 说明自己是什么
- 带哪些 skills
- 带哪些 MCP servers
- 带哪些 hooks
- 是否包含 executable module
- 宿主给它什么权限

这也是 Codex plugin、Claude Code plugin/MCP bundle、OpenCode plugin 都共同体现的一个方向：分发单元要统一。

## 5. 总体架构

```text
plugin root
├── .nanoclaw-plugin/
│   ├── plugin.toml
│   ├── hooks.toml
│   ├── mcp.toml
│   └── permissions.example.toml
├── skills/
├── wasm/
│   └── plugin.wasm
└── assets/
```

运行时分层：

```text
crates/plugins                 控制面：发现、解析、校验、计划
        │
        ▼
crates/core::plugin_boot       宿主接线：driver registry / activation merge
        │
        ├── declarative contributions
        │   ├── skills
        │   ├── hooks
        │   └── mcp
        │
        └── executable contributions
            ├── builtin rust driver
            └── wasm runtime
                    │
                    ▼
crates/runtime/hooks           统一 effect 执行与消息变换
```

## 6. 目标数据模型

### 6.1 插件 manifest

建议新增字段：

```toml
id = "team-policy"
version = "0.2.0"
name = "Team Policy"
description = "Shared hooks, MCP, skills, and policy transforms"
kind = "bundle"
enabled_by_default = false

[components]
skill_roots = ["skills"]
hook_files = [".nanoclaw-plugin/hooks.toml"]
mcp_files = [".nanoclaw-plugin/mcp.toml"]

[runtime]
driver = "builtin.wasm-hook-validator"
module = "wasm/plugin.wasm"
abi = "nanoclaw.plugin.v1"

[capabilities]
hook_handlers = ["wasm", "command"]
message_mutations = ["append", "replace", "insert_before", "insert_after"]
tool_policies = ["deny", "rewrite_args"]
mcp_exports = true
skill_exports = true
```

说明：

- `components` 表达 declarative bundle。
- `runtime` 表达 executable part。
- `capabilities` 不是授权，而是插件声明自己会用到什么。

### 6.2 宿主配置

`core.toml` 的插件项建议扩展为：

```toml
[plugins.entries.team-policy]
enabled = true

[plugins.entries.team-policy.permissions]
read = ["docs", "schemas", ".nanoclaw/shared"]
write = [".nanoclaw/plugin-state/team-policy"]
exec = [".nanoclaw/plugins-cache/team-policy"]
network = "deny"
message_mutation = "allow"

[plugins.entries.team-policy.config]
profile = "prod"
enforce_commit_message = true
```

权限解释：

- `read` / `write`：相对 workspace root 解析。
- `exec`：WASM module、辅助 binary、hook runner 允许落盘和装载的位置。
- `network`：`deny | allow | allow_domains = []`
- `message_mutation`：`deny | allow | review_required`

### 6.3 Hook 配置

配置层 hook 继续保留，但 effect 升级：

```toml
[[hooks]]
name = "deny-rm"
event = "PreToolUse"

[hooks.matcher]
field = "tool_name"
pattern = "Bash"

[hooks.handler]
type = "command"
command = ".nanoclaw/hooks/deny-rm.sh"

[[hooks]]
name = "rewrite-user-message"
event = "UserPromptSubmit"

[hooks.handler]
type = "wasm"
module = "wasm/prompt-filter.wasm"
entrypoint = "on_user_prompt"
```

### 6.4 统一 HookEffect

当前 `HookOutput` 需要扩展成 effect list，而不是只塞 `system_message`：

```rust
pub enum HookEffect {
    AppendMessage { role: MessageRole, parts: Vec<MessagePart> },
    ReplaceMessage { selector: MessageSelector, message: Message },
    PatchMessage { selector: MessageSelector, patch: MessagePatch },
    RemoveMessage { selector: MessageSelector },
    AddContext { text: String },
    SetPermissionDecision { decision: PermissionDecision, reason: Option<String> },
    RewriteToolArgs { tool_name: ToolName, arguments: serde_json::Value },
    InjectInstruction { text: String },
    Stop { reason: String },
}
```

这里必须强调：

- `command` hook 默认只能输出“受限 effect”。
- `wasm` hook 才允许完整消息变换。
- 即使是 `wasm` hook，也必须受 capability + granted permission 双重约束。

## 7. WASM 执行模型

### 7.1 为什么选 WASM

相比动态 Rust dylib 或直接脚本执行，WASM 更适合这个仓库：

- ABI 更稳定
- 资源限制更清晰
- 权限预打开目录更自然
- 宿主可精确暴露 host function
- 更容易做组织级 allowlist / review

### 7.2 运行时建议

推荐：

- `wasmtime`
- 开启 fuel / epoch interruption
- 限制 linear memory
- 只暴露最小 host API
- 文件访问走 capability object，不直接给完整 FS

### 7.3 Host API

第一版只开放以下 host calls：

- `host.log(level, message)`
- `host.read_file(path)`
- `host.write_file(path, content)` 仅限 granted write roots
- `host.list_dir(path)`
- `host.get_hook_context()`
- `host.emit_hook_effect(effect_json)`
- `host.spawn_mcp(spec_json)` 受授权控制
- `host.resolve_skill(path)`

明确不开放：

- 任意进程执行
- 任意网络访问
- 任意环境变量读取
- 共享内存或线程

### 7.4 资源限制

建议默认值：

- 单次 hook 超时：`200ms - 2s` 视事件而定
- 最大 linear memory：`32MB`
- fuel：事件类型分层配置
- 并发实例数：按 session / plugin 限制

## 8. 对当前仓库的具体改造方案

### Phase 0：协议收敛

目标：

- 先把数据模型定住，再动执行器。

文件：

- `crates/types/src/hook.rs`
- `crates/plugins/src/manifest.rs`
- `crates/plugins/src/config.rs`
- `crates/plugins/src/resolution.rs`
- `docs/2026-03-28-plugin-system-plan.md`

交付：

- 新 manifest 字段
- 新 permission grant 模型
- 新 `HookEffect` 协议
- 删除旧 `HookOutput` 单体模型，统一切到 effect list

### Phase 1：权限模型落地

目标：

- 让插件 read/write/exec scope 成为真正的宿主策略输入。

文件：

- `crates/tools/src/context.rs`
- `crates/sandbox/src/policy.rs`
- `crates/config/src/core.rs`
- `crates/core/src/plugin_boot.rs`

交付：

- `read_only_roots` / `writable_roots` / `exec_roots`
- plan 里解析并传递 permission grants
- host summary 可展示每个插件的 granted scopes

### Phase 2：driver registry

目标：

- 去掉 `plugin_boot/drivers.rs` 里的硬编码 `match`。

文件：

- `crates/core/src/plugin_boot.rs`
- `crates/core/src/plugin_boot/drivers.rs`
- 新增 `crates/core/src/plugin_boot/registry.rs`

交付：

- `PluginDriverFactory`
- `PluginDriverRegistry`
- activation outcome 支持：
  - tools
  - hooks
  - mcp servers
  - instructions
  - diagnostics

### Phase 3：WASM hook runtime

目标：

- 把 Hook 从“只能配命令”提升为“可代码化执行，但有边界”。

建议新增 crate：

- `crates/plugin_wasm`

文件：

- `crates/runtime/src/hooks/handlers/mod.rs`
- `crates/runtime/src/hooks/runner.rs`
- `crates/types/src/hook.rs`
- `crates/core/src/plugin_boot/*`

交付：

- `HookHandler::Wasm`
- host function ABI
- `WasmHookExecutor`
- effect 验证与应用器

### Phase 4：插件分发与组件整合

目标：

- 统一 Skill / MCP / Hook / WASM 的 bundle 分发体验。

文件：

- `crates/plugins/*`
- `apps/reference-tui/src/boot/*`
- `apps/code-agent/src/main.rs`

交付：

- 插件启用后自动装载 skills / hooks / MCP / wasm module
- 宿主展示 plugin diagnostics / permissions / contributions

## 9. 验收标准

必须满足：

- 插件 manifest 与 host grants 都能被静态校验。
- 插件权限是显式的，未授权默认失败。
- `command` hook 与 `wasm` hook 都能挂在同一事件模型上。
- `wasm` hook 能安全地产生消息变换 effect。
- 插件能同时带 `skills`、`MCP`、`hooks`、`instructions`。
- 启动摘要里能看到：
  - 启用了哪些插件
  - 每个插件贡献了什么
  - 每个插件拥有哪些权限

## 10. 风险与回滚

主要风险：

- HookEffect 一旦设计过宽，会把 runtime 变成一个难以审计的“二次解释器”。
- WASM host API 如果暴露过多，权限模型会失真。
- 插件权限如果只是展示而不参与 enforcement，会变成伪安全。

回滚策略：

- 保留 declarative-only 模式。
- `HookHandler::Wasm` behind feature flag。
- 驱动注册表首批仅承载新的 builtin drivers，不引入旧接线与新注册表双轨并存。

## 11. 推荐执行顺序

建议顺序：

1. 先做 `协议 + 权限 + driver registry`
2. 再做 `HookEffect`
3. 最后做 `WASM runtime`

原因：

- 没有前两步，WASM 只会变成“另一个能跑代码的洞”。
- 先把 HookEffect 和权限面定住，后续 memory 与 multi-agent 才能安全接入消息变换与策略钩子。

## 12. 第一批可直接立项的 Issue

### Issue P1：扩展 plugin manifest / entry config

- 目标文件：
  - `crates/plugins/src/manifest.rs`
  - `crates/plugins/src/config.rs`
  - `crates/plugins/src/resolution.rs`
- 交付：
  - `runtime` / `capabilities` / `permissions` 新字段
  - 以新 manifest 为唯一格式
- 验收：
  - 解析测试覆盖新格式与越权/缺字段失败场景

### Issue P2：把 plugin grants 接到 sandbox/tool context

- 目标文件：
  - `crates/tools/src/context.rs`
  - `crates/sandbox/src/policy.rs`
  - `crates/core/src/plugin_boot.rs`
- 交付：
  - read/write/exec grants 进入宿主上下文
- 验收：
  - 非授权目录访问失败
  - 授权目录访问成功

### Issue P3：引入 driver registry

- 目标文件：
  - `crates/core/src/plugin_boot.rs`
  - `crates/core/src/plugin_boot/drivers.rs`
  - 新增 `crates/core/src/plugin_boot/registry.rs`
- 交付：
  - `PluginDriverFactory`
  - `PluginDriverRegistry`
  - builtin memory drivers 迁移到 registry
- 验收：
  - 现有 memory plugins 行为不退化

### Issue P4：升级 HookOutput 为 HookEffect

- 目标文件：
  - `crates/types/src/hook.rs`
  - `crates/runtime/src/hooks/runner.rs`
- 交付：
  - effect list
  - 删除旧字段路径
- 验收：
  - append/replace/stop 等 effect 单测

### Issue P5：实现 `HookHandler::Wasm`

- 目标文件：
  - 新增 `crates/plugin_wasm/**`
  - `crates/runtime/src/hooks/handlers/mod.rs`
  - `crates/runtime/src/hooks/runner.rs`
- 交付：
  - 最小 WASM hook host ABI
- 验收：
  - 超时、越权、正常 effect 三类测试

### Issue P6：宿主呈现与诊断

- 目标文件：
  - `apps/reference-tui/src/boot/*`
  - `apps/reference-tui/src/app/*`
- 交付：
  - 启动摘要展示 plugin contributions / permissions / diagnostics
- 验收：
  - boot summary / diagnostics 测试通过

## 13. 模块级实施清单

### 13.1 `crates/plugins`

- `manifest.rs`
  - 新增：
    - `PluginRuntimeSpec`
    - `PluginCapabilitySet`
    - `PluginPermissionRequest`
  - 删除：
    - 只够表达 bundle 的最小 manifest 结构
- `config.rs`
  - 新增：
    - `PluginPermissionGrant`
    - `PluginNetworkGrant`
    - `PluginMessageMutationGrant`
- `resolution.rs`
  - 输出：
    - `PluginExecutableActivation`
    - `PluginResolvedPermissions`
    - `PluginContributionPlan`

### 13.2 `crates/core/src/plugin_boot`

- `plugin_boot.rs`
  - activation plan 与 host boot merge 改成面向 `contributions` 工作，而不是只看 `driver_activations`
- 新增 `registry.rs`
  - 定义：
    - `PluginDriverFactory`
    - `PluginDriverRegistry`
    - `DriverContribution`
- `drivers.rs`
  - 保留 memory drivers，但迁移成 registry factory

### 13.3 `crates/types/src/hook.rs`

- 新增：
  - `HookEffect`
  - `MessageSelector`
  - `MessagePatch`
  - `HookMutationPermission`
- 删除：
  - 只靠 `system_message` / `additional_context` 表达副作用的旧模型

### 13.4 `crates/runtime/src/hooks`

- 新增 `handlers/wasm.rs`
  - `WasmHookExecutor`
- `runner.rs`
  - 执行链从“拿到 HookOutput”升级为“收集 effect -> 验证权限 -> 应用 effect”
- `handlers/mod.rs`
  - 新增 `HookHandler::Wasm`

### 13.5 `crates/runtime/src/runtime.rs`

- 增加 `apply_hook_effects(...)`
- effect 应用顺序固定：
  1. gate / permission
  2. message mutation
  3. context / instruction injection
  4. stop / continue

### 13.6 `crates/tools/src/context.rs` 与 `crates/sandbox/src/policy.rs`

- 新增：
  - `read_only_roots`
  - `writable_roots`
  - `exec_roots`
  - plugin-granted network policy

### 13.7 宿主层

- `apps/reference-tui/src/boot/*`
  - 展示 plugin grants / diagnostics / runtime modules
- `apps/code-agent/src/main.rs`
  - 接入 registry / runtime contributions / wasm runtime

## 14. 测试矩阵

### 14.1 单元测试

- `crates/plugins`
  - manifest 解析成功 / 失败
  - permission grant 缺失 / 越权
  - path traversal 拒绝
- `crates/runtime`
  - hook effect 应用顺序
  - message selector / patch 语义
  - wasm hook timeout / fuel / memory limit
- `crates/sandbox`
  - plugin granted roots 生效

### 14.2 集成测试

- 插件启用后：
  - 声明式 hook 生效
  - wasm hook 生效
  - MCP 注入生效
  - skill roots 生效
- 越权 wasm hook：
  - 读未授权目录失败
  - 写未授权目录失败
  - 调用未授权 host API 失败

### 14.3 宿主测试

- `apps/reference-tui`
  - boot summary 展示 contributions / permissions / diagnostics
- `apps/code-agent`
  - 插件启用后 runtime preamble / hook set / tool set 正确

### 14.4 建议验证命令

```bash
cargo test -p plugins
cargo test -p runtime
cargo test -p sandbox
cargo test -p agent
cargo test -p reference-tui
```

## 15. 里程碑与完成定义

### M0：协议冻结

- 完成：
  - manifest/runtime/capabilities/permissions 结构冻结
  - `HookEffect` 协议冻结
- DoD：
  - 不再讨论字段命名，只允许实现

### M1：权限与 registry

- 完成：
  - plugin grants 进入 sandbox/tool context
  - registry 替换 host `match`
- DoD：
  - memory drivers 已通过 registry 启动

### M2：effect runtime

- 完成：
  - `HookHandler::Wasm`
  - effect 验证与应用
- DoD：
  - append/replace/rewrite_args/stop 都能跑通

### M3：宿主可观测性

- 完成：
  - UI/summary 展示 contributions / permissions / diagnostics
- DoD：
  - 出错时用户能直接看出是哪个 plugin、哪种权限、哪个 runtime 失败

## 16. 审查校准与修复清单

### 16.1 当前完成度校准

- 估计完成度：约 `78%`

当前已经落地的部分：

- manifest / permissions / activation plan 控制面
- `HookEffect` 协议与 effect runtime 最小闭环
- `PluginDriverRegistry`
- `HookHandler::Wasm`
- Reference TUI 的基础插件可观测性
- WASM gate 权限不再因 handler 类型自动放开
- `prompt` / `agent` handler 默认 fail-closed
- host app 默认 HookRunner wiring 已切到 fail-closed evaluator
- `message_mutation = review_required` 会在 activation 阶段直接判为不支持

当前尚未达到计划目标的部分：

- 独立的 `builtin.wasm-hook-runtime` 已可产出 `hooks / mcp_servers / instructions`
- 当前仍未覆盖的是更广泛的 runtime contributions 种类与更强的可执行 driver 生态，不再是“只有 validator”的状态

### 16.2 P0 修复项

- 当前分支已完成：
  - 收紧 WASM hook 的 gate 权限
  - `prompt` / `agent` hook 未实现前 fail-closed
  - `ReviewRequired` 语义 fail-closed 收口

- 收紧 WASM hook 的 gate 权限：
  - 已完成：
    - `allow_gate_decision` 不再因为 handler 是 `Wasm` 自动放开
    - gate 决策现在只来自显式 `Gate` capability
    - activation plan 已补 capability-based 回归测试
- `prompt` / `agent` hook 未实现前必须 fail-closed：
  - 已完成：
    - 默认 evaluator 现在显式返回 hook error
    - HookRunner 与 host app wiring 都已切到 fail-closed evaluator
    - handler 单测与 runner 集成测试已补齐
- 明确 `ReviewRequired` 的语义：
  - 已完成：
    - `message_mutation = review_required` 在 manifest 或 resolver grant 中都会触发 activation diagnostic
    - 当前没有 host review 流程时，插件会在 activation plan 阶段被禁用

### 16.3 P1 对齐项

- 打通 `DriverActivationOutcome` 的宿主消费链：
  - 已完成：
    - `DriverActivationOutcome::extend_host_inputs()` 已成为统一 merge 点
    - `reference-tui` 与 `code-agent` 都会消费 `hooks / mcp_servers / instructions / diagnostics`
    - `code-agent` 侧已补 driver MCP 的路径解析、按名去重与宿主沙箱策略对齐
- 明确 `builtin.wasm-hook-validator` 的职责：
  - 已完成：
    - 内建 driver 已更名为 `builtin.wasm-hook-validator`
    - 其职责明确收窄为 module path / exec-root validation
    - host diagnostic 文案已改成 `validated wasm hook module ...`
    - 独立的 `builtin.wasm-hook-runtime` 已补上，可从 runtime config 产出 `hooks / mcp_servers / instructions`
- 统一消息 mutation 能力：
  - 已完成：
    - `MessageSelector` 现在支持 `Current`、`MessageId` 与 `LastOfRole`
    - `MessageId` 与 `LastOfRole` 都只允许命中当前可见 transcript
    - `LastOfRole` 只会扫描已落盘的可见 transcript，不会命中当前 in-flight 的 `Current` 消息
    - 历史 mutation 通过 append-only `patched/removed` 事件落盘，并会显式失效 provider continuation

### 16.4 P2 性能与硬化

- 缓存 WASM `Engine/Module`
  - 已完成：
    - `DefaultWasmHookExecutor` 现在按 module path 缓存 `Engine/Module`
    - 缓存会根据 wasm 文件的长度与修改时间自动失效重载，避免开发态改文件后必须重启宿主
    - 已补回归测试，覆盖缓存复用与文件变更后的自动重载
- 避免每次 hook 都起一个专用 timer 线程
  - 已完成：
    - 超时控制改成 tokio watchdog task + epoch interrupt
    - 同一模块的执行会先串行化，再启动 watchdog，避免排队请求误伤正在运行的 store
- 统一 command/http/wasm 的网络执行与审计平面
  - 已完成：
    - 三类 handler 都先经过同一个 execution preflight，不再各自散落做 grant 判断
    - preflight 统一要求显式 `HookExecutionPolicy`
    - `command` / `wasm` 共享 execute-path grant 检查
    - `http` 共享 network grant 检查
    - 三类 handler 都共享同一套 audit observer / tracing 入口，记录 `allowed / denied / completed / failed`
    - command sandbox policy 改为从 shared tool-context policy 派生，并与 host base policy 取交集，避免宿主策略被 hook grant 意外放宽
- 收紧 `DefaultCommandHookExecutor::default()` 的默认安全姿态
  - 已完成：
    - 默认 command executor 已切到 `ManagedPolicyProcessExecutor`
    - 默认 sandbox policy 改成 `workspace-write + network off + host escape deny`
    - 默认策略要求 enforcing backend 可用，否则 fail-closed
    - command hook 未显式提供 execution grants 时，会在 preflight 阶段直接拒绝

### 16.5 文档修正

本路线后续文档必须明确写清：

- 当前 `prompt` / `agent` handlers 是否仍为 fail-closed stub，还是已有真实执行器
- `DriverActivationOutcome` 已经被 `reference-tui` / `code-agent` 完整消费
- `builtin.wasm-hook-validator` 只是校验器，不是完整 runtime driver
- message mutation 当前支持 `Current + MessageId + LastOfRole(visible transcript only)`
- `LastOfRole` 只解析已落盘的可见 transcript；要修改当前正在构造的消息仍需使用 `Current`
