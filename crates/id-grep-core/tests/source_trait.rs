//! The `Source` abstraction can drive ingestion end-to-end with any backend,
//! independent of DBLP. Proves the pivot's ingestion is source-agnostic.

use id_grep_core::config::Venue;
use id_grep_core::db::{Database, Search};
use id_grep_core::sources::Source;
use id_grep_core::{Paper, Result};

/// A stand-in ingestion backend that returns canned records (no network).
struct FakeSource;

impl Source for FakeSource {
    fn name(&self) -> &str {
        "fake"
    }

    async fn fetch_venue(&self, venue: &Venue, min_year: i32, max_year: i32) -> Result<Vec<Paper>> {
        Ok(vec![Paper {
            key: format!("fake:{}:{min_year}", venue.id),
            source: self.name().to_string(),
            venue: venue.id.clone(),
            year: max_year.min(2021),
            title: "Spillover dynamics of a zoonotic pathogen".into(),
            authors: "Ada Lovelace, Alan Turing".into(),
            doi: Some("10.1000/fake".into()),
            url: None,
            abstract_text: Some("A model of cross-species transmission.".into()),
        }])
    }
}

fn venue(id: &str) -> Venue {
    Venue {
        id: id.into(),
        name: id.into(),
        dblp_stream: None,
        issn: vec!["1234-5678".into()],
        openalex_source_id: None,
        pubmed_journal: None,
        aliases: Vec::new(),
        rank: None,
        tags: vec!["epi".into()],
    }
}

#[tokio::test]
async fn arbitrary_source_ingests_end_to_end() {
    let source = FakeSource;
    let v = venue("Epidemics");

    // fetch via the trait
    let papers = source.fetch_venue(&v, 2020, 2025).await.unwrap();
    assert_eq!(papers.len(), 1);
    assert_eq!(papers[0].source, "fake");

    // upsert into the index
    let mut db = Database::open_in_memory().unwrap();
    let n = db.upsert_papers(&papers).unwrap();
    assert_eq!(n, 1);
    assert_eq!(db.count().unwrap(), 1);

    // query the ingested record back out
    let hits = db
        .search(&Search {
            fts: Some("spillover".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].venue, "Epidemics");
    assert_eq!(hits[0].source, "fake");
    assert_eq!(hits[0].doi.as_deref(), Some("10.1000/fake"));
}
