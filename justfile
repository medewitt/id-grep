# Build & test control surface for the workspace.
# Install `just` with: cargo install just  (https://github.com/casey/just)
#
# CI runs `just ci`, which is identical to `just check`, so local and CI never
# drift. Network-dependent tests are marked `#[ignore]`, so `just test` is
# offline and deterministic.

set shell := ["bash", "-uc"]

# Show the available recipes.
default:
    @just --list

# Build the whole workspace.
build:
    cargo build --workspace

# Run the offline test suite (network tests are #[ignore]d).
test:
    cargo test --workspace

# Format all code in place.
fmt:
    cargo fmt --all

# Check formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Lint with clippy; warnings are errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Format check + lint + tests. Run this before pushing.
check: fmt-check lint test

# Exactly what CI runs.
ci: check

# Run the CLI with a query, e.g. `just run 'malaria WHERE venue:Epidemics'`.
run query='':
    cargo run -- {{query}}

# Fetch/update metadata for a bundle, e.g. `just update epi`.
update bundle='':
    cargo run -- update --bundle {{bundle}}

# Fill missing abstracts on the existing database.
enrich:
    cargo run -- enrich
