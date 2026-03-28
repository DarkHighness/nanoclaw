# AGENTS.md

This repository is an agent substrate, not a throwaway demo. Design clarity is part of the implementation.

## Required Engineering Behavior

- Write the smallest correct change, but do not leave non-trivial behavior unexplained.
- Add comments where control flow, invariants, protocol mapping, or safety constraints are not obvious from the code alone.
- Do not add decorative comments or narrate trivial assignments. Comments must explain intent, constraints, or reasoning.
- When a subsystem boundary or tradeoff matters, capture it in the nearest appropriate design note, crate doc, or README update.
- Prefer removing misleading abstractions over layering more policy on top of them.
- Before any commit, run repository formatting. The enforced hook path does this automatically and blocks the commit until formatted files are re-staged.
- Commit messages must follow Conventional Commits. The repository `commit-msg` hook enforces this on the first line.
- When a meaningful implementation slice is complete and the relevant validation passes, create a git commit proactively instead of leaving the repository in an uncommitted completed state.

## Comments

Necessary comments are required. Examples:

- append-only transcript behavior
- provider-to-runtime id mapping
- loop detection heuristics and blocking thresholds
- approval and sandbox boundaries
- feature-gated tool surfaces

All source-code comments and doc comments must be written in English. Do not
introduce Chinese or mixed-language code comments, even if surrounding design
notes or user-facing documentation are written in another language.

If a future reader would have to reverse-engineer why a piece of code exists, add the comment.

## Design Notes

- Runtime coordination primitives such as steer and queue belong in the runtime layer, not as normal tools.
- Tool surfaces should stay minimal by default; non-essential bundles belong behind Cargo features.
- Global fixed iteration budgets are not the primary control model. Prefer explicit stop conditions and progress-aware loop detection.
- Prefer ownership and message passing over `Arc<Mutex<_>>`. If a single consumer owns a channel receiver, keep the receiver on that owner instead of wrapping it in a mutex.
- Use async locks only when guarded state must survive across `.await`. Plain in-memory state with short critical sections should use synchronous locks or owned task state.
- Independent async startup and discovery work should run with bounded concurrency. Preserve output ordering deliberately instead of serializing by accident.
- Environment-variable definitions and lookups belong in `crates/env`. Do not scatter raw env-key access across substrate crates.
- Substrate crates should emit structured `tracing` events for turn lifecycle, provider requests, tool execution, retries, background sessions, and degradation paths.

## Git Hooks

- Install hooks with `scripts/install-git-hooks.sh` after cloning or whenever local git config is reset.
- The repository hook path is `.githooks`.
- `pre-commit` formats the `crates/` substrate workspace and the `apps/` host-app workspace, then stops if formatting changed staged files.
- `commit-msg` accepts only Conventional Commit subjects such as `feat(runtime): ...` or `docs: ...`.
