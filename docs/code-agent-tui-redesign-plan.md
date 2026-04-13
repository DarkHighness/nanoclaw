# Code Agent TUI Redesign Plan

Date: 2026-04-13

Status: Active

## Goal

Rebuild the `code-agent` TUI from a sequence of local surface tweaks into a
stable layout and rendering system that can carry future tool, review, and
history work without repeated structural rewrites.

This plan is intentionally not a "small visual polish" checklist. The current
issues cluster around shared layout constraints, duplicated surface logic, and a
weak transcript cell model. Those need a deeper pass before additional tool or
workflow surfaces are added.

## Reference Inputs

This redesign is grounded in three primary inputs:

- local reference screenshots:
  - `reference/main.png`
  - `reference/image.png`
- public Codex CLI screenshots and README presentation:
  - `https://github.com/openai/codex/blob/main/README.md`
  - `https://github.com/openai/codex/raw/main/.github/codex-cli-splash.png`
- the current in-repo TUI implementation:
  - `apps/code-agent/crates/tui/src/frontend/tui/render.rs`
  - `apps/code-agent/crates/tui/src/frontend/tui/render/chrome.rs`
  - `apps/code-agent/crates/tui/src/frontend/tui/render/transcript.rs`
  - `apps/code-agent/crates/tui/src/frontend/tui/render/transcript_shell.rs`
  - `apps/code-agent/crates/tui/src/frontend/tui/render/statusline.rs`
  - `apps/code-agent/crates/tui/src/frontend/startup_loading.rs`

## Problems Confirmed

The current TUI still has systemic issues:

- startup loading uses hard-coded colors instead of the active theme
- transcript width is computed through string padding and then wrapped again by
  `Paragraph`, causing premature line breaks
- turn dividers do not fill the full pane width
- composer and statusline still behave like separate footer bands instead of a
  lighter shared footer surface
- transcript cell boundaries are inconsistent across message, tool, shell, and
  system entries
- tool cells still leak too much low-signal structure and do not present a
  stable `intent / input / output / result` shape
- modal and overlay surfaces still share no common presentation model
- Markdown rendering exists for assistant and user text, but it is not yet the
  dominant transcript composition model

## Design Principles

1. The transcript is the primary surface.
   All layout decisions should preserve transcript width, vertical rhythm, and
   cell readability before optimizing secondary chrome.

2. Surface tokens must come from the active theme.
   Loading screens, overlays, transcript, composer, and statusline must draw
   from the same palette contract instead of embedding local color constants.

3. Layout spacing must be rect-based, not string-based.
   Width, padding, and separators should be expressed through layout geometry,
   not via pre-inserted spaces that later interact badly with wrapping.

4. Transcript entries must be cell-shaped objects.
   Messages, tool activity, shell summaries, plan/task updates, and review
   bridges should share one explicit `header / body / meta` rendering contract.

5. Markdown is the default reading path for assistant output.
   Markdown rendering should not be a side path for a few entries. It should be
   the normal composition model for assistant-facing transcript content.

6. Tool payloads must be summarized first and reviewed second.
   The timeline should show stable summaries. Deep payload inspection belongs in
   structured review surfaces, not in raw timeline dumps.

## Phase Ledger

| Phase | Status | Summary |
| --- | --- | --- |
| Phase 1: Theme And Surface Unification | Complete | `ThemePalette` now exposes semantic surface helpers and startup loading consumes the active theme instead of a local palette. |
| Phase 2: Width, Divider, And Spacing Repair | Complete | Transcript width now follows the visible pane geometry directly, and turn dividers no longer inherit artificial side padding. |
| Phase 3: Footer Layout Redesign | Complete | Composer and statusline now sit on the main transcript surface by default, and the composer reserves more breathing room. |
| Phase 4: Transcript Cell Model | Complete | Transcript rendering now composes explicit `header / body / meta` sections, and selected cells share one restrained focus chrome. |
| Phase 5: Markdown-First Transcript | Complete | Markdown now renders through assistant content and transcript detail blocks, including shell/tool text sections. |
| Phase 6: Tool, Review, And Overlay Surfaces | In Progress | Standardize tool cards, approval/review/rollback overlays, and truncation rules. |
| Phase 7: Tests, Docs, And Stabilization | Pending | Lock the new layout with rendering tests and keep this ledger updated. |

## Phase 1: Theme And Surface Unification

Goal:

- eliminate local color constants in the startup/loading path
- define one theme-driven surface vocabulary for canvas, pane, footer, overlay,
  selection, and muted chrome

Write set:

- `apps/code-agent/crates/tui/src/frontend/startup_loading.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/theme.rs`
- `apps/code-agent/crates/contracts/themes/defaults.toml`

Acceptance criteria:

- startup loading looks like the active theme, not a separate product
- there is no hard-coded palette block left in the loading screen
- footer and overlay surfaces use the same semantic palette contract as the
  transcript

Shipped:

- `ThemePalette` now exposes semantic surface helpers for canvas, transcript,
  elevated cards, overlays, and chrome borders
- `startup_loading.rs` no longer embeds a private palette and now derives all
  colors from the active theme
- theme tests now lock the semantic helpers to the underlying catalog-backed
  palette values

## Phase 2: Width, Divider, And Spacing Repair

Goal:

- stop premature wrapping
- make turn separators fill the pane
- define stable horizontal and vertical spacing rules

Write set:

- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/shared.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/shell.rs`

Required decisions:

- remove string padding as a layout primitive
- compute transcript inner width once and reuse it consistently
- keep divider width tied to the full visible pane, not the text body

Acceptance criteria:

- single-line transcript entries do not wrap before the visible boundary
- turn dividers visually fill the pane width
- transcript cells, code fences, and continuation lines share one width model

Shipped:

- transcript rendering no longer subtracts global string padding before wrapping
- turn dividers now use the full live pane width instead of a padded text width
- cell and turn spacing are expressed as layout constants instead of ad hoc
  blank-line insertion

## Phase 3: Footer Layout Redesign

Goal:

- redesign the lower area into a lighter shared footer system
- improve composer height, breathing room, and hierarchy relative to the
  transcript

Write set:

- `apps/code-agent/crates/tui/src/frontend/tui/render/chrome.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/statusline.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render.rs`

Required decisions:

- composer and statusline should not rely on persistent colored footer bands by
  default
- selection, warnings, and active work should be emphasized locally instead of
  repainting the entire footer

Acceptance criteria:

- the lower shell feels less dense and less segmented
- composer has a stronger default reading/writing area
- statusline reads as supportive metadata, not as a second main pane

Shipped:

- composer, toast, and statusline now render on the transcript surface instead
  of painting a dedicated footer band by default
- composer minimum height increased so the input area no longer collapses too
  aggressively
- footer layout tests now implicitly track the new viewport budget through the
  raised composer floor

## Phase 4: Transcript Cell Model

Goal:

- convert transcript rendering from entry-specific formatting functions into a
  stable cell protocol

Write set:

- `apps/code-agent/crates/tui/src/frontend/tui/state/transcript.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript_shell.rs`

Cell contract target:

- `header`
- `body`
- `meta`
- `selection_state`
- `collapse_policy`

Acceptance criteria:

- message/tool/system/shell cells share a consistent structural rhythm
- cell separation relies on spacing and weak separators instead of random
  formatting differences
- selected cells have a clear but restrained focus treatment

Shipped:

- transcript rendering now composes cells through an explicit
  `RenderedTranscriptCell { header, body, meta }` contract instead of flattening
  each entry shape ad hoc
- collapsed tool and shell summaries now place hidden-line hints and action
  affordances in the `meta` section instead of mixing them into the main body
- selected cells now share one elevated-surface treatment with a restrained
  focus rail, and render tests lock that chrome in place

## Phase 5: Markdown-First Transcript

Goal:

- make Markdown the canonical assistant-content rendering path
- unify code fences, headings, lists, and blockquotes under the same transcript
  width and spacing system

Write set:

- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript_markdown.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript_markdown_blocks.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/transcript_markdown_line.rs`
- related transcript cell glue in `render/transcript.rs`

Required decisions:

- preserve static prompt guidance only; do not mutate the runtime system prompt
  to teach Markdown usage
- system and review summaries may reuse the Markdown renderer where structured
  content is clearer than flat text

Acceptance criteria:

- assistant responses render as Markdown-first cells by default
- code blocks, lists, and inline code follow the same visual grammar across the
  transcript
- tool and system cells can embed Markdown sections without special-case hacks

Shipped:

- assistant and user transcript cells continue to use the Markdown renderer as
  their canonical body path on top of the new cell protocol
- shell summary `TextBlock` content now renders through the same Markdown path,
  so headings, quotes, lists, and fenced code behave like first-class transcript
  content instead of plain string dumps
- tool `TextBlock` and `LabeledBlock` content now reuse the Markdown renderer,
  which lets inline code and fenced previews survive inside structured tool
  sections without inventing a second formatting system

## Phase 6: Tool, Review, And Overlay Surfaces

Goal:

- rebuild tool timeline entries and overlays around one structured review model

Write set:

- `apps/code-agent/crates/contracts/src/tool_render.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/tool_review_overlay.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/history_rollback_overlay.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/render/chrome.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/observer.rs`

Target tool structure:

- `Intent`
- `Input`
- `Output`
- `Result`

Required decisions:

- raw JSON should be summary-backed and truncated by typed rules
- head/tail truncation should be consistent across tools
- approval, rollback, and review overlays should share one modal family

Acceptance criteria:

- tool cells are readable without opening review
- expanded review surfaces present structured detail instead of generic dumps
- modal surfaces look like one system instead of separate custom widgets

Shipped So Far:

- approval, permissions, tool review, and history rollback now share one
  overlay substrate for centered geometry, outer chrome, and inner panel
  surfaces
- overlay containers now render on the theme `overlay_surface`, while
  list/preview panes render on the shared elevated panel surface instead of
  mixing footer and bottom-pane colors by subsystem

## Phase 7: Tests, Docs, And Stabilization

Goal:

- make the redesign durable and regression-resistant

Write set:

- `apps/code-agent/crates/tui/src/frontend/tui/render/tests.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/state/tests.rs`
- `apps/code-agent/README.md`
- `docs/code-agent-tui-redesign-plan.md`

Required tests:

- width and wrapping tests
- divider width tests
- composer/statusline layout tests
- markdown transcript snapshot tests
- tool cell and overlay snapshot tests

Acceptance criteria:

- core layout invariants are test-covered
- this document is updated at the end of each phase
- the README reflects only the shipped TUI behavior

## Execution Order

The implementation should proceed in this order:

1. Phase 1
2. Phase 2
3. Phase 3
4. Phase 4
5. Phase 5
6. Phase 6
7. Phase 7

The order matters because later cell and tool changes depend on a stable theme,
width, and footer model. Do not skip ahead to tool-surface polish before the
underlying layout contracts are repaired.
