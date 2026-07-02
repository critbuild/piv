# ADR-0001: Store product and architecture docs in the repository

Date: 2026-07-02
Status: Accepted

## Context

piv is small enough that product intent, domain language, and architecture decisions can drift faster than the code. The codebase previously had no `CONTEXT.md`, PRD folder, or ADR folder, so future architecture reviews and implementation sessions had no durable project memory beyond commit messages.

We want agents and humans to share the same vocabulary before changing Modules or introducing new Seams.

## Decision

Store durable project memory in the repository:

- `CONTEXT.md` for shared domain language.
- `docs/prd/` for product requirements and feature specs.
- `docs/adr/` for decisions that should affect future architecture work.
- `docs/architecture.md` for the current Module map and architecture review candidates.

PRDs describe desired product behavior and constraints. ADRs record decisions after enough context exists that future contributors should not re-open the same question casually.

## Consequences

- Architecture reviews should read `CONTEXT.md` and `docs/adr/` before suggesting changes.
- New feature work can start with a PRD when behavior or scope is unclear.
- Non-obvious architectural choices should be captured as ADRs rather than hidden in chats or commit messages.
- Docs can become stale; updates should be part of the same change that alters the product behavior or decision.

## Alternatives considered

- Keep docs outside the repo: rejected because agents and contributors would not reliably load them.
- Use only commit messages: rejected because decisions are hard to discover by topic.
- Add a large docs process: rejected because piv needs lightweight project memory, not ceremony.
