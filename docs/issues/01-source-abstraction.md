# Source abstraction + model generalization

**Labels:** `pivot`, `ingestion`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** none (foundation — do first, alongside #8)
**Phase:** 1

## Problem

Ingestion is hard-wired to DBLP. `crates/cs-grep-core/src/dblp.rs` is the only
source, there is no abstraction over it, and CS-specific assumptions leak into
the data model: `Paper.dblp_key` is the primary key, and `Venue.dblp_stream` is
a required field that only makes sense for DBLP. Before any new source can be
added, ingestion and the model must become source-agnostic.

## Deliverable

- A `trait Source` in a new `crates/cs-grep-core/src/sources/` module:
  ```rust
  pub trait Source {
      /// Fetch all papers for a venue within [min_year, max_year].
      fn fetch_venue(&self, venue: &Venue, min_year: i32, max_year: i32) -> Result<Vec<Paper>>;
  }
  ```
- Generalized data model:
  - `model.rs`: rename `Paper.dblp_key` → `key`; add `source: String`
    (e.g. `"openalex"`, `"pubmed"`, `"zotero"`). Keep `cite_key()` behavior.
  - `config.rs`: replace required `Venue.dblp_stream: String` with
    source-agnostic identifiers — `issn: Vec<String>`,
    `openalex_source_id: Option<String>`, `pubmed_journal: Option<String>`
    (NLM title abbreviation) — plus existing `id/name/aliases/rank/tags`.
  - `db.rs`: the `UNIQUE` key column and upsert follow the `key` rename; dedup
    on `doi` when present, else normalized title.
- DBLP retired for the full pivot (delete `dblp.rs` and the OpenReview client),
  **or** left behind the trait as a dormant/legacy source — recommend delete to
  keep a single-domain tool clean (git history preserves it).

## Approach

1. **Tests first:** adapt existing pure-parse tests to the renamed
   `Paper.key`/`source` and new `Venue` fields; add a trait-level test with a
   fake in-memory `Source` returning canned `Paper`s to prove the
   `update` command drives an arbitrary source.
2. Introduce the trait + `sources/mod.rs`; move/remove DBLP accordingly.
3. Thread `source` through `db.rs` upsert/search and `output.rs`.
4. Decide DB migration vs. rebuild for the `dblp_key`→`key` column — **rebuild is
   simplest** given local-index semantics; bump a DB version constant and
   recreate on mismatch.

## Key files

- `crates/cs-grep-core/src/model.rs`
- `crates/cs-grep-core/src/config.rs` (`Venue` struct ~L18-30)
- `crates/cs-grep-core/src/db.rs` (schema, UNIQUE key, upsert)
- `crates/cs-grep-core/src/dblp.rs` (retire)
- `crates/cs-grep/src/main.rs` (`cmd_update` wiring)

## Acceptance criteria

- Workspace builds; `just test` green.
- A fake `Source` implementation can be ingested end-to-end in a unit/integration
  test (no network).
- No remaining references to `dblp_key`; `Venue` no longer requires
  `dblp_stream`.
- DB rebuild-on-version-mismatch path covered by a test.
