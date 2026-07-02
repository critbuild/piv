# PRD: Code pane refactor and flat UI

Date: 2026-07-02
Status: Shipped

## Problem

The Code pane is doing too much inside `src/app.rs`. Rendering the active Tab currently mixes viewport slicing, row caching, line-number/gutter formatting, selection styling, removed-line styling, Remote highlight styling, Search match styling, and ratatui `Line` construction in one large Viewer method.

This makes UI changes risky. The recent `/` Search prompt issue showed that layout/rendering behavior was not isolated behind a clear Module Interface. The current bordered Code pane also feels heavy for a read-only code viewer; users want the code area to look more like an editor pane and less like a boxed widget.

## Goals

- Deepen the Code pane rendering seam without changing the external Viewer Interface.
- Move Code pane line construction and Style overlay composition out of `src/app.rs`.
- Make overlay ordering explicit and testable.
- Replace the full Code pane border with a flatter editor-like UI.
- Keep existing watcher, diff, search, tab, clipboard, and Remote control behavior intact.

## Non-goals

- Do not redesign the Tab lifecycle Module.
- Do not change Git Diff base behavior.
- Do not change Search query semantics.
- Do not add horizontal scrolling.
- Do not introduce a theme system yet.

## Users and use cases

- As a piv user, I want the Code pane to feel like an editor viewport, so that code is visually primary rather than boxed inside a widget.
- As a piv maintainer, I want Code pane rendering behind a small Interface, so that selection, Search, and highlight UI bugs can be fixed locally.
- As an agent driving piv, I want Remote highlights and Search matches to compose predictably, so that guided walkthroughs remain readable.

## Requirements

1. `src/app.rs` must delegate Code pane line construction to a dedicated Module.
2. The new Module must own Prepared row creation, static viewport caching, gutter formatting, and Style overlay composition.
3. The new Module Interface must be small: given a Tab, viewport height, and active overlays, return ratatui lines for the Code pane.
4. Overlay order must be explicit:
   - syntax highlighting
   - removed-line background
   - Remote highlight
   - selection
   - Search match
   - current Search match
5. The Code pane must render without a full surrounding border.
6. The flat UI must preserve visual structure with a line-number/change gutter and subtle vertical separator.
7. Mouse selection coordinates must still map to the correct text columns after the borderless UI change.
8. Viewport height calculations must use the full Code pane area once the border is removed.
9. Existing tests must pass, and new tests must cover the new Code pane Module behavior.

## Interaction details

### Code pane

The Code pane no longer draws `Block::borders(Borders::ALL)`. It fills its allocated area directly.

Each rendered row uses a flat gutter:

```text
  42 + │ code here
  43   │ unchanged code
  44 - │ removed code
```

The gutter remains fixed width so mouse selection can subtract the same prefix width when mapping terminal coordinates to text columns.

### Search command line

The Search command line remains a dedicated one-row prompt above the Status line. This PRD does not change Search prompt behavior beyond ensuring the Code pane height responds correctly to the borderless pane.

### Empty state

When no file is open, piv renders a plain muted message in the Code pane area instead of a bordered placeholder box.

## Acceptance criteria

- [x] Code pane rendering logic lives in a dedicated Module.
- [x] `App::render_code` no longer builds per-row overlay spans directly.
- [x] Code pane renders without a full border.
- [x] Gutter includes line number, change marker, and a subtle separator.
- [x] Mouse selection maps text columns correctly in the borderless Code pane.
- [x] Search matches still highlight and `n` / `N` still cycle.
- [x] Remote highlights still fade and compose with other overlays.
- [x] Removed lines still have their background style.
- [x] Static viewport cache still works when no dynamic overlays are active.
- [x] `cargo test` passes.

## Implementation plan

1. Add `src/code_pane.rs`.
2. Move Prepared row creation and span-splitting helpers into `code_pane`.
3. Add `CodePaneOverlays`, `RemoteHighlightOverlay`, and `SearchOverlay` structs.
4. Expose `render_code_pane(tab, height, overlays)` and `prepare_rows(...)`.
5. Make `src/app.rs` build overlay inputs and delegate to `render_code_pane`.
6. Remove the Code pane border and update viewport/mouse-coordinate math.
7. Add behavior tests for gutter rendering, selection overlay, Search overlay, and cache behavior.
8. Run `cargo test`.

## Open questions

- Should the flat gutter become theme-configurable later?
- Should Search prompt and Status line share a command/status styling Module later?

## References

- `CONTEXT.md`
- `docs/architecture.md`
- `docs/adr/0001-store-product-and-architecture-docs-in-repo.md`
