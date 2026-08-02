# LLM/agent ergonomics + `CLAUDE.md`

**Labels:** `pivot`, `docs`, `tooling`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (schema shape); finalize alongside #7
**Phase:** 3

## Problem

The maintainer will call `id-grep` from Claude Code as a tool after running their
own literature searches. The tool must therefore be cleanly drivable by an
agent: predictable machine-readable output, no interactive prompts, and a
documented contract the calling agent can rely on.

## Deliverable

- **Stable, versioned JSON contract:** `--format json` emits a documented schema
  — an array of records with a fixed field set plus a top-level
  `schema_version`. Errors are serialized to JSON on stderr under `--format
  json`.
- **Non-interactive operation:** `--quiet` suppresses progress; no command ever
  prompts; the TUI is never required for any capability.
- **Distinct exit codes:** e.g. `0` ok, and non-zero codes for
  config/usage error, network/source error, and no-results — documented.
- **A root `CLAUDE.md`** documenting canonical invocations, the JSON schema,
  how to point at a Zotero library, how to dedup (`--exclude-owned`), and
  copy-pasteable examples for an agent.

## Approach

1. **Tests first:** snapshot-test the JSON schema on a fixture (see #9); test
   each documented exit code (ok / no-results / bad-config); test that JSON-mode
   errors are valid JSON on stderr.
2. Add `schema_version` + freeze the record field set in `output.rs`.
3. Add `--quiet` and route progress/log output to stderr only.
4. Define an exit-code enum in `main.rs` and map error categories to it.
5. Write `CLAUDE.md` with real, runnable examples (query, JSON, dedup, update).

## Key files

- `crates/cs-grep-core/src/output.rs` (JSON schema + `schema_version`)
- `crates/cs-grep/src/main.rs` (`--quiet`, exit codes, JSON error path)
- `CLAUDE.md` (new, repo root)

## Acceptance criteria

- `id-grep --format json '<query>' --quiet` emits the documented schema with
  `schema_version`; pinned by a snapshot test.
- A no-results query and a bad-config invocation return the documented distinct
  exit codes.
- JSON-mode errors are valid JSON on stderr.
- `CLAUDE.md` examples run as written; `just check` green.
