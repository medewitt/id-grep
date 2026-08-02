# ID journal catalog + preprint venues

**Labels:** `pivot`, `catalog`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (source abstraction), #2 (OpenAlex), #3 (PubMed for NLM ids)
**Phase:** 2

## Problem

The venue catalog (`crates/cs-grep-core/venues/*.yaml`) currently lists CS
conferences/journals keyed by DBLP stream. It must be replaced with
infectious-disease ecology / evolution / epidemiology journals keyed by ISSN /
OpenAlex source-id (and NLM abbreviation for PubMed). The catalog is both the
ingestion list and the search vocabulary, so this defines what `id-grep` can find.

## Deliverable

- New YAML bundles under `crates/cs-grep-core/venues/`: `epi.yaml`,
  `modelling.yaml`, `ecoevo.yaml`, `preprints.yaml`.
- `DEFAULT_BUNDLES` / `BUNDLED_VENUES` in `config.rs` updated to register them;
  CS bundles (`security.yaml`, `ml.yaml`, `se.yaml`) deleted.
- `preprints` off by default (opt-in via `--bundle`).

## Proposed catalog

ISSNs / OpenAlex source-ids **must be resolved during implementation** via
OpenAlex `/sources?search=<name>` — do not hand-enter. NLM title abbreviations
resolved from the NLM Catalog for PubMed.

- **`epi`** — Journal of Infectious Diseases, The Lancet Infectious Diseases,
  The Lancet, Emerging Infectious Diseases, Epidemiology and Infection,
  Clinical Infectious Diseases, American Journal of Epidemiology,
  International Journal of Epidemiology, Epidemiology, BMC Infectious Diseases.
- **`modelling`** — Epidemics, Infectious Disease Modelling, PLoS Computational
  Biology, PLoS ONE, PLoS Neglected Tropical Diseases, Journal of Theoretical
  Biology, Theoretical Population Biology, Bulletin of Mathematical Biology,
  Mathematical Biosciences, J. R. Soc. Interface.
- **`ecoevo`** — Ecology, Ecology Letters, Journal of Animal Ecology,
  Functional Ecology, Proc. R. Soc. B, Evolution, Molecular Biology and
  Evolution, Virus Evolution, PLoS Pathogens, Parasitology, Trends in Ecology &
  Evolution.
- **`preprints`** — bioRxiv, medRxiv (arXiv `q-bio.PE` optional).

The full list above is confirmed by the maintainer (original request plus the
agreed additions).

Each venue entry carries `id`, `name`, `aliases`, `tags`, and the
source-agnostic identifiers from #1 (`issn`, `openalex_source_id`,
`pubmed_journal`). Tags group sub-domains (e.g. `epi`, `modelling`, `ecology`,
`evolution`, `preprint`) for `tag:` queries.

## Approach

1. **Tests first:** rewrite catalog-coupled tests in `config.rs` /
   `main.rs` for the new bundle names, tags, and aliases (the existing tests
   assert the CS catalog — see `bundled_tags_match_venue_families`,
   `generated_default_config_parses`, alias lookups, bundle-selection tests).
   Add a test asserting every bundled venue has at least one usable source id.
2. Resolve each journal's ISSN + OpenAlex source-id via `/sources?search=`;
   record them in the YAML with a short comment noting the resolved name.
3. Delete CS bundles; update `DEFAULT_BUNDLES`/`BUNDLED_VENUES`.

## Key files

- `crates/cs-grep-core/venues/*.yaml`
- `crates/cs-grep-core/src/config.rs` (`DEFAULT_BUNDLES`, `BUNDLED_VENUES`, tests)
- `crates/cs-grep/src/main.rs` (bundle-selection tests)

## Acceptance criteria

- All four bundles parse; `config` tests pass with ID tags/aliases.
- Every bundled venue resolves to ≥1 ingestion identifier.
- `id-grep update --bundle epi --since 2020` ingests real records end-to-end.
- `just test` green.
