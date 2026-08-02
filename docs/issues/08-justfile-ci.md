# Build tooling (`justfile`) + CI

**Labels:** `tooling`, `tests`
**Parent:** [Epic] Pivot cs-grep → id-grep
**Depends on:** none (do first, alongside #1)
**Phase:** 1

## Problem

There is no build/test control surface and **no CI** (`.github/` does not
exist). To develop the pivot test-first and prevent drift, the repo needs a
single command surface used identically by humans, CI, and the calling agent.

## Deliverable

- A root `justfile` with recipes:
  - `just build` — `cargo build --workspace`
  - `just test` — `cargo test --workspace` (offline; network tests `#[ignore]`d)
  - `just fmt` — `cargo fmt --all`
  - `just lint` — `cargo clippy --workspace --all-targets -- -D warnings`
  - `just check` — `fmt --check` + `lint` + `test`
  - `just run '<query>'` — run the CLI
  - `just update <bundle>` — `cargo run -- update --bundle <bundle>`
  - `just enrich` — `cargo run -- enrich`
  - `just ci` — exactly what CI runs
- **GitHub Actions CI** (`.github/workflows/ci.yml`) that installs `just` +
  toolchain and runs `just ci` on push/PR.

## Approach

1. Author the `justfile`; keep recipes thin wrappers over `cargo` so behavior is
   obvious and reproducible.
2. Add the workflow: stable Rust toolchain, `rustfmt` + `clippy` components,
   cache, `just ci`. No network in CI (ignored tests stay ignored).
3. Document `just` installation in the README (superseded/expanded by #11).

## Key files

- `justfile` (new, repo root)
- `.github/workflows/ci.yml` (new)

## Acceptance criteria

- `just check` runs fmt-check, clippy (`-D warnings`), and tests locally.
- CI runs `just ci` on every PR and is green on the current tree.
- Local and CI use the same recipes (no drift).
