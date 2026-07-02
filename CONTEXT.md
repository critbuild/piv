# piv Context

Shared domain language for product, PRDs, ADRs, and architecture reviews.

## Product

**piv** — a read-only terminal code viewer for watching a project while an editor or agent changes files.

**Viewer** — the running TUI process. It owns the terminal, current tabs, watched root, control socket, and render loop.

**Watched root** — the project directory piv observes. File paths from CLI remote control commands are resolved relative to this root unless absolute.

**Remote control** — one-shot `piv` CLI commands that connect to the Viewer control socket to open a file, jump to a line, move tabs, recenter, or highlight a range.

**Control socket** — the Unix socket derived from the Watched root. It receives newline-delimited Remote control commands.

## Code viewing

**Tab** — one open file in the Viewer. A Tab carries file content, highlighted lines, diff rows, viewport position, focus, selection, and last edit time.

**Tab strip** — the one-line row showing open Tabs, with the active Tab and close targets.

**Code pane** — the bordered area that renders the active Tab’s diff rows.

**Status line** — the bottom row showing active file, diff base, line/change counts, copy notice, and transient highlight status.

**Command line** — the one-row prompt above the Status line used for interactive commands such as `/` search.

**Viewport** — the visible slice of the active Tab’s prepared diff rows.

**Focus line** — the diff row piv wants to bring into view after a change, jump, search, or Remote control command.

**Selection** — a mouse-drag text range in the Code pane that is copied on mouse release.

## Change and diff model

**Snapshot** — piv’s remembered file content for a path. In `HEAD` diff mode, this is used as the old content when no Git reference content exists.

**Diff base** — the reference used to compute the displayed diff. Current values are `HEAD` and `origin/main` with fallback to `HEAD`.

**Diff row** — one rendered line of a diff, tagged as added, removed, or unchanged.

**Prepared row** — a Diff row plus precomputed line number, syntax-highlight spans, whitespace/text length metadata, and a cached static render line.

**Diff hunk** — a contiguous run of changed Diff rows used by `[` and `]` navigation.

## File intake

**Watch event** — a filesystem notification from `notify`, filtered into changed/removed paths.

**Fallback scan** — periodic mtime scan used to catch changes missed by filesystem watching.

**Ignore policy** — shared filtering for Git/build/cache/virtualenv metadata and `.gitignore` rules.

## Rendering overlays

**Remote highlight** — a transient range highlight requested through Remote control.

**Search match** — a text match in the active Tab, shown with normal/current match styling and navigated by `n` / `N`.

**Style overlay** — any style applied over syntax-highlighted spans, including selection, removed-line shading, Remote highlight, and Search match styling.
