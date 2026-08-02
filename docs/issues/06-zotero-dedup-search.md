# Zotero: dedup + search-your-library

**Labels:** `pivot`, `zotero`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (source abstraction)
**Phase:** 2

## Problem

The maintainer keeps references in a local **Zotero** library and wants `id-grep`
to (a) **de-duplicate** search results against what they already own, and (b)
**search their own library** with the same query language. No Zotero integration
exists today.

## Deliverable

- `crates/cs-grep-core/src/zotero.rs` that reads the local Zotero library
  safely and exposes:
  - **Dedup:** a set of owned DOIs + normalized titles, used to flag or hide
    results (`--exclude-owned`, and an `owned` column in table/JSON output).
  - **Search library:** a `zotero` `Source` that materializes library items as
    `Paper`s into the index (queryable alongside fetched papers).
- Config for the Zotero data-directory path, with platform-default discovery.

## Approach

1. **Tests first:** commit a tiny fixture `zotero.sqlite` under
   `crates/cs-grep-core/tests/fixtures/`; unit-test extraction of owned
   DOIs/titles and materialization of items → `Paper`s; test that
   `--exclude-owned` removes a known-owned result.
2. **Safe read:** copy `zotero.sqlite` to a temp path before opening (Zotero
   holds a write lock while running); read `items` / `itemData` /
   `itemDataValues` / `fields` / `itemTypes` for DOI, title, year, creators.
   Handle attachments/notes exclusion.
3. **Discovery:** resolve the data dir from config → env → platform default
   (`~/Zotero`, `~/.zotero`, macOS `~/Zotero`). Clear error if not found.
4. **Optional enhancement:** Better BibTeX JSON-RPC (`localhost:23119`) when
   available, for stable citation keys — behind a feature flag / config toggle,
   with the SQLite-copy path as the default.

## Key files

- `crates/cs-grep-core/src/zotero.rs` (new)
- `crates/cs-grep-core/src/config.rs` (Zotero data-dir path config)
- `crates/cs-grep-core/src/db.rs` / `output.rs` (`owned` flag surfacing)
- `crates/cs-grep/src/main.rs` (`--exclude-owned`, `zotero` source wiring)

## Acceptance criteria

- Offline unit tests against the fixture `zotero.sqlite` yield correct owned
  DOIs/titles and materialized `Paper`s.
- `--exclude-owned` hides a result known to be in the fixture library.
- Library items are queryable via the normal query language.
- No DB-lock errors when Zotero is running (copy-first verified).
- `just test` green.
