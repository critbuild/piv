# Architecture

Last reviewed: 2026-07-02

Read first:

- `CONTEXT.md` for product/domain language.
- `docs/adr/` for decisions that future architecture work should respect.

## Current Module map

- `src/main.rs` — process entry point. Chooses Watch mode vs Remote control mode and starts the TUI.
- `src/cli.rs` — CLI parsing, target parsing, and Control socket path derivation.
- `src/control.rs` — Control socket server/client and Remote control text protocol.
- `src/code_pane.rs` — Code pane Prepared row creation, viewport line rendering, gutter formatting, static viewport caching, and Style overlay composition.
- `src/watcher.rs` — filesystem Watch event intake and Ignore policy.
- `src/diff.rs` — old/new content to Diff rows.
- `src/highlight.rs` — tree-sitter syntax highlighting with plain-text fallback.
- `src/search.rs` — in-file Search query matching and match cycling.
- `src/model.rs` — Tab, TabManager, Selection, Prepared row, and viewport cache data.
- `src/ui.rs` — terminal setup/teardown wrapper.
- `src/app.rs` — Viewer orchestration: event loop, file intake, Git Diff base lookup, Tab lifecycle, input handling, rendering, Style overlays, clipboard integration, search state, and many helper functions/tests.

## Architecture notes

Externally, `App` is a deep Module: callers only need `App::new(root)` and `App::run(terminal)` to get a full Viewer. Internally, `src/app.rs` has accumulated many concepts. That is not automatically bad, but it makes local reasoning harder because change intake, input mode, Git reference lookup, and Tab invariants are all edited in one file. Code pane row rendering has now been deepened into `src/code_pane.rs`.

The best near-term architecture work should keep the external Viewer Interface small while introducing internal Modules that improve Locality and testability.

## Deepening opportunities

### 1. Deepen the Prepared viewport / Style overlay Module

**Status** — Shipped in `docs/prd/2026-07-02-code-pane-refactor-and-flat-ui.md`.

**Files** — `src/code_pane.rs`, `src/app.rs`, `src/model.rs`, `src/highlight.rs`, `src/search.rs`.

**Problem** — Rendering required the caller to know too much about character offsets, code prefix width, syntax-highlight spans, selection ranges, removed-line shading, Remote highlight timing, Search match styling, cached static lines, and viewport height. The `/` prompt overlap bug was a symptom: layout/prompt rendering lived near code-pane rendering but the Code pane did not own a clear rendering contract.

**Solution** — Concentrated row preparation and Style overlay application behind `src/code_pane.rs`. Syntax highlighting and raw search matching remain separate, while Code pane rendering combines prepared rows plus active overlays into terminal `Line`s.

**Benefits** — Locality improved because rendering bugs in selection/search/highlight overlap now live in one place. Leverage improved because tests exercise overlay composition through the same Interface used by the Viewer instead of reaching many helper functions.

### 2. Deepen the File intake Module

**Files** — `src/app.rs`, `src/watcher.rs`, `src/diff.rs`, `src/highlight.rs`, `src/model.rs`.

**Problem** — The Viewer currently handles Watch events, Fallback scan mtimes, removals, snapshots, initial-file selection, content reads, diff creation, syntax highlighting, focus-line choice, and Tab insertion. The same load path is nearly repeated for `open_path` and `load_change`, with subtle differences around Snapshot updates and focus selection.

**Solution** — Create an internal Module around File intake that turns paths plus intent (`initial`, `watch changed`, `remote open`, `removed`) into Tab updates or removals. The Module should own Snapshot/mtime bookkeeping and focus selection policy.

**Benefits** — Locality improves for filesystem race and missed-event bugs. Leverage improves because Watch events, Fallback scans, and Remote control opens can share one tested path for producing Tab state.

### 3. Deepen the Git Diff base Module

**Files** — `src/app.rs`, `src/diff.rs`.

**Problem** — Git reference concerns are mixed into Viewer state: `DiffBase`, ref existence checks, object id polling, `git show`, fallback from `origin/main` to `HEAD`, and refresh of open Tab diffs. Callers must understand when Git ref movement should invalidate Search, refresh prepared rows, and update last-seen refs.

**Solution** — Move Diff base policy and Git reference content lookup into one internal Module. Keep shelling out to `git` if that remains sufficient, but make the policy coherent and testable without spreading Git details through Viewer orchestration.

**Benefits** — Locality improves for Git edge cases such as missing `origin/main`, new files, deleted files, and post-push ref movement. Leverage improves because the Viewer can ask for reference content/current revision without knowing the fallback policy.

### 4. Deepen the Interaction mode Module

**Files** — `src/app.rs`, `src/search.rs`, `src/model.rs`.

**Problem** — Keyboard and mouse handling currently mutate Viewer and Tab fields directly. Search input mode short-circuits key handling, Tab changes manually invalidate search in several places, and selection/copy behavior is coupled to mouse events. This spreads state-transition knowledge across handlers.

**Solution** — Concentrate input modes and their state transitions. The Viewer should still own terminal events, but a smaller Module should decide how keys/mouse gestures affect Search state, Selection state, Tab navigation, and focus requests.

**Benefits** — Locality improves for interaction bugs. Leverage improves because tests can describe user actions and assert state transitions without rendering a terminal.

### 5. Deepen the Tab lifecycle Module

**Files** — `src/model.rs`, `src/app.rs`.

**Problem** — `Tab` is a public struct with many fields that must stay consistent: content, highlighted lines, Diff rows, Prepared rows, cache invalidation, focus, scroll, auto-centering, selection, and last edit. Callers construct and mutate these fields directly, so the Interface is nearly as complex as the Implementation.

**Solution** — Give the Tab lifecycle one owning Module for creation, refresh, focus, scroll clamping, cache invalidation, and selection clearing. `TabManager` can then manage ordering/active index while Tab lifecycle owns per-Tab invariants.

**Benefits** — Locality improves because Tab invariants live in one place. Leverage improves because adding features like search, hunk navigation, or Remote highlight does not require every caller to remember which fields to reset.

## Suggested exploration order

1. File intake Module — reduces duplication and makes watcher/fallback behavior easier to test.
2. Tab lifecycle Module — smaller refactor that supports both rendering and intake work.
3. Git Diff base Module — valuable, but only after File intake boundaries are clearer.
4. Interaction mode Module — useful once rendering and Tab lifecycle have firmer seams.
