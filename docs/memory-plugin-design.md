# Memory Plugin Design

## Status

Drafted on 2026-03-26 as the design target for the first `memory` plugin slot.

## Scope

This note defines a memory subsystem that is:

- plugin-selected through the `memory` slot
- file-grounded, with Markdown as the source of truth
- exposed to the model through two local tools
- implemented in two interchangeable plugin variants

The two initial plugins are:

- `memory-core`
- `memory-embed`

## What Memory Is Not

This memory system is not the run store.

`crates/store` persists transcripts and replay artifacts for host/runtime auditing.
The memory subsystem is retrieval-oriented context for the model, backed by curated Markdown files inside the workspace.

That separation is deliberate:

- run history is append-only event storage
- memory is operator-editable knowledge with retrieval semantics

## Research Baseline

OpenClaw’s memory design provides the right high-level shape:

- Markdown files are the source of truth
- `MEMORY.md` and `memory/*.md` are the default corpus
- the active memory plugin provides `memory_search` and `memory_get`
- a richer backend can layer hybrid retrieval over the same files

QMD adds the important backend lessons for the richer variant:

- hybrid retrieval beats vector-only for exact ids and symbols
- BM25, vector search, and reranking should be separable stages
- the search backend should return citations and keep file retrieval explicit

For `nanoclaw`, the best split is to make those two operational modes explicit as separate plugins instead of hiding them behind one giant config surface.

## Shared Contract

### Slot model

Only one memory plugin is active at a time:

```toml
[plugins.slots]
memory = "memory-core"
```

### Canonical tool names

The repository already uses snake_case tool ids such as `web_search`.
To stay consistent, the canonical tool ids should be:

- `memory_search`
- `memory_get`

If the UI later wants kebab-case labels such as `memory-search`, that should be presentation only.

### Source of truth

Both plugins read the same corpus model:

- `MEMORY.md`
- `memory/**/*.md`
- optional extra Markdown paths declared in TOML config

The search index is derived data and may be rebuilt at any time.

### No dedicated write tool

We should not introduce `memory_write` in the first slice.

The agent already has file mutation tools:

- `write`
- `edit`
- `patch`

That keeps the memory contract small and preserves one mutation model for all workspace files.

### Shared path policy

`memory_get` only reads files accepted by the active memory plugin’s corpus policy.

That means:

- workspace-relative memory files are allowed
- configured extra Markdown roots are allowed if the plugin enables them
- arbitrary workspace files are not treated as memory

### Shared result shape

`memory_search` should return bounded, citation-ready hits:

- stable hit id
- workspace-relative path
- line range
- score
- backend id
- snippet text
- optional backend-specific metadata

`memory_get` should read from the source file, not the search index, and return:

- path
- requested line range
- resolved line range
- snapshot id
- text body

## Common Rust Surface

Add a new edge crate:

- `crates/memory`

Core traits:

```rust
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn sync(&self) -> Result<MemorySyncStatus>;
    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse>;
    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument>;
}
```

Supporting pieces:

- corpus config and path policy
- chunking policy
- index freshness tracking
- tool adapters for `memory_search` and `memory_get`

The plugin driver for the selected `memory` slot instantiates one backend and registers the two tools against it.

## How Memory Plugin Code Executes

Memory cannot be a pure configuration bundle. It needs executable logic for:

- chunking and indexing
- search and ranking
- file-grounded retrieval
- optional embedding requests

The implementation model should therefore be:

- `memory-core` and `memory-embed` are first-party compiled drivers
- their plugin manifests only select the driver id and supply TOML config
- the compiled driver builds an `Arc<dyn MemoryBackend>`
- two thin local tools call into that backend

In other words, the plugin layer is declarative, but the memory driver is code.

### Activation sketch

```rust
pub struct MemoryDriverActivation {
    pub backend: Arc<dyn MemoryBackend>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub hooks: Vec<HookRegistration>,
}

pub struct MemoryCoreDriver;

impl PluginDriverFactory for MemoryCoreDriver {
    fn id(&self) -> &'static str {
        "builtin.memory-core"
    }

    fn activate(
        &self,
        config: toml::Table,
        ctx: &PluginDriverContext,
    ) -> anyhow::Result<PluginDriverActivation> {
        let cfg = MemoryCoreConfig::try_from(config)?;
        let backend = Arc::new(SqliteLexicalMemoryBackend::new(cfg, ctx.workspace_root.clone())?);
        Ok(PluginDriverActivation {
            tools: vec![
                Arc::new(MemorySearchTool::new(backend.clone())),
                Arc::new(MemoryGetTool::new(backend)),
            ],
            hooks: Vec::new(),
            diagnostics: Vec::new(),
        })
    }
}
```

This is the key implementation point:

- the host binary owns executable behavior
- the plugin manifest chooses which compiled behavior to activate
- no dynamic library loading is required for v1

### Why this is the right first step

This is preferable to dynamic loading for the same reasons as the general plugin design:

- stable Rust trait boundaries are easy inside one binary and hard across separately built plugins
- memory backends need close integration with existing tool/path/runtime types
- we only need two first-party implementations in the initial scope

If we later want third-party executable memory backends, the safer extension path is an out-of-process protocol or MCP server, not arbitrary in-process plugin code.

## Shared Configuration Shape

The slot-specific config lives under the selected plugin entry:

```toml
[plugins.entries.memory-core.config]
include = ["MEMORY.md", "memory/**/*.md"]
index_path = ".agent-core/memory/memory-core.sqlite"

[plugins.entries.memory-core.config.chunking]
target_tokens = 400
overlap_tokens = 80

[plugins.entries.memory-core.config.search]
max_results = 6
max_snippet_chars = 700
```

Both plugins should share the same outer structure where possible so operators can switch slots without rewriting unrelated knobs.

## Plugin 1: `memory-core`

### Goal

`memory-core` is the zero-service default:

- Markdown source of truth
- local index
- deterministic retrieval
- no external embedding dependency

This captures the most important OpenClaw memory ideas without coupling the default path to remote services.

### Retrieval model

`memory-core` indexes Markdown chunks into a local SQLite sidecar:

- FTS5 for lexical retrieval
- chunk metadata for path, line range, heading ancestry, and file date

Ranking is intentionally simple:

- BM25 relevance
- curated-memory boost for `MEMORY.md`
- optional recency boost for daily files

This gives a practical baseline for:

- exact tokens
- preferences
- recent decisions

without introducing embedding-service latency or credentials.

### Why not vectors in the default plugin

OpenClaw’s upstream memory stack can do more than this, but `nanoclaw` benefits from a cleaner split:

- `memory-core` should always work in an offline local setup
- service-backed semantic retrieval should be a separate operational choice

That makes the default slot safer to ship and easier to debug.

### Session primer

`memory-core` should be designed to support a lightweight session primer in a later slice:

- read `MEMORY.md`
- read today plus N recent daily notes
- inject a short synthesized reminder near session start

This should not block the initial implementation of the search/get tools.

### Pre-compaction reminder

OpenClaw’s “memory flush before compaction” is worth preserving conceptually, but it should be a second-step feature here.

Why defer it:

- current runtime hooks in this repository do not yet have a built-in memory-aware evaluator
- the initial value is the retrievable file corpus and the two tools

So the design target is:

- v1: tools plus local index
- v2: add a memory-aware pre-compaction reminder path

## Plugin 2: `memory-embed`

### Goal

`memory-embed` is the richer retrieval plugin:

- same Markdown source of truth
- same `memory_get`
- hybrid lexical plus semantic retrieval
- configurable embedding provider
- qmd-inspired ranking pipeline

### Why a second plugin instead of more flags

This split is operationally meaningful:

- `memory-core` is local and dependency-light
- `memory-embed` depends on an embedding provider and more index state

Encoding that as two plugins keeps slot selection honest and keeps each plugin config comprehensible.

### Backend model

The current implementation maintains a local sidecar cache at
`.agent-core/memory/memory-embed.json` unless `index_path` overrides it.

That cache stores:

- chunk identity derived from path plus snapshot window
- document snapshot ids for freshness checks
- config fingerprint for embedding-invalidating changes
- chunk embedding vectors
- chunk text plus line metadata for stable hit reconstruction
- enough line metadata to map hits back to the source Markdown corpus

Embedding generation comes from a configured provider, not from an in-process model runtime.

This is intentionally the smallest durable slice. A later upgrade can replace the JSON sidecar
with a richer SQLite or FTS-backed index without changing the plugin slot contract.

### Provider config

To keep config TOML-native while still avoiding checked-in secrets, the host resolves an
environment variable name during driver activation:

```toml
[plugins.entries.memory-embed.config.embedding]
provider = "openai-compatible"
model = "text-embedding-3-small"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

This keeps the configuration surface in TOML while still using the shared env-resolution crate for
secret materialization.

### Search pipeline

The retrieval pipeline should follow the qmd lesson directly:

1. chunk the Markdown corpus into stable windows
2. compare persisted document snapshots and config fingerprint
3. lazily sync only missing or invalidated chunk embeddings into the sidecar cache
4. batch embedding requests by `embedding.batch_size`
5. embed the query
6. compute lexical scores for all candidate chunks
7. merge lexical and vector scores with configured weights
8. optionally apply rerank or diversity logic later

Suggested initial merge config:

```toml
[plugins.entries.memory-embed.config.hybrid]
vector_weight = 0.65
text_weight = 0.35
candidate_multiplier = 4
fallback = "lexical"
```

Important behaviors:

- if embedding lookup fails, fall back to lexical search
- if a chunk has only lexical or only vector score, it still participates
- `memory_get` remains fully deterministic because it reads the source file

### Future qmd-aligned features

These should be explicit follow-on work, not hidden in the first slice:

- optional reranking
- optional MMR for duplicate suppression
- optional indexing of exported session summaries from the run store
- optional background sync timers

The first implementation only needs hybrid search plus robust fallback.

## Tool Contract

### `memory_search`

Suggested request:

```json
{
  "query": "redis sentinel failover preference",
  "limit": 5,
  "path_prefix": "memory/",
  "backend_options": {}
}
```

Suggested response metadata:

- `backend`
- `index_state`
- `result_count`
- `fallback_used`

Each hit should include:

- `hit_id`
- `path`
- `start_line`
- `end_line`
- `score`
- `snippet`

### `memory_get`

Suggested request:

```json
{
  "path": "memory/2026-03-25.md",
  "start_line": 1,
  "line_count": 80
}
```

Suggested response metadata:

- `path`
- `snapshot`
- `resolved_start_line`
- `resolved_end_line`

The human-readable body should follow the same line-numbered style as `read` so the model can move from recall into file edits without switching mental models.

## Index Freshness

The first implementation should avoid background daemons.

Recommended freshness model:

- store file mtime and size for each indexed document
- run lazy sync on `memory_search`
- skip re-embedding unchanged chunks
- keep `memory_get` independent from index freshness

This is sufficient to start implementation without introducing a service manager.

## Interaction With Existing Subsystems

### Runtime

Runtime only sees more local tools. It does not need memory-specific turn logic in the first slice.

### Store

Run storage stays separate. The memory system may later consume exported summaries, but it should not depend on the run store for v1 correctness.

### Skills and hooks

Memory behavior can later be reinforced by:

- a small built-in skill that explains memory usage expectations
- optional pre-compaction reminder hooks

Neither is required for the first implementation slice.

## Planned Builtin Plugin Manifests

`memory-core`:

```toml
id = "memory-core"
version = "0.1.0"
name = "Memory Core"
description = "Markdown memory with local lexical retrieval"
kind = "memory"
enabled_by_default = true
driver = "builtin.memory-core"
```

`memory-embed`:

```toml
id = "memory-embed"
version = "0.1.0"
name = "Memory Embed"
description = "Markdown memory with hybrid lexical and embedding retrieval"
kind = "memory"
enabled_by_default = false
driver = "builtin.memory-embed"
```

## Implementation Sequence

### Slice 1

- add `crates/memory`
- add shared tool contracts
- implement `memory-core`

### Slice 2

- add plugin-driver activation for the `memory` slot
- wire `memory_search` and `memory_get` into host boot via plugin activation

### Slice 3

- implement `memory-embed`
- add embedding provider config resolution and hybrid search fallback

### Slice 4

- add optional session primer
- add optional pre-compaction reminder
- add optional background sync and richer reranking

## Sources

- OpenClaw memory concepts: <https://openclawlab.com/en/docs/concepts/memory/>
- OpenClaw plugins overview: <https://docs.openclaw.ai/plugins>
- OpenClaw plugin architecture: <https://docs.openclaw.ai/plugins/architecture>
- QMD repository and README: <https://github.com/tobi/qmd>
