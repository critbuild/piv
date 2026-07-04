# PRD: Tracker reading and status UI

Date: 2026-07-04
Status: Shipped

## Problem

The first PRD tracker UI proves the hierarchy works, but it is still too sparse for real planning. Users can see Projects and PRDs, but they cannot comfortably read the original PRD content, inspect all linked Issue details, or visually scan PRD/Issue state. Current text-only status columns such as `complete 0/0` are easy to misread and do not feel like a polished CLI product.

## Goals

- Let users read the original PRD from the tracker UI.
- Let users read every linked Issue for a PRD, including status, blockers, and body/notes when available.
- Add visually pleasing status treatment for PRDs and Issues, inspired by the clarity and polish of the Claude Code CLI.
- Preserve keyboard-first navigation and AI/socket-driven source of truth.

## Non-goals

- Do not add inline editing of PRD or Issue body content in this slice.
- Do not add external issue tracker synchronization yet.
- Do not replace the existing project → PRD → issue tree; improve it with a detail/read pane and better visual language.
- Do not make AI write SQLite directly.

## Users and use cases

- As a piv user, I want to select a PRD and read its original content, so that I can understand the work without leaving the TUI.
- As a piv user, I want to inspect all issues linked to a PRD, so that I can see scope, order, blockers, and details.
- As a piv user, I want statuses to be visually obvious and pleasant, so that I can scan progress at a glance.
- As an AI agent, I want tracker API reads to expose the same PRD and Issue content shown in the UI, so that "read this from piv" returns the authoritative tracker state.

## Requirements

1. The tracker API must expose a PRD read operation that returns key, title, status, source URI, body/original markdown, progress, and linked Issues.
2. The tracker API must expose an Issue read operation that returns key, title, status, body/notes, order, blockers, blocked state, and linked PRDs.
3. The tracker UI must show a detail/read pane for the selected Project, PRD, or Issue on sufficiently wide terminals.
4. Selecting a PRD must render the original PRD body or a clear empty/source-missing state.
5. Selecting a PRD must also make all linked Issues discoverable with status, order, blockers, and body/notes.
6. Selecting an Issue must render the Issue body/notes, status, blockers, and linked PRDs.
7. Statuses must have a compact visual treatment for both tree rows and detail pane:
   - PRD: draft, in progress, complete, archived.
   - Issue: open, in progress, complete, canceled, blocked.
8. Blocked Issues must visually differ from merely open Issues.
9. PRDs with no linked Issues must not show meaningless progress such as `0/0`.
10. Layout must align status and progress columns consistently across projects, PRDs, and issues.
11. The visual style should feel deliberate and modern: subtle separators, selected-row accent, compact badges/glyphs, and readable contrast in a dark terminal.
12. Existing `:prd` navigation must continue to work: `j/k`, `h/l`, `Enter`, `Space`, `p`, `q`, `Esc`, and `Ctrl-C`.
13. Existing tracker JSON-RPC plan registration and `issue.next` behavior must continue to pass tests.

## Interaction details

### Tracker layout

Narrow terminals may keep a single tree pane. Wide terminals should use a two-pane layout:

```text
PRD Tracker

Project / PRD / Issue tree                   Details
──────────────────────────                   ──────────────────────────
▾ piv                              5 PRDs    PRD: Tracker reading and status UI
  ▾ Tracker reading and status UI  ● active  status  ● in progress
    1. ◌ Read original PRD                  source  docs/prd/...
    2. ◌ Read all issues                    progress 0/5
    3. ◆ Status visuals            blocked
```

### Status visual language

Use short badges/glyphs plus aligned text. Exact colors may evolve, but the intended feel is:

```text
PRD badges
◌ draft        muted
● in progress  cyan/blue accent
✓ complete     green
◇ archived     dim

Issue badges
◌ open         muted
● in progress cyan/blue accent
✓ complete    green
− canceled    dim
◆ blocked     amber/red accent
```

Rows should avoid noisy punctuation. Prefer concise, aligned, readable labels over raw enum strings when space allows.

### Detail pane

- Project detail: project key/name, roots/sources, PRD counts.
- PRD detail: title, status badge, source URI, progress, body/original markdown preview, linked Issues.
- Issue detail: title, status badge, PRD links, blockers, body/notes.
- Long body text may be truncated initially; scrolling can be added in this or a follow-up slice if needed.

## Acceptance criteria

- [x] `prd.get` or equivalent tracker read API returns original PRD body/source and linked Issues.
- [x] `issue.get` or equivalent tracker read API returns Issue details, blockers, and PRD links.
- [x] The `:prd` UI shows a detail pane on wide terminals.
- [x] Selecting a PRD shows its original PRD text/body in the detail pane.
- [x] Selecting an Issue shows Issue details in the detail pane.
- [x] PRD and Issue rows use visually distinct status badges/glyphs.
- [x] Blocked Issues are visually distinct from open Issues.
- [x] PRDs with no Issues omit `0/0` progress.
- [x] Status/progress columns align cleanly across rows.
- [x] `Ctrl-C` still quits from tracker mode.
- [x] `cargo test` passes.

## Open questions

- Should the first detail pane render markdown headings/lists specially, or plain wrapped text?
- Should PRD/Issue bodies be scrollable in this slice or truncated with a follow-up task?
- Should the status palette become configurable later?

## References

- `docs/prd/2026-07-04-prd-issue-tracker.md`
- `CONTEXT.md`
- `docs/architecture.md`
