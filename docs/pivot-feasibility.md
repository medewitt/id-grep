# Feasibility plan: pivot `cs-grep` → `id-grep`

*Infectious-disease ecology, evolution & epidemiology literature search.*

The actionable backlog lives in [`docs/issues/`](issues/README.md) (an epic +
11 sub-issues). This document is the feasibility rationale behind it.

## Context

`cs-grep` (a fork of `philippnormann/cs-grep`, descended from `sec-grep`) is a
Rust CLI + TUI that ingests computer-science paper metadata from **DBLP's SPARQL
endpoint** into a local **SQLite + FTS5** index, exposes a query language
(`text WHERE filters`), and enriches abstracts from OpenAlex / Semantic Scholar /
Crossref / OpenReview. The goal is to repurpose it for **infectious-disease
ecology, evolution, and epidemiology**, driven by OpenAlex + PubMed, scoped to a
curated **journal** list (plus bioRxiv/medRxiv preprints), with **local Zotero**
support for searching one's own library and de-duplicating results.

The hard constraint: **DBLP only indexes CS**, so the target journals are
unreachable through it. The pivot replaces the **ingestion backend**, not just a
journal list. Two assets make this tractable: (1) working OpenAlex / Semantic
Scholar / Crossref clients already exist in `abstracts.rs` (used today only for
abstract enrichment, incl. OpenAlex `abstract_inverted_index` reconstruction);
(2) the venue catalog already doubles as both the ingestion list and the search
vocabulary, so swapping the catalog swaps both.

## Decisions (locked)

- **Full pivot + rename** to **`id-grep`**; CS bundles removed. Preserve upstream
  **attribution/citations**.
- **Ingestion:** OpenAlex primary + **PubMed** (NCBI E-utilities) complementary.
- **Zotero:** **search your own library** + **dedup / flag already-owned** (no
  push-to-Zotero). Access = read a **copy of `zotero.sqlite`**, optional Better
  BibTeX support.
- **Search axis:** journal-scoped. **Preprints (bioRxiv, medRxiv) included as
  venues**, optional to query.

## Feasibility summary

Feasible with moderate effort. The dominant work is a new source abstraction plus
two ingestion clients; most surrounding machinery (DB, FTS, query language,
output formats, TUI, HTTP/SSRF plumbing, OpenAlex/Crossref parsing) is reused
as-is.

| Area | Verdict | Notes |
|---|---|---|
| Replace DBLP with OpenAlex+PubMed | Feasible, main effort | New `Source` trait + 2 clients; OpenAlex parsing largely exists |
| Journal catalog by ISSN | Feasible | Resolve ISSN / OpenAlex source-id per journal at build time |
| Preprints as venues | Feasible | OpenAlex indexes bioRxiv/medRxiv sources |
| Zotero (search + dedup) | Feasible | Read a copy of `zotero.sqlite`; DOI/title match for dedup |
| Rename + attribution | Mechanical | Crate/binary/paths/README + preserve LICENSE, add NOTICE |
| Tests / CI | Mechanical + additive | Fix catalog-coupled tests; add GitHub Actions (none today) |

## Cross-cutting requirements

- **Build/test via `just`** — a root `justfile` is the single control surface;
  CI calls the same recipes so local and CI never drift.
- **Test-driven development** — tests land first; network tests are `#[ignore]`d
  and fed canned fixtures so the default suite is offline and deterministic.
- **LLM / agent-first ergonomics** — JSON is a first-class, versioned,
  documented contract; non-interactive by default; machine-readable failures; a
  root `CLAUDE.md` documents the contract for calling agents.

## Target architecture

1. **Source abstraction** — `trait Source { fn fetch_venue(...) }` in
   `sources/`; generalize `Paper.dblp_key` → `key` (+ `source`) and
   `Venue.dblp_stream` → `issn` / `openalex_source_id` / `pubmed_journal`; retire
   DBLP + OpenReview.
2. **OpenAlex ingestion (primary)** — `/works?filter=locations.source.issn:…`
   with cursor paging; reuse existing OpenAlex parsing; polite-pool `mailto`.
3. **PubMed ingestion (complementary)** — E-utilities `esearch`→`efetch`,
   XML→`Paper` incl. DOI + MeSH; rate-limited; `NCBI_API_KEY`/`NCBI_EMAIL`.
4. **Enrichment routing** — add bio-publisher DOI prefixes/selectors
   (PLOS/Elsevier/OUP/Royal Society/bioRxiv/medRxiv/EID/PNAS/Wiley); retire
   OpenReview.
5. **Zotero** — copy `zotero.sqlite`, read items for dedup + a searchable
   `zotero` source; optional Better BibTeX.
6. **Rename + attribution** — `cs-grep`→`id-grep` across crates/paths/docs;
   preserve `LICENSE`; add `NOTICE`/Credits.

## Proposed journal catalog

Bundles under `crates/cs-grep-core/venues/`. ISSNs / OpenAlex source-ids
**resolved during implementation** via OpenAlex `/sources?search=` — do not
hand-enter.

- **`epi`** — Journal of Infectious Diseases, The Lancet Infectious Diseases,
  The Lancet, Emerging Infectious Diseases, Epidemiology and Infection,
  Clinical Infectious Diseases, American Journal of Epidemiology, International
  Journal of Epidemiology, Epidemiology, BMC Infectious Diseases.
- **`modelling`** — Epidemics, Infectious Disease Modelling, PLoS Computational
  Biology, PLoS ONE, PLoS Neglected Tropical Diseases, Journal of Theoretical
  Biology, Theoretical Population Biology, Bulletin of Mathematical Biology,
  Mathematical Biosciences, J. R. Soc. Interface.
- **`ecoevo`** — Ecology, Ecology Letters, Journal of Animal Ecology,
  Functional Ecology, Proc. R. Soc. B, Evolution, Molecular Biology and
  Evolution, Virus Evolution, PLoS Pathogens, Parasitology, Trends in Ecology &
  Evolution.
- **`preprints`** *(off by default)* — bioRxiv, medRxiv (arXiv `q-bio.PE`
  optional).

The full list above is confirmed by the maintainer — it covers the original
journal request plus the agreed additions.

## Verification (for the eventual implementation)

- `just check` (fmt + clippy `-D warnings` + test) green; the same recipe runs in
  CI. Offline suite deterministic via canned fixtures.
- Manual end-to-end: `just update epi` → `id-grep 'transmission WHERE
  venue:Epidemics'` returns real records with abstracts after `id-grep enrich`.
- Zotero: point at a copy of a real `zotero.sqlite`; `--exclude-owned` hides a
  known-owned item; library items are queryable.
- Agent contract: `id-grep --format json '…' --quiet` emits the documented
  schema with `schema_version`, pinned by a snapshot test; documented exit codes
  for no-results and bad-config.
- Live API smoke tests gated behind `#[ignore]` to keep CI offline.

## Open items to confirm during implementation

- Verify every journal's ISSN and OpenAlex source-id (don't hand-enter).
- DB migration vs. rebuild on the `dblp_key`→`key` rename (rebuild is simplest).
- Config/data-dir path change with the rename — document the one-time migration.
