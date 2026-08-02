# Issue backlog: pivot `cs-grep` → `id-grep`

The feasibility rationale behind this backlog is in
[`../pivot-feasibility.md`](../pivot-feasibility.md).

GitHub Issues are currently disabled on this repository, so this backlog is
tracked as Markdown. Each file below is written to be copy-pasted into a GitHub
issue verbatim once Issues are enabled (**Settings → General → Features →
Issues**). The epic is the parent; the numbered files are its sub-issues.

| # | File | Title | Phase |
|---|------|-------|-------|
| — | [`00-epic-pivot-id-grep.md`](00-epic-pivot-id-grep.md) | **[Epic]** Pivot cs-grep → id-grep | — |
| 1 | [`01-source-abstraction.md`](01-source-abstraction.md) | Source abstraction + model generalization | 1 |
| 8 | [`08-justfile-ci.md`](08-justfile-ci.md) | Build tooling (`justfile`) + CI | 1 |
| 2 | [`02-openalex-ingestion.md`](02-openalex-ingestion.md) | OpenAlex ingestion source | 2 |
| 3 | [`03-pubmed-ingestion.md`](03-pubmed-ingestion.md) | PubMed (NCBI E-utilities) ingestion source | 2 |
| 4 | [`04-id-journal-catalog.md`](04-id-journal-catalog.md) | ID journal catalog + preprint venues | 2 |
| 5 | [`05-bio-publisher-enrichment.md`](05-bio-publisher-enrichment.md) | Bio-publisher abstract enrichment routing | 2 |
| 6 | [`06-zotero-dedup-search.md`](06-zotero-dedup-search.md) | Zotero: dedup + search-your-library | 2 |
| 9 | [`09-test-suite-hardening.md`](09-test-suite-hardening.md) | Test suite hardening & ID fixtures | 2 |
| 7 | [`07-rename-attribution.md`](07-rename-attribution.md) | Rename to `id-grep` + attribution/credits | 3 |
| 10 | [`10-llm-ergonomics-claude-md.md`](10-llm-ergonomics-claude-md.md) | LLM/agent ergonomics + `CLAUDE.md` | 3 |
| 11 | [`11-docs-setup.md`](11-docs-setup.md) | Docs & setup | 3 |

## Phasing

- **Phase 1 (foundation):** #1 + #8 — land the source abstraction and the
  build/CI/TDD harness first so every other issue is developed test-first
  through `just test`.
- **Phase 2 (sources & data, parallelizable after #1):** #2, #3, #4, #5, #6,
  each feeding #9 as it lands.
- **Phase 3 (finish):** #7, #10, #11 — rename, agent contract, docs.

## Suggested labels

`epic`, `pivot`, `ingestion`, `catalog`, `zotero`, `enrichment`, `rename`,
`tooling`, `tests`, `docs`.
