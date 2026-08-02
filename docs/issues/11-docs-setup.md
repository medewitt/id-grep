# Docs & setup

**Labels:** `docs`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #7 (rename), #10 (agent contract)
**Phase:** 3

## Problem

The README documents the CS workflow (DBLP, CS venues). After the pivot it must
describe the ID workflow, the new sources and their credentials, `just` usage,
and Zotero setup — so a new user (or an agent) can go from zero to results.

## Deliverable

- README rewritten for the `id-grep` / ID workflow.
- Setup docs for credentials and Zotero; `.env.example` updated.

## Approach

1. Rewrite README: what `id-grep` is, install (incl. `just`), quickstart
   (`init → update → query → enrich`), the bundles/catalog, the query language,
   output formats, and a pointer to `CLAUDE.md` for agent use.
2. Document credentials and add to `.env.example`:
   - OpenAlex polite pool `mailto` (recommended email).
   - `NCBI_API_KEY` + `NCBI_EMAIL` for PubMed rate limits.
   - existing `OPENALEX_API_KEY` / `SEMANTIC_SCHOLAR_S2_KEY`.
3. Document Zotero setup: data-dir discovery/override, the copy-first read,
   `--exclude-owned`, and optional Better BibTeX.
4. Cross-link the `NOTICE`/Credits from #7.

## Key files

- `README.md`
- `.env.example`
- links to `CLAUDE.md`, `NOTICE`

## Acceptance criteria

- A new user can follow the README from install to a first query + dedup.
- All credentials documented and present in `.env.example`.
- Zotero setup documented; README references `CLAUDE.md` and `NOTICE`.
