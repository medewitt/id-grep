# PubMed (NCBI E-utilities) ingestion source (complementary)

**Labels:** `pivot`, `ingestion`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (source abstraction)
**Phase:** 2

## Problem

PubMed complements OpenAlex with strong biomedical/epidemiology coverage, MeSH
indexing, and reliable abstracts for clinical-ID journals. There is currently no
PubMed / NCBI integration anywhere in the repo.

## Deliverable

- `crates/cs-grep-core/src/sources/pubmed.rs` implementing `Source` via NCBI
  E-utilities: `esearch` to find PMIDs for a journal + year, `efetch` to pull
  records, mapped to `Paper` including DOI and MeSH terms.

## Approach

1. **Tests first:** unit test parsing a canned `efetch` XML record (fixture) →
   `Paper`, asserting title, authors, year, abstract, DOI extraction (ArticleId
   `IdType="doi"`), and MeSH terms folded into `tags`/abstract.
2. Query shape:
   ```
   esearch.fcgi?db=pubmed&term="<journal>"[ta]+AND+<year>[dp]&retmax=...&retstart=...
   efetch.fcgi?db=pubmed&id=<pmids>&retmode=xml
   ```
   Use the venue's `pubmed_journal` (NLM title abbreviation); paginate via
   `retstart`/`retmax`.
3. Respect NCBI rate limits (3 req/s without a key, 10 with a key); support
   `NCBI_API_KEY` + `NCBI_EMAIL` query params, added to `Secrets`.
4. Wire into `cmd_update` via the `Source` trait; a venue may declare OpenAlex
   and/or PubMed identifiers and be fetched from whichever are configured.

## Key files

- `crates/cs-grep-core/src/sources/pubmed.rs` (new)
- `crates/cs-grep-core/src/config.rs` (`Secrets` — add `ncbi_api_key`,
  `ncbi_email`)
- `crates/cs-grep/src/main.rs` (`cmd_update`)
- An XML parser dependency (e.g. `quick-xml`) added to `Cargo.toml` if not
  present.

## Acceptance criteria

- Offline unit tests parse canned `efetch` XML → correct `Paper`s incl. DOI and
  MeSH.
- Rate-limiting respected; `NCBI_API_KEY`/`NCBI_EMAIL` honored when set.
- A live smoke test (`#[ignore]`d) ingests a small real journal + year window.
- `just test` green.
