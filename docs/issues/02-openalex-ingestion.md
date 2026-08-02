# OpenAlex ingestion source (primary)

**Labels:** `pivot`, `ingestion`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (source abstraction)
**Phase:** 2

## Problem

OpenAlex is the primary ingestion backend for the pivot — it covers ecology,
biomedical, and preprint literature and can be filtered by journal ISSN. The repo
already parses OpenAlex responses for abstract enrichment; that parsing should be
reused to build a full ingestion source.

## Deliverable

- `crates/cs-grep-core/src/sources/openalex.rs` implementing `Source`:
  fetch all works for a venue's ISSN(s) / source-id within a year range, mapped
  to `Paper` with reconstructed abstracts.

## Approach

1. **Tests first:** unit test mapping a canned `/works` JSON page (fixture) to
   `Paper`s — asserting title, authors (signature order), year, DOI, and an
   abstract reconstructed from `abstract_inverted_index`. Cursor-pagination
   handling tested with a two-page fixture.
2. Query shape:
   ```
   GET https://api.openalex.org/works
     ?filter=locations.source.issn:<issn>,from_publication_date:<y>-01-01,to_publication_date:<y>-12-31
     &per-page=200&cursor=*
   ```
   Prefer `openalex_source_id` (`primary_location.source.id:Sxxxx`) when present.
3. Reuse existing OpenAlex helpers in `abstracts.rs`
   (`abstract_from_openalex` / inverted-index reconstruction,
   `abstracts_from_openalex_works`) — extract to a shared module if needed.
4. Send `mailto` (polite pool) and optional `OPENALEX_API_KEY`; retry without the
   key on 401/403 (existing pattern in `abstracts.rs`).
5. Wire into `cmd_update` via the `Source` trait.

## Key files

- `crates/cs-grep-core/src/sources/openalex.rs` (new)
- `crates/cs-grep-core/src/abstracts.rs` (reuse OpenAlex parsing)
- `crates/cs-grep-core/src/config.rs` (`Secrets` — add `openalex_mailto`)
- `crates/cs-grep/src/main.rs` (`cmd_update`)

## Acceptance criteria

- Offline unit tests map canned single- and multi-page `/works` JSON to correct
  `Paper`s incl. inverted-index abstracts.
- A live smoke test (`#[ignore]`d) ingests a small real ISSN + year window.
- Polite-pool `mailto` sent; missing/invalid API key degrades gracefully.
- `just test` green.
