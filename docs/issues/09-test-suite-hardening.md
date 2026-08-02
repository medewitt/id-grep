# Test suite hardening & ID fixtures

**Labels:** `tests`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #2, #3, #4, #5, #6 (accretes as each lands)
**Phase:** 2

## Problem

The current suite hardcodes the CS domain — the single integration test
(`crates/cs-grep-core/tests/e2e.rs`) uses synthetic DBLP/NDSS content, and many
`config.rs`/`main.rs` unit tests assert the CS catalog. To prevent drift as the
pivot lands, the suite must be re-grounded on ID-domain fixtures and cover the
full pipeline offline and deterministically.

## Deliverable

- An ID-domain end-to-end integration test replacing the NDSS one:
  canned source responses → `parse`/map → in-memory DB → query → output.
- A `tests/fixtures/` set: OpenAlex `/works` JSON, PubMed `efetch` XML, a small
  `zotero.sqlite`, and canned publisher HTML for enrichment.
- Coverage across **ingestion → map → dedup → output**, including a
  byte-for-byte snapshot of the JSON output schema on a fixture.

## Approach

1. Build fixtures from small, real (redacted) responses; keep them tiny and
   documented.
2. Replace `e2e.rs` content with an ID example (e.g. a Epidemics/EID record);
   assert the rendered BibTeX/JSON.
3. Add a regression guard: a deliberately corrupted fixture must fail a specific
   test (proves the suite catches parser drift).
4. Ensure all network access is behind `#[ignore]` so `just test` is offline.

## Key files

- `crates/cs-grep-core/tests/e2e.rs` (rewrite)
- `crates/cs-grep-core/tests/fixtures/` (new)
- colocated `#[cfg(test)]` modules touched by #2–#6

## Acceptance criteria

- `just test` is fully offline, deterministic, and green.
- JSON output schema pinned by a snapshot test.
- A deliberate parser regression is caught by a failing test.
