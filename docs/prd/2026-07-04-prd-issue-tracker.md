# PRD: PRD and issue tracker

Date: 2026-07-04
Status: Shipped

## Problem

piv can show code changes, but it has no durable way for humans and AI agents to track product requirements, ordered implementation issues, blockers, or the relationship between issues and PRDs. Agents need an intent-level interface to register plans and update progress instead of editing markdown ad hoc or screen-scraping the TUI.

## Goals

- Add a piv-owned PRD/issue registry backed by SQLite.
- Let AI create projects and atomically register PRD plans through a local structured API.
- Track PRD status, issue status, PRD/issue links, explicit issue order, and blocking issue dependencies.
- Add a full-screen `:prd` tracker UI organized by project, then PRD, then ordered issues.
- Provide a socket/API shape that can be wrapped by the piv skill and future MCP tools.

## Non-goals

- Do not require manual project creation in the TUI.
- Do not make the TUI the only way to mutate tracker data.
- Do not let AI write SQLite directly.
- Do not derive issue order only from dependency graphs; explicit PRD issue order remains canonical.
- Do not implement every possible external tracker adapter in the first slice.

## Users and use cases

- As an AI agent, I want to register a PRD plan with ordered issues and blockers in one call, so that piv can validate and persist the implementation plan.
- As a piv user, I want `:prd` to show projects, PRDs, and expandable ordered issues, so that I can see what work is next.
- As an AI agent, I want to ask for the next unblocked issue, so that I can execute plans in order.
- As a piv user, I want status changes to be reflected in the tracker UI and API, so that progress stays shared across humans and agents.

## Requirements

1. The tracker registry stores projects, project roots/sources, PRDs, issues, PRD/issue links, issue order, issue blockers, and events in SQLite.
2. Projects are explicit persisted records created by AI/API, not implicitly only the current watched root.
3. PRDs support `draft`, `in_progress`, `complete`, and `archived` statuses.
4. Issues support `open`, `in_progress`, `complete`, and `canceled` statuses.
5. Blocked issue state is derived from incomplete blocker issues.
6. `prd.upsert_plan` atomically creates or updates one PRD, its ordered issues, PRD/issue links, and blockers.
7. `prd.upsert_plan` rejects invalid blocker keys and dependency cycles without partially saving the plan.
8. The tracker API accepts stable AI-friendly keys such as `project/prd/issue` and also returns opaque internal IDs.
9. `issue.next` returns the first non-complete, non-canceled, non-blocked issue by explicit PRD issue order.
10. The tracker exposes JSON-RPC over a local Unix socket independent of the TUI.
11. `:prd` opens a full-screen tracker mode.
12. The tracker UI renders projects, expandable PRDs, and ordered issues with status glyphs and blocker labels.
13. The tracker UI supports `j/k`, `h/l`, `Enter`, `Space`, `p`, `q`, and `Esc` for navigation/status changes/exit.
14. The API surface should be usable by an enhanced piv skill: “send this to piv” maps to `prd.upsert_plan`; “read from piv” maps to project/PRD/issue query methods.

## Interaction details

### Command mode

- `:` opens a command prompt.
- Typing `prd` and pressing Enter enters full-screen tracker mode.
- `Esc` cancels the command prompt.

### Tracker UI

```text
PRD Tracker                                      :prd

▾ piv                                           3 PRDs
  ▾ PRD and issue tracker                      in-progress  2/8
    1. [x] Create SQLite registry
    2. [~] Add tracker socket
    3. [!] Add MCP adapter          blocked by socket-api
    4. [ ] Add full-screen :prd UI
```

Keymap:

```text
j/k       move cursor
h         collapse / parent
l/Enter   expand / enter
Space     cycle selected issue status
p         cycle selected PRD status
q/Esc     leave tracker
```

## API details

JSON-RPC requests are newline-delimited over the tracker socket.

Example plan registration:

```json
{
  "jsonrpc": "2.0",
  "id": "42",
  "method": "prd.upsert_plan",
  "params": {
    "project_key": "piv",
    "prd": { "key": "prd-tracker", "title": "PRD and issue tracker", "status": "in_progress" },
    "issues": [
      { "key": "sqlite-registry", "title": "Create SQLite tracker registry", "position": 1 },
      { "key": "socket-api", "title": "Add JSON-RPC tracker socket", "position": 2, "depends_on": ["sqlite-registry"] },
      { "key": "prd-ui", "title": "Add full-screen :prd tracker UI", "position": 3, "depends_on": ["sqlite-registry", "socket-api"] }
    ]
  }
}
```

## Acceptance criteria

- [x] A behavior test can create a project and atomically upsert a PRD plan with ordered issues and blockers.
- [x] A behavior test proves invalid/cyclic blockers do not partially save a plan.
- [x] A behavior test proves `issue.next` skips blocked and completed issues according to PRD order.
- [x] A behavior test proves JSON-RPC can register a plan and query the next issue without direct store access.
- [x] A behavior test proves tracker UI rows are project → PRD → ordered issues with blocker/status glyphs.
- [x] A behavior test proves `:` then `prd` enters tracker mode.
- [x] Existing remote control behavior and tests still pass.
- [x] `cargo test` passes.

## Open questions

- Should future adapters sync status changes back into source markdown frontmatter?
- Should the first MCP adapter live in this repo or in a separate pi package?

## References

- `CONTEXT.md`
- `docs/architecture.md`
- `docs/adr/0001-store-product-and-architecture-docs-in-repo.md`
