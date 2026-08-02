# Using `id-grep` from an agent

`id-grep` is a local CLI for searching infectious-disease ecology, evolution, and
epidemiology literature. It keeps a local SQLite/FTS5 index of paper metadata
(fetched from OpenAlex and PubMed) that you query offline. This file is the
contract for driving it programmatically — e.g. after you have run your own
literature search and want to cross-reference, filter, or export.

## Golden path

```bash
id-grep init                              # one-time: create the DB + config
id-grep update --bundle epi --since 2020  # fetch metadata (network)
id-grep enrich                            # fill any missing abstracts (network)
id-grep --format json --quiet 'transmission WHERE venue:Epidemics'
```

`update`/`enrich` need network access; querying does not.

## Always use these flags when scripting

- `--format json` — machine-readable output (see schema below). Also switches
  error reporting to JSON on stderr.
- `--quiet` — suppress human progress/logging on stderr. Results still print on
  stdout. (Global: valid on any subcommand.)

## JSON output schema

`--format json` prints one object on stdout:

```json
{
  "schema_version": 1,
  "count": 2,
  "results": [
    {
      "key": "W123",            // source-scoped id (openalex W…, pmid:…, zotero:…)
      "source": "openalex",     // openalex | pubmed | zotero | dblp
      "venue": "Epidemics",     // catalog venue id
      "year": 2021,
      "title": "…",
      "authors": "Ada Lovelace, Alan Turing",   // ", "-joined, byline order
      "doi": "10.1016/…",       // may be null
      "url": "https://…",       // may be null
      "abstract": "…"           // may be null
    }
  ]
}
```

Check `schema_version` before parsing; it bumps on any incompatible change.
Object key order is **not** significant — parse by key, not by position (the
serializer currently emits keys alphabetically). On error under `--format json`,
stdout is empty and stderr carries `{"schema_version":1,"error":"…"}`.

## Exit codes (branch on these)

| code | meaning |
|------|---------|
| 0 | success, ≥1 result / command completed |
| 1 | generic error |
| 2 | CLI usage error (clap) |
| 3 | search ran but returned **no results** |
| 4 | config / query error (e.g. unknown venue or tag) |
| 5 | network / upstream source failure |

## Query language

`text [WHERE metadata-filters]`

- Text: bare terms (implicit AND), `OR`, `NOT`, parentheses, `"quoted phrases"`,
  `prefix*`, and field-scoped `title:`, `author:`, `abstract:`. `*` alone matches
  everything (use with a `WHERE` clause).
- Metadata filters (after `WHERE`): `venue:<id|alias>`, `tag:<tag>`,
  `year:2020`, `year:2018-2024`, `year:2020-`, `year:-2019`, `doi:<substr>`,
  combined with `AND`/`OR`/`NOT`/parens.

Examples:
```bash
id-grep --format json 'spillover OR reservoir WHERE tag:ecology AND year:2015-'
id-grep --format json '"basic reproduction number" WHERE venue:Epidemics'
id-grep --format json '* WHERE venue:plos-ntd AND year:2023'
```

Unknown venue/tag/rank → exit 4. List what exists by reading the catalog bundles
under `crates/id-grep-core/venues/` (`epi`, `modelling`, `ecoevo`, `preprints`).

## Zotero dedup

Cross-reference against a local Zotero library and drop what you already own:

```bash
id-grep --format json --exclude-owned --zotero ~/Zotero 'malaria WHERE venue:PLoS-NTD'
```

`--zotero` defaults to `~/Zotero` if omitted. Matching is DOI-first, then
normalized title. Zotero can be running (the DB is copied and opened read-only).

## Credentials (optional, via .env or environment)

- `OPENALEX_MAILTO` — email for OpenAlex's polite pool (recommended).
- `NCBI_API_KEY`, `NCBI_EMAIL` — raise PubMed rate limits.
- `OPENALEX_API_KEY`, `SEMANTIC_SCHOLAR_S2_KEY` — optional API keys.

See `.env.example`. None are required to query an already-built index.

## Build / test (for working on id-grep itself)

`just check` runs fmt-check + clippy (`-D warnings`) + the offline test suite.
Network-dependent tests are `#[ignore]`d; run them with `cargo test -- --ignored`
where OpenAlex/NCBI are reachable.
