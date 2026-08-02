# [Epic] Pivot cs-grep → id-grep: infectious-disease ecology, evolution & epidemiology literature search

**Labels:** `epic`, `pivot`

## Summary

Repurpose this tool from a **computer-science** literature search CLI into
**`id-grep`**, a local literature search tool for **infectious-disease (ID)
ecology, evolution, and epidemiology**. Today the tool ingests CS metadata from
**DBLP's SPARQL endpoint** into a local **SQLite + FTS5** index, exposes a query
language (`text WHERE filters`), and enriches abstracts from OpenAlex /
Semantic Scholar / Crossref / OpenReview.

The hard constraint: **DBLP only indexes CS**, so the target journals (Ecology,
Ecology Letters, Lancet ID, Epidemics, PLoS Comp Biol, EID, …) are unreachable
through it. The pivot replaces the **ingestion backend**, not just a journal
list. Two existing assets make this tractable:

1. Working **OpenAlex / Semantic Scholar / Crossref** clients already live in
   `crates/cs-grep-core/src/abstracts.rs` (used today only for abstract
   enrichment, incl. OpenAlex `abstract_inverted_index` reconstruction and
   identity matching).
2. The venue catalog already doubles as **both** the ingestion list **and** the
   search vocabulary, so swapping the catalog swaps both.

## Decisions (locked with maintainer)

- **Full pivot + rename** to **`id-grep`**; CS bundles removed. Preserve
  upstream **attribution/citations**.
- **Ingestion:** OpenAlex primary + **PubMed** (NCBI E-utilities) complementary.
- **Zotero:** **search your own library** + **dedup / flag already-owned** (no
  push-to-Zotero). Access = read a **copy of `zotero.sqlite`**, optional Better
  BibTeX support.
- **Search axis:** journal-scoped. **Preprints (bioRxiv, medRxiv) included as
  venues**, optional to query.

## Cross-cutting requirements (apply to every sub-issue)

- **Build/test via `just`** — a root `justfile` is the single control surface
  (`build/test/fmt/lint/check/run/update/enrich/ci`); CI calls the same recipes
  so local and CI never drift.
- **Test-driven development** — tests land first; new modules ship colocated
  unit tests + integration tests; all network tests are `#[ignore]`d and fed
  **canned fixtures** so the default suite is offline and deterministic. The
  suite pins ingestion → map → dedup → output so refactors can't silently drift.
- **LLM / agent-first ergonomics** — `id-grep` must be cleanly drivable by
  Claude Code as a tool the maintainer calls after their own literature
  searches: JSON is a first-class, versioned, documented contract
  (`--format json` + `schema_version`); non-interactive by default (`--quiet`,
  no prompts, never require the TUI); machine-readable failures (distinct exit
  codes + JSON errors on stderr); a root `CLAUDE.md` documenting canonical
  invocations and examples.

## Proposed journal catalog (bundles)

ISSNs / OpenAlex source-ids **resolved during implementation** via OpenAlex
`/sources?search=` — do not hand-enter.

- **`epi`** — Journal of Infectious Diseases, The Lancet Infectious Diseases,
  The Lancet, Emerging Infectious Diseases, Epidemiology and Infection,
  Clinical Infectious Diseases *(rec)*, American Journal of Epidemiology *(rec)*,
  International Journal of Epidemiology *(rec)*, Epidemiology *(rec)*,
  BMC Infectious Diseases *(rec)*.
- **`modelling`** — Epidemics, Infectious Disease Modelling, PLoS Computational
  Biology, PLoS ONE, PLoS Neglected Tropical Diseases, Journal of Theoretical
  Biology, Theoretical Population Biology, Bulletin of Mathematical Biology
  *(rec)*, Mathematical Biosciences *(rec)*, J. R. Soc. Interface *(rec)*.
- **`ecoevo`** — Ecology, Ecology Letters, Journal of Animal Ecology *(rec)*,
  Functional Ecology *(rec)*, Proc. R. Soc. B *(rec)*, Evolution *(rec)*,
  Molecular Biology and Evolution *(rec)*, Virus Evolution *(rec)*,
  PLoS Pathogens *(rec)*, Parasitology *(rec)*, Trends in Ecology & Evolution
  *(rec)*.
- **`preprints`** *(off by default)* — bioRxiv, medRxiv (arXiv `q-bio.PE`
  optional).

The maintainer's original journal list is fully covered; `(rec)` items are
recommendations to confirm/trim.

## Sub-issues & phasing

- **Phase 1 (foundation):** #1 Source abstraction + model generalization ·
  #8 Build tooling (`justfile`) + CI
- **Phase 2 (sources & data, parallelizable after #1):** #2 OpenAlex ingestion ·
  #3 PubMed ingestion · #4 ID journal catalog + preprints · #5 Bio-publisher
  enrichment routing · #6 Zotero dedup + search-library · (feeds #9 Test suite
  hardening)
- **Phase 3 (finish):** #7 Rename to `id-grep` + attribution · #10 LLM/agent
  ergonomics + `CLAUDE.md` · #11 Docs & setup

## Attribution

Preserve `LICENSE`; add a `NOTICE`/Credits section citing upstream
**`philippnormann/cs-grep`**, DBLP (historical), **OpenAlex**, **NCBI/PubMed**,
**Semantic Scholar**, and **Crossref**.

## Definition of done

- All sub-issues closed.
- `just check` green locally and in CI; offline test suite deterministic.
- `id-grep init → update → query → enrich → dedup` works end-to-end against the
  new catalog.
- `--format json` emits a documented, versioned schema; `CLAUDE.md` examples run
  as written.
- README, `--help`, and `NOTICE`/Credits reflect the ID domain and cite all
  sources.
