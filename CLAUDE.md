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

`enrich` paces its Semantic Scholar requests and backs off (skipping S2 in
favor of its existing OpenAlex/Crossref fallback) under sustained rate
limiting rather than failing the run — see `Credentials` below for how
`SEMANTIC_SCHOLAR_S2_KEY` affects that pacing.

## Always use these flags when scripting

- `--format json` — machine-readable output (see schema below). Also switches
  error reporting to JSON on stderr.
- `--quiet` — suppress human progress/logging on stderr. Results still print on
  stdout. (Global: valid on any subcommand.)

## JSON output schema

`--format json` prints one object on stdout:

```json
{
  "schema_version": 2,
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
      "abstract": "…",          // may be null
      "owned": true              // true|false if a Zotero library was consulted (--mark-owned / --exclude-owned), else null
    }
  ]
}
```

Check `schema_version` before parsing; it bumps on any incompatible change.
Object key order is **not** significant — parse by key, not by position (the
serializer currently emits keys alphabetically). On error under `--format json`,
stdout is empty and stderr carries `{"schema_version":2,"error":"…"}`.

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
  `added-since:<YYYY-MM-DD>`, combined with `AND`/`OR`/`NOT`/parens.

`added-since:<YYYY-MM-DD>` matches papers whose row was inserted or last
changed (`updated_at`) on or after that date — use it to see what's new in
the local index since you last checked, without re-reading a full result set.

Examples:
```bash
id-grep --format json 'spillover OR reservoir WHERE tag:ecology AND year:2015-'
id-grep --format json '"basic reproduction number" WHERE venue:Epidemics'
id-grep --format json '* WHERE venue:plos-ntd AND year:2023'
id-grep --format json '* WHERE tag:epi AND added-since:2026-07-01'
```

Unknown venue/tag/rank → exit 4. List what exists by reading the catalog bundles
under `crates/id-grep-core/venues/` (`epi`, `modelling`, `ecoevo`, `general`,
`preprints`). `general` (Nature, Science) loads by default; `preprints` is
opt-in — add it to `bundles:` in the user config or pass `--bundle preprints`
on `update` to fetch it.

Multidisciplinary venues (Nature, Science, Nature Medicine, PLOS ONE, The
American Naturalist) carry `scope: infectious-disease` in the catalog, so
`update` fetches only works whose primary OpenAlex topic sits in an
infectious-disease subfield (Infectious Diseases, Virology, Parasitology,
Epidemiology) rather than everything the journal publishes.

## Saved searches

Persist a named query and, on each run, see only what's new since the last
run — a scriptable alternative to manually tracking `added-since:` dates.

```bash
id-grep search save weekly-epi 'transmission WHERE venue:Epidemics'
id-grep --format json --quiet search run weekly-epi   # first run: full results
id-grep --format json --quiet search run weekly-epi   # later runs: only rows added/changed since the last run
id-grep --format json --quiet search run weekly-epi --peek  # preview without advancing the last-run marker
id-grep --format json --quiet search list
id-grep search rm weekly-epi
```

- `save <name> <query>` validates the query and stores it; resaving an
  existing name replaces the query and resets its last-run marker.
- `run <name>` executes the saved query. The first run (no prior last-run
  marker) returns the full result set; every run after that is implicitly
  ANDed with `added-since:<last run>`, so only new/changed rows print. Exit
  code follows the normal search contract (3 = ran fine, nothing new). A
  real (non-`--peek`) run always advances the last-run marker, including
  under `--format json` — that flag is for machine-readable output, not a
  dry-run signal; use `--peek` when you want a no-op preview. `run` accepts
  the usual `--format`/`--sort`/`--limit`/`--fields`.
- `list` prints saved searches (name, query, last run); `--format json`
  emits `{"schema_version": 2, "count": N, "saved_searches": [{"name",
  "query", "last_run_at"}, ...]}` — note this is a different envelope shape
  than search results (`results`/`count` of papers), since it's listing
  saved queries, not papers.
- `rm <name>` removes a saved search.
- `run`/`rm` on an unknown name exit 4 (config error), consistent with the
  exit-code table above.
- Caveat: an ad-hoc query whose *first shell argument* is exactly `search`
  (e.g. `id-grep search algorithms`, unquoted) is now parsed as this
  subcommand group instead of a text search. Pass the query as a single
  shell argument (`id-grep 'search algorithms'`) or reorder terms so
  `search` isn't first (`id-grep 'algorithms search'`) to avoid this.

## Zotero cross-reference

Cross-reference against a local Zotero library. Two modes, both opt-in (a
library is never consulted unless one of these flags is passed):

- `--mark-owned` — keep every result, and annotate each with whether it's
  already in the library. Adds an `owned` (`true`/`false`) field to JSON
  records, and an `owned` column (`*`/blank) as the first table/CSV column.
  This is the default choice when you still want to see (and cite) papers
  you already have — the point is visibility, not filtering.
- `--exclude-owned` — drop owned results entirely, built on the same
  owned-check as `--mark-owned` (so combining both is redundant: the
  survivors are all unowned, and `owned` reads `false` throughout).

```bash
id-grep --format json --mark-owned --zotero ~/Zotero 'malaria WHERE venue:PLoS-NTD'
id-grep --format json --exclude-owned --zotero ~/Zotero 'malaria WHERE venue:PLoS-NTD'
```

`--zotero` defaults to `~/Zotero` if omitted. Matching is DOI-first, then
normalized title. Zotero can be running (the DB is copied and opened read-only).
Without `--mark-owned`/`--exclude-owned`, `owned` is `null` in JSON and no
`owned` column appears in table/CSV output — plain queries never touch Zotero.

## Credentials (optional, via .env or environment)

- `OPENALEX_MAILTO` — email for OpenAlex's polite pool (recommended).
- `NCBI_API_KEY`, `NCBI_EMAIL` — raise PubMed rate limits.
- `OPENALEX_API_KEY`, `SEMANTIC_SCHOLAR_S2_KEY` — optional API keys.
  `SEMANTIC_SCHOLAR_S2_KEY` also raises the pacing rate `enrich` uses for
  Semantic Scholar requests; without it, requests are paced more
  conservatively to avoid tripping S2's unauthenticated rate limits.

See `.env.example`. None are required to query an already-built index.

## Build / test (for working on id-grep itself)

`just check` runs fmt-check + clippy (`-D warnings`) + the offline test suite.
Network-dependent tests are `#[ignore]`d; run them with `cargo test -- --ignored`
where OpenAlex/NCBI are reachable.
