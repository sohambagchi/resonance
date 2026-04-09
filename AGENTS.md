# Agent Instructions — Resonance

This file governs how an AI agent should behave when working on this codebase.
Read it fully before taking any action.

---

## 1. Confirm Before Implementing

Before writing any code for a new feature, present a concise plan to the user
and wait for explicit approval. Do not assume approval from silence or from a
vague "yes, do it" unless the scope is unambiguous. If the plan changes mid-
implementation, pause and re-confirm.

---

## 2. Decision Log (ADR)

Every significant decision — architecture choices, algorithm selection, API
design, dependency additions, deviations from DESIGN.md — must be recorded as
a timestamped entry in `docs/adr/`.

File naming: `docs/adr/YYYY-MM-DD-short-slug.md`

Minimum content per entry:

```
# <Title>

Date: YYYY-MM-DD
Status: Accepted | Superseded by <file>

## Context
<Why this decision arose.>

## Decision
<What was decided.>

## Consequences
<Trade-offs, follow-up tasks, anything that changes because of this.>
```

Create the `docs/adr/` directory if it does not exist. Do not skip this step.

---

## 3. Global TODO List

Maintain a running task list at `docs/TODO.md`.

- Append new tasks as they are identified; never delete old ones.
- Mark completed tasks with `[x]`; leave pending tasks as `[ ]`.
- Keep tasks in chronological order (newest at the bottom).
- Group tasks under dated headings when a batch is added at once.

Format:

```markdown
## YYYY-MM-DD

- [x] Completed task description
- [ ] Pending task description
```

Update this file whenever a task is created or completed — not in a batch at
the end of a session.

---

## 4. Knowledge Base

`docs/knowledge/` is a **read-only** reference. Do not modify its contents.
Consult it when implementing algorithms or making design decisions:
the prior implementations documented there are the primary source of design
lessons encoded in `docs/DESIGN.md`.

Relevant files:
- `docs/knowledge/00-knowledge-index.md` — index of all knowledge entries
- Individual entries cover Calibrator (Manegold, fukien, magsilva forks),
  lmbench, tinymembench, and the X-Ray paper.

---

## 5. Documentation

### Inline documentation
All public items (`pub fn`, `pub struct`, `pub trait`, `pub mod`) must have a
doc comment (`///`). Significant `unsafe` blocks must have a `// SAFETY:`
comment explaining the invariants being upheld.

Keep comments accurate. Outdated comments are worse than none.

### Per-component guides
For every significant component of the project, create a corresponding guide
file in `docs/guide/`. "Significant" means a module or subsystem that a new
contributor would need to understand independently (e.g., the buffer manager,
timing infrastructure, a measurement kernel family, the analysis pipeline).

File naming: `docs/guide/<component>.md`

Keep guides minimal: explain the *why* and the non-obvious *how*. Do not
re-state what the code already says clearly.

### Index
`docs/INDEX.md` is the single entry point for all project documentation.
Keep it up to date whenever a new guide or ADR is added. Structure:

```markdown
# Resonance — Documentation Index

## Design
- [DESIGN.md](docs/DESIGN.md)

## Guides
- [guide/...](docs/guide/...)

## Architecture Decision Records
- [adr/...](docs/adr/...)

## TODO
- [TODO.md](docs/TODO.md)
```

Do not produce redundant documentation. If something is already fully covered
in `DESIGN.md` or inline doc comments, the guide entry should link there
rather than repeat it.

---

## 6. Code Style

Follow these rules consistently throughout the codebase:

- **Edition**: Rust 2021.
- **Formatting**: `rustfmt` defaults. Run `cargo fmt` before committing.
- **Lints**: resolve all `cargo clippy -- -D warnings` warnings before
  committing. Do not `#[allow(...)]` a lint without a comment explaining why.
- **`unsafe` blocks**: keep them as small as possible. Each `unsafe` block
  must be accompanied by a `// SAFETY:` comment.
- **Error handling**: use `Result` with descriptive error types; do not
  `unwrap()` or `expect()` in library code. `unwrap()` is permitted only in
  tests and in `main` after a clear diagnostic message.
- **Floating point**: use `f64` everywhere. Never introduce `f32` without an
  explicit discussion and ADR entry.
- **Naming**: follow Rust API guidelines — `snake_case` for functions and
  variables, `CamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- **No dead code**: do not leave commented-out code or unused items in
  committed files.
- **No external logging frameworks**: use `eprintln!` for `--verbose` stderr
  output and buffer all stdout output until measurement is complete (see
  DESIGN.md §16.3).

---

## 7. Clarify Ambiguities

Do not make assumptions when requirements, behaviour, or constraints are
unclear. Ask a focused, specific question before proceeding. One clarifying
question is better than an incorrect implementation that must be unwound.

This applies to: algorithm parameter choices, API surface decisions, platform-
specific behaviour, output format details, and anything not addressed by
`DESIGN.md` or the knowledge base.
