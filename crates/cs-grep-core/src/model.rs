use serde::{Deserialize, Serialize};

/// A paper record as stored and queried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    /// Stable, source-scoped identifier used as the storage key.
    pub key: String,
    /// Ingestion source that produced this record (e.g. `dblp`, `openalex`).
    pub source: String,
    pub venue: String,
    pub year: i32,
    pub title: String,
    /// Authors joined with ", " in signature order.
    pub authors: String,
    pub doi: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "abstract")]
    pub abstract_text: Option<String>,
}

impl Paper {
    /// Generate a BibTeX citation key, e.g. `NDSS:2021:smith`.
    pub fn cite_key(&self) -> String {
        let first_author = self
            .authors
            .split(',')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .last()
            .unwrap_or("anon")
            .to_lowercase();
        let venue = self.venue.replace([' ', '&'], "").to_lowercase();
        format!("{venue}:{}:{first_author}", self.year)
    }
}
