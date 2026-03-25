# AGENT.md

This repository is an agent substrate, not a throwaway demo. Design clarity is part of the implementation.

## Required Engineering Behavior

- Write the smallest correct change, but do not leave non-trivial behavior unexplained.
- Add comments where control flow, invariants, protocol mapping, or safety constraints are not obvious from the code alone.
- Do not add decorative comments or narrate trivial assignments. Comments must explain intent, constraints, or reasoning.
- When a subsystem boundary or tradeoff matters, capture it in the nearest appropriate design note, crate doc, or README update.
- Prefer removing misleading abstractions over layering more policy on top of them.

## Comments

Necessary comments are required. Examples:

- append-only transcript behavior
- provider-to-runtime id mapping
- loop detection heuristics and blocking thresholds
- approval and sandbox boundaries
- feature-gated tool surfaces

If a future reader would have to reverse-engineer why a piece of code exists, add the comment.

## Design Notes

- Runtime coordination primitives such as steer and queue belong in the runtime layer, not as normal tools.
- Tool surfaces should stay minimal by default; non-essential bundles belong behind Cargo features.
- Global fixed iteration budgets are not the primary control model. Prefer explicit stop conditions and progress-aware loop detection.
