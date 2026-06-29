# piv

`piv` is a read-only terminal code viewer.

It watches a project directory for file changes, opens changed files in tabs, highlights code, and centers on the latest change. It also supports remote control so agents or scripts can jump to files and lines without editing anything.

## Features

- filesystem watch for create, write, rename, delete
- LRU tab strip
- change gutter for added and modified lines
- tree-sitter highlighting for `rs`, `ts`, `tsx`, `js`, `jsx`
- mouse wheel scroll
- clickable tabs
- mouse text selection
- remote open, jump, and highlight commands

## Run

```sh
piv
# or
piv /path/to/project
```

## Remote control

Restart `piv` after upgrading so the control socket is available.

```sh
piv --open src/main.rs:120
piv --open test.md
piv --highlight src/main.rs:120
piv --highlight-range src/control.rs:24-34
piv --line 200
piv --next-tab
piv --prev-tab
piv --recenter
```

From another directory:

```sh
piv --root /path/to/project --open src/main.rs:120
```

## Controls

- `Tab` and `Shift-Tab` switch tabs
- arrow keys and page keys scroll
- `[` and `]` jump between diff hunks
- `\` toggles diff base between `HEAD` and `origin/main`
- `c` re-center
- `q` or `Ctrl-C` quit
- mouse wheel scrolls
- click tab row to switch tabs
- drag in code pane to select text
