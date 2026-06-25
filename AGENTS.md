# Agent Notes

## piv

If `piv` is running for this repo, prefer remote control over fake file edits when the user wants navigation only.

### Remote control

```sh
piv --open src/main.rs:120
piv --open path/to/file
piv --highlight src/main.rs:120
piv --highlight-range src/control.rs:24-34
piv --line 200
piv --next-tab
piv --prev-tab
piv --recenter
```

If sending from another cwd:

```sh
piv --root /path/to/project --open src/main.rs:120
```

Restart `piv` after rebuilding so the socket server is available.

## Build

```sh
cargo test
cargo build --release
```
