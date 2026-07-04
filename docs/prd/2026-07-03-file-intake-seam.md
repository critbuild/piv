# PRD: File intake seam

Date: 2026-07-03
Status: Shipped

## Problem

`src/app.rs` is the public Viewer orchestration module, but it owns too much file-intake policy internally. Initial file selection, Watch event handling, fallback scans, Remote control opens, removed-file cleanup, Snapshot updates, mtime bookkeeping, content reads, diff creation, syntax highlighting, Prepared row creation, and focus selection are spread across several methods.

This makes small behavior changes risky because similar load paths have intentional differences. Remote open should not overwrite an existing Snapshot, while Watch/fallback change intake should. Fallback scan should update mtime state before dispatching changes. Removed-file intake should clear stale tab and Snapshot state. These invariants should be local and testable.

## Goals

- Introduce an internal File intake Module with a small Interface and behavior-preserving implementation.
- Route initial file selection, Watch/Fallback changed-file handling, removed-file cleanup, and Remote open loading through shared intake logic.
- Keep the external Viewer Interface unchanged: `App::new(root)` and `App::run(terminal)` remain the entry points.
- Preserve current Diff base behavior by passing Git reference content into intake rather than moving Git policy in this slice.
- Improve TDD coverage for file intake behaviors before and during refactor.

## Non-goals

- Do not extract the Git Diff base Module yet.
- Do not redesign `Tab` or `TabManager` lifecycle yet.
- Do not change Remote control protocol or CLI flags.
- Do not change watcher event filtering semantics.
- Do not change Code pane rendering, Search semantics, or overlay order.

## Users and use cases

- As a piv user, I want file changes to keep opening and focusing the newest changed lines, so that the refactor does not affect daily watching.
- As an agent driving piv, I want Remote open/highlight commands to preserve requested line focus, so that guided walkthroughs remain reliable.
- As a piv maintainer, I want file-intake invariants behind a local seam, so that missed-event, Snapshot, and focus bugs can be fixed without editing the full Viewer loop.

## Requirements

1. A File intake Module must own Snapshot and mtime bookkeeping.
2. The Module must expose behavior-level operations for:
   - seed known mtimes from the Watched root,
   - select the newest allowed initial file,
   - scan for missed changed/removed files,
   - load a changed file into a Tab,
   - load a Remote-opened file into a Tab,
   - remove a file from intake state.
3. Changed-file intake must update the Snapshot to the new content and update the seen mtime.
4. Remote-open intake must insert a Snapshot only when one does not already exist.
5. Requested Remote open line numbers must map to Diff rows by new-file line number.
6. Changed-file focus must prefer the latest Snapshot delta and fall back to the last changed Diff row.
7. Fallback scans must continue to use the shared Ignore policy and ignore build/cache/PDF paths.
8. Unreadable text files must continue to render as `<binary or unreadable file>`.
9. Git reference lookup and Diff base fallback must remain in `App` for this slice.
10. Existing public behavior tests must continue to pass.

## Interaction details

No user-facing command, keyboard, mouse, layout, or rendering behavior changes.

Remote control examples must continue to work:

```sh
piv --open src/main.rs:120
piv --highlight-range src/control.rs:24-34
```

The fallback scan remains an idle reliability layer behind the watcher; it should not introduce new visible polling behavior.

## Acceptance criteria

- [x] `src/file_intake.rs` exists and owns Snapshot/mtime state.
- [x] `src/app.rs` delegates initial-file, changed-file, missed-scan, removed-file, and Remote-open intake to the new Module.
- [x] TDD tests cover Remote-open Snapshot preservation and requested line focus.
- [x] TDD tests cover Watch/Fallback changed-file Snapshot/mtime update and latest-change focus.
- [x] TDD tests cover fallback scan changed/removed detection through the Ignore policy.
- [x] Existing git Diff base refresh tests still pass.
- [x] `cargo test` passes.

## Future seam roadmap

After this PRD ships, the next recommended seams remain:

1. Tab lifecycle Module — creation, refresh, focus, scroll clamping, cache invalidation, selection clearing.
2. Git Diff base Module — ref existence checks, object-id polling, reference content lookup, fallback policy, open-tab refresh.
3. Interaction mode Module — search input, tab navigation, selection/copy gestures, focus requests.

## Open questions

- Should the File intake Module eventually own Tab construction fully, or should a later Tab lifecycle Module provide a constructor?
- Should fallback scan timing become injectable for deterministic tests beyond module-level state control?

## References

- `CONTEXT.md`
- `docs/architecture.md`
- `docs/adr/0001-store-product-and-architecture-docs-in-repo.md`
- `docs/prd/2026-07-02-code-pane-refactor-and-flat-ui.md`
