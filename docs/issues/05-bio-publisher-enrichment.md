# Bio-publisher abstract enrichment routing

**Labels:** `pivot`, `enrichment`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1 (source abstraction)
**Phase:** 2

## Problem

The abstract-enrichment fallback in `crates/cs-grep-core/src/abstracts.rs`
routes by DOI prefix / host to CS publishers (ACM, IEEE, ACL, NeurIPS,
OpenReview, …). ID/ecology/epi papers come from different publishers, so the
routing table and scrapers need bio-publisher entries. OpenReview (CS-conference
only) becomes dead code and should be retired.

## Deliverable

- New DOI-prefix / host routing + CSS selectors (or reuse of the generic
  `citation_abstract` meta fallback) for the major bio publishers.
- OpenReview enrichment path removed (or gated off).

## Approach

1. **Tests first:** add routing unit tests (mirroring the existing
   `abstracts.rs` source-routing tests ~L1750-1810) asserting each new DOI
   prefix maps to the right `AbstractSource`, and selector/extraction tests
   against canned HTML fixtures.
2. Add DOI-prefix routes:
   - `10.1371/` → PLOS
   - `10.1016/` → Elsevier / ScienceDirect
   - `10.1093/` → OUP
   - `10.1098/` → Royal Society
   - `10.1101/` → bioRxiv / medRxiv (Cold Spring Harbor)
   - `10.3201/` → EID (CDC)
   - `10.1073/` → PNAS
   - `10.1002/`, `10.1111/` → Wiley
   Many of these expose `citation_abstract` / `og:description` meta tags already
   handled by the generic `meta_contents` fallback — prefer that where it works,
   add bespoke selectors only where needed.
3. Keep the existing SSRF guards, size caps, and identity checks
   (`paper_identity_matches`, `MIN/MAX_ABSTRACT_CHARS`).
4. Remove the OpenReview client + `AbstractSource::Openreview` variant.

## Key files

- `crates/cs-grep-core/src/abstracts.rs` (`AbstractSource` enum, host/DOI
  routing ~L1151-1192, selectors ~L317-352)

## Acceptance criteria

- Routing unit tests cover all new prefixes.
- Selector/extraction tests pass against canned publisher HTML fixtures.
- An `enrich` pass fills abstracts on a sample of real ID DOIs (manual /
  `#[ignore]`d).
- OpenReview code removed; `just test` green.
