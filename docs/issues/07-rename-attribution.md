# Rename to `id-grep` + attribution/credits

**Labels:** `pivot`, `rename`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** #1–#6 (do the rename last to reduce churn)
**Phase:** 3

## Problem

With the pivot complete, the CS-era branding is misleading. Rename the tool to
`id-grep` across code, paths, and docs, and ensure upstream and data-source
attribution is properly preserved and expanded.

## Deliverable

- Full rename `cs-grep` → `id-grep`:
  - crate names (workspace + both crates in `Cargo.toml`), binary name.
  - `directories::ProjectDirs::from("", "", "id-grep")` in `config.rs:318`
    (config/data dir).
  - user-agent string (`cs-grep/{version}` → `id-grep/{version}`).
  - clap `name` + `about` in `main.rs` (`about = "Search infectious-disease
    ecology, evolution & epidemiology literature"`).
  - README, `flake.nix` package name.
- **Attribution preserved & expanded:**
  - Keep `LICENSE` intact.
  - Add a `NOTICE` file and/or README "Credits" section citing upstream
    **`philippnormann/cs-grep`** (and its `sec-grep` origin), plus data sources:
    **OpenAlex**, **NCBI/PubMed (E-utilities)**, **Semantic Scholar**,
    **Crossref**, and DBLP (historical).

## Approach

1. **Tests first:** update tests asserting the binary/config-dir/user-agent names
   and the `about` string; snapshot the `--help` header if practical.
2. Mechanical rename across crates, paths, strings, README, `flake.nix`.
3. Document the one-time **config/data-dir migration** for existing users
   (old `cs-grep` dir → new `id-grep` dir), or auto-detect + notify.
4. Add `NOTICE`/Credits and verify license terms of each source are respected
   (attribution + polite-pool usage).

## Key files

- `Cargo.toml` (workspace + `crates/*/Cargo.toml`)
- `crates/cs-grep-core/src/config.rs` (`ProjectDirs`)
- `crates/cs-grep/src/main.rs` (clap `name`/`about`, user-agent)
- `flake.nix`, `README.md`, new `NOTICE`

## Acceptance criteria

- `cargo install --path crates/id-grep` (or equivalent) yields an `id-grep`
  binary; `--help` shows the ID domain.
- Config/data resolve under the `id-grep` dir; migration documented.
- `LICENSE` unchanged; `NOTICE`/Credits cite upstream + all data sources.
- `just check` green.
