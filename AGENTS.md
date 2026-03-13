# Agent Rules

Rules for AI agents (Claude Code, Copilot, Cursor, etc.) working in this repository. These exist because past bugs were caused by violating these patterns. Read `CLAUDE.md` for project context and `STYLE_GUIDE.md` for design guidelines.

## Mandatory

### Animation state machines must not leak "active" or "open" state

`PanelAnimation::close()` must set `open = false` immediately. `OverlayAnimation::close()` keeps `active = true` but requires `update()` to be called every frame to eventually clear it.

**Why this matters:** The mouse click dispatch chain in `app/mod.rs` checks `is_open()` / `is_active()` for each UI layer and swallows ALL clicks with an early `return` if true. A stuck-open panel blocks every click handler below it in the priority chain. This caused a real bug where closing the commands panel left `is_open() == true` permanently because (a) `close()` didn't clear `open`, and (b) no `update()` call existed for the commands panel.

**Rules:**
- If you add a new panel using `PanelAnimation`, its `close()` behavior comes from `PanelAnimation::close()` which correctly sets `open = false`. Do not override this.
- If you add a new overlay using `OverlayAnimation`, you **must** add an `update_animation()` call in the render path (`app/mod.rs` render section, near the other `update()` calls around lines 930-1020).
- If you add a new entry to the click dispatch chain in `app/mod.rs`, place it at the correct priority level and ensure the guard condition (`is_open()`, `is_active()`) correctly reflects the component's state.
- Never add an `is_open()` / `is_active()` guard that returns early without ensuring the component properly clears that state on close.

### Hit-test functions must be `pub`, not `pub(super)`

Functions like `tab_bar_hit_test()`, `search_bar_hit_test()`, `commands_panel_hit_test()`, `session_panel_hit_test()`, `session_dropdown_hit_test()` are called from `app/mod.rs`, outside the renderer module. They must be `pub fn`, not `pub(super) fn`.

Build functions (`build_tab_bar()`, `build_command_palette()`, etc.) are called only from `renderer/mod.rs` and should be `pub(super) fn`.

### Renderer types must be re-exported

All public types defined in `renderer/types.rs` are re-exported from `renderer/mod.rs` via `pub use types::*`. If you add a new type to `types.rs`, it is automatically re-exported. If you add a type to a sub-module instead, you must add an explicit `pub use` in `mod.rs`.

### Hit-test geometry must match render geometry

Every `*_hit_test` function must compute button/element positions using the **exact same math** as the corresponding `build_*` function. If you change padding, spacing, or layout in a `build_*` function, you must update the matching hit-test function. These are often in the same file but not always — check both.

## Style & Conventions

### Read STYLE_GUIDE.md before any UI work

Theme names must be real roots/rhizomes/tubers. Colors must derive from the actual plant. Animations use ease-out easing, 150-250ms. UI should feel warm and organic, not clinical.

### Animation timing

Use `Instant`-based timing, never delta-time accumulation. See `CLAUDE.md` for the correct pattern.

### Renderer module organization

- Each `build_*` function lives in its own sub-module file under `renderer/`
- The parent `renderer/mod.rs` contains only: struct definition, `new()`, `render()`, `build_render_data()` dispatch, `UiPalette`, and color helpers
- Do not add new `build_*` functions directly to `mod.rs` — create a new sub-module file

### Test after changes

Run `cargo test` after every change. All 60+ tests must pass. Run `cargo build` to check for warnings. The project uses `optimized + debuginfo` dev profile.

## Common Mistakes to Avoid

- **Adding a UI component with input blocking but no state cleanup** — always trace the full lifecycle: open, input dispatch, close, state reset
- **Forgetting `update()` calls for OverlayAnimation users** — without it, `is_active()` stays true forever after close
- **Using `pub(super)` for hit-test functions** — they're called from outside the renderer module
- **Hardcoding layout values in hit-test but not matching the render function** — geometry drift causes click targets to be wrong
- **Using frame-step animation (`+= 0.15` per frame)** — use `Instant`-based timing
- **Adding fields to Renderer without considering sub-module access** — sub-modules access parent fields via `&mut self`, so private fields are accessible within the module tree
