//! End-to-end OpenAlex pipeline, offline: canned `/works` JSON page ->
//! `parse_works_page` -> upsert -> `query::parse` + `search` ->
//! `render(Format::Json)` -> parse back and assert against the documented
//! JSON envelope (see CLAUDE.md).
//!
//! This complements the per-module OpenAlex unit tests in
//! `src/sources/openalex.rs` (which exercise `map_work` against individual
//! edge cases) by driving the *whole* pipeline the way the CLI does.

use id_grep_core::config::Config;
use id_grep_core::db::{Database, Search};
use id_grep_core::output::{render, Format, SCHEMA_VERSION};
use id_grep_core::query;
use id_grep_core::sources::openalex::parse_works_page;
use serde_json::json;

/// A single well-formed OpenAlex work, shaped like a real `/works` page.
fn good_work() -> serde_json::Value {
    json!({
        "id": "https://openalex.org/W2001",
        "doi": "https://doi.org/10.1016/j.epidem.2021.100123",
        "title": "Estimating the basic reproduction number of a zoonotic spillover event",
        "publication_year": 2021,
        "authorships": [
            {"author": {"display_name": "Ada Lovelace"}},
            {"author": {"display_name": "Alan Turing"}}
        ],
        "abstract_inverted_index": {
            "We": [0],
            "estimate": [1],
            "R0.": [2]
        },
        "primary_location": {"landing_page_url": "https://doi.org/10.1016/j.epidem.2021.100123"}
    })
}

fn works_page() -> serde_json::Value {
    json!({
        "results": [good_work()],
        "meta": {"next_cursor": null}
    })
}

#[test]
fn full_pipeline_from_canned_openalex_page() {
    let (papers, cursor) = parse_works_page(&works_page(), "Epidemics");
    assert_eq!(papers.len(), 1);
    assert!(cursor.is_none());

    let mut db = Database::open_in_memory().unwrap();
    let n = db.upsert_papers(&papers).unwrap();
    assert_eq!(n, 1);
    assert_eq!(db.count().unwrap(), 1);

    let config = Config::defaults().unwrap();
    let parsed = query::parse("reproduction WHERE venue:Epidemics AND year:2021", &config).unwrap();
    let hits = db
        .search(&Search {
            fts: parsed.fts,
            filter: parsed.filter,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);

    let rendered = render(&hits, Format::Json, None).unwrap();
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    assert_eq!(value["count"], 1);

    let record = &value["results"][0];
    assert_eq!(record["key"], "W2001");
    assert_eq!(record["source"], "openalex");
    assert_eq!(record["venue"], "Epidemics");
    assert_eq!(record["year"], 2021);
    assert_eq!(
        record["title"],
        "Estimating the basic reproduction number of a zoonotic spillover event"
    );
    assert_eq!(record["authors"], "Ada Lovelace, Alan Turing");
    assert_eq!(record["doi"], "10.1016/j.epidem.2021.100123");
    assert_eq!(
        record["url"],
        "https://doi.org/10.1016/j.epidem.2021.100123"
    );
    assert_eq!(record["abstract"], "We estimate R0.");
}

/// Regression guard against OpenAlex API schema drift.
///
/// Guards two independent failure modes if the upstream `/works` shape ever
/// changes underneath us:
///
///  1. A work missing an expected required field (simulated here by renaming
///     `publication_year` -> `year`) must be dropped outright, not crash or
///     produce a bogus record with `year: 0`.
///  2. A work whose nested author shape drifts (simulated here by renaming
///     `authorships[].author.display_name` -> `author.name`) must still map
///     since title/year are present -- but must NOT silently substitute the
///     wrong value; `authors` must come back empty (visibly missing), never
///     a mismapped string that could be mistaken for correct data.
#[test]
fn parse_works_page_skips_and_never_silently_mismaps_drifted_fields() {
    let renamed_year_field = json!({
        "id": "https://openalex.org/W3001",
        "title": "A work with a renamed year field",
        // drift: upstream renamed `publication_year` -> `year`
        "year": 2021,
        "authorships": [],
    });

    let renamed_author_field = json!({
        "id": "https://openalex.org/W3002",
        "title": "A work with a renamed author field",
        "publication_year": 2019,
        // drift: upstream renamed `author.display_name` -> `author.name`
        "authorships": [
            {"author": {"name": "Grace Hopper"}}
        ],
    });

    let page = json!({
        "results": [good_work(), renamed_year_field, renamed_author_field],
        "meta": {"next_cursor": null}
    });

    let (papers, _cursor) = parse_works_page(&page, "Epidemics");

    // The work with a renamed *required* field (year) is dropped outright.
    assert!(!papers.iter().any(|p| p.key == "W3001"));

    // The work with a renamed *nested* field (author name) is kept -- it has
    // a title and year -- but its authors string must be empty, not a wrong
    // value silently threaded through.
    let drifted = papers
        .iter()
        .find(|p| p.key == "W3002")
        .expect("work with title/year present should still map");
    assert_eq!(drifted.authors, "");

    // The well-formed work is unaffected.
    assert!(papers.iter().any(|p| p.key == "W2001"));
    assert_eq!(papers.len(), 2);
}
