# 提交时间线摘录

Date: 2026-03-27

本文件不是完整 `git log`，而是用于配合归档报告查看的关键时间线摘录。

## 2026-03-25：仓库骨架成型

- `f48b0aa` `feat: bootstrap workspace and protocol contracts`
- `b7e10d3` `feat: add local tools, skills, and run-store foundations`
- `4938fa5` `feat: add runtime orchestration`
- `8dd2a31` `feat: add provider and integration crates`
- `c2786da` `feat: add independent reference terminal shell`
- `ca2ee74` `feat: add codex-like code agent example`
- `b277238` `feat(tools): redesign read and edit contracts`
- `94d32e1` `feat(tools): unify tool contracts and add patch surface`
- `76e63e0` `feat(tools): add async bash sessioning and richer tool contracts`
- `09dcda1` `feat(tools): add optional code-intel substrate bundle`

## 2026-03-26：工程化扩展日

### Runtime / Store / Host

- `5e137e0` `feat(runtime): add composable approval policy rules`
- `6452945` `feat(runtime): add provider-managed response continuation`
- `db0f581` `feat(store): add indexed file retention controls`
- `6da4eb5` `refactor(workspace): split foundation and app workspaces`
- `14642ec` `feat(substrate): centralize env access and harden concurrency`
- `524f017` `refactor(core): split config and standardize workspace state`

### Structured Tooling

- `d18e61b` `feat(tooling): add structured tool output scaffolding`
- `422b440` `feat(tooling): structure read and web tool outputs`
- `256ffc8` `feat(tooling): structure mutation and bash outputs`
- `0c847c7` `feat(tooling): structure agentic and code-intel outputs`
- `a44b4d6` `feat(runtime): expose canonical tool lifecycle events`

### Plugins / Memory

- `615d02a` `feat(plugins): add toml plugin planning and memory drivers`
- `6799e70` `feat(plugins): wire code-agent and persist memory-embed`
- `ae26e10` `feat(agent): share plugin boot and harden memory-embed`
- `489b698` `feat(memory): align embed pipeline with qmd`
- `6733065` `feat(memory): add typed expansion and mmr reranking`
- `80f0744` `feat(memory): finalize workspace state and vector backends`
- `2f88339` `feat(memory): persist memory-core lexical index`
- `e9bab54` `fix(memory): refresh lexical cache on snapshot drift`
- `90fb5e0` `refactor(memory): share lexical sqlite sidecars`
- `af88269` `perf(memory): incrementally refresh lexical sidecars`
- `2c11df8` `perf(memory): reuse embeddings across snapshot drift`
- `a5bfdb4` `perf(memory): deduplicate embedding requests`

### Sandbox / Web / Code-Intel

- `45aeccc` `feat(tools): add managed sandbox executor`
- `f80930f` `feat(tools): add linux sandbox backend`
- `8b8761b` `feat(tools): enforce allowlist proxy sandbox networking`
- `56ca06a` `fix(tools): harden allow-domains sandboxing`
- `9ae6f51` `fix(tools): probe sandbox backend availability`
- `247a0a3` `fix(sandbox): validate backend availability at startup`
- `8dad2c6` `refactor(sandbox): extract top-level sandbox crate`
- `6460acc` `fix(sandbox): enforce protected paths in file tools`
- `700008e` `feat(web): introduce search backend boundary`
- `0ce563e` `feat(web): add exa and duckduckgo search engines`
- `8baccb6` `feat(web): add registry-driven backend selection`
- `8744409` `feat(web): add backend catalog tool`
- `1d0c730` `feat(code-agent): add managed lsp-backed code intel`
- `03c1b64` `feat(code-intel): expand managed lsp coverage`

### 文档基线校准

- `6ac0079` `docs: mark tooling status boundaries`
- `be968c8` `docs: add tooling industrial alignment note`
- `a9d15f7` `docs: add plugin and memory design notes`
- `d4592f1` `docs: archive historical design notes`

## 2026-03-27：当前最新增量

- `a38e352` `feat(memory): add temporal scoring for daily logs`

这条最新提交表明，`memory` 线已经从“把索引和 sidecar 做出来”继续推进到“检索排序质量”层面。
