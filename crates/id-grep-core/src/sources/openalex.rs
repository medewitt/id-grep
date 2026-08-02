//! OpenAlex ingestion: fetch works for a venue by ISSN / source id.
//!
//! OpenAlex covers ecology, biomedical, and preprint literature and returns
//! abstracts inline (as an `abstract_inverted_index`), so ingestion populates
//! abstracts directly, reducing later `enrich` work.

use std::time::Duration;

use serde_json::Value;

use crate::abstracts::{abstract_from_openalex, normalized_doi};
use crate::config::{Secrets, Venue};
use crate::sources::Source;
use crate::{Error, Paper, Result};

/// Value stored in [`Paper::source`] for records from this backend.
pub const SOURCE_NAME: &str = "openalex";

const DEFAULT_BASE_URL: &str = "https://api.openalex.org";
const PER_PAGE: usize = 200;
/// Fields requested from the API; trims the payload to what `map_work` uses.
const SELECT_FIELDS: &str =
    "id,doi,title,display_name,publication_year,authorships,abstract_inverted_index,primary_location";
/// Safety bound on cursor pages (200/page => up to 200k works per venue).
const MAX_PAGES: usize = 1000;

/// OpenAlex ids are URLs (`https://openalex.org/W123`); keep the short id.
fn openalex_short_id(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_string()
}

/// Map a single OpenAlex work into a [`Paper`], or `None` if it lacks a title,
/// year, or id.
fn map_work(work: &Value, venue_id: &str) -> Option<Paper> {
    let key = work
        .get("id")
        .and_then(|v| v.as_str())
        .map(openalex_short_id)?;
    let title = work
        .get("title")
        .and_then(|v| v.as_str())
        .or_else(|| work.get("display_name").and_then(|v| v.as_str()))
        .map(str::to_string)?;
    let year = work.get("publication_year").and_then(|v| v.as_i64())? as i32;

    let authors = work
        .get("authorships")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("author")
                        .and_then(|au| au.get("display_name"))
                        .and_then(|v| v.as_str())
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let doi = work
        .get("doi")
        .and_then(|v| v.as_str())
        .and_then(normalized_doi);
    let url = work
        .get("primary_location")
        .and_then(|loc| loc.get("landing_page_url"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")));

    Some(Paper {
        key,
        source: SOURCE_NAME.to_string(),
        venue: venue_id.to_string(),
        year,
        title,
        authors,
        doi,
        url,
        abstract_text: abstract_from_openalex(work),
    })
}

/// Parse one `/works` response page into papers plus the next cursor (if any).
fn parse_works_page(json: &Value, venue_id: &str) -> (Vec<Paper>, Option<String>) {
    let papers = json
        .get("results")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|w| map_work(w, venue_id)).collect())
        .unwrap_or_default();
    let next_cursor = json
        .get("meta")
        .and_then(|m| m.get("next_cursor"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (papers, next_cursor)
}

/// Build the OpenAlex `filter` value for a venue and year range. Prefers the
/// explicit source id; otherwise OR-joins the venue's ISSN(s).
fn build_filter(venue: &Venue, min_year: i32, max_year: i32) -> Result<String> {
    let source_filter = if let Some(sid) = venue
        .openalex_source_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        format!("primary_location.source.id:{sid}")
    } else if !venue.issn.is_empty() {
        format!("locations.source.issn:{}", venue.issn.join("|"))
    } else {
        return Err(Error::Config(format!(
            "venue `{}` has no OpenAlex source id or ISSN",
            venue.id
        )));
    };
    Ok(format!(
        "{source_filter},from_publication_date:{min_year}-01-01,to_publication_date:{max_year}-12-31"
    ))
}

/// HTTP client over the OpenAlex works API.
pub struct OpenAlex {
    base_url: String,
    client: reqwest::Client,
    mailto: Option<String>,
    api_key: Option<String>,
}

impl OpenAlex {
    pub fn new(secrets: &Secrets) -> Self {
        Self::with_base_url(DEFAULT_BASE_URL, secrets)
    }

    pub fn with_base_url(base_url: &str, secrets: &Secrets) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("id-grep/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            mailto: secrets.openalex_mailto.clone(),
            api_key: secrets.openalex_api_key.clone(),
        }
    }
}

impl Source for OpenAlex {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn fetch_venue(&self, venue: &Venue, min_year: i32, max_year: i32) -> Result<Vec<Paper>> {
        let filter = build_filter(venue, min_year, max_year)?;
        let url = format!("{}/works", self.base_url);
        let mut cursor = "*".to_string();
        let mut out = Vec::new();

        for _ in 0..MAX_PAGES {
            let mut query: Vec<(&str, String)> = vec![
                ("filter", filter.clone()),
                ("per-page", PER_PAGE.to_string()),
                ("select", SELECT_FIELDS.to_string()),
                ("cursor", cursor.clone()),
            ];
            if let Some(mailto) = self.mailto.as_deref() {
                query.push(("mailto", mailto.to_string()));
            }
            if let Some(key) = self.api_key.as_deref() {
                query.push(("api_key", key.to_string()));
            }

            let resp = self
                .client
                .get(&url)
                .query(&query)
                .send()
                .await?
                .error_for_status()?;
            let json: Value = resp.json().await?;

            let raw_len = json
                .get("results")
                .and_then(|v| v.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            let (papers, next_cursor) = parse_works_page(&json, &venue.id);
            out.extend(papers);

            match next_cursor {
                Some(next) if raw_len > 0 => cursor = next,
                _ => break,
            }
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn venue(id: &str, issn: &[&str], source_id: Option<&str>) -> Venue {
        Venue {
            id: id.into(),
            name: id.into(),
            dblp_stream: None,
            issn: issn.iter().map(|s| s.to_string()).collect(),
            openalex_source_id: source_id.map(str::to_string),
            pubmed_journal: None,
            aliases: Vec::new(),
            rank: None,
            tags: Vec::new(),
        }
    }

    fn work() -> Value {
        json!({
            "id": "https://openalex.org/W42",
            "doi": "https://doi.org/10.1371/journal.pcbi.1000001",
            "title": "Transmission dynamics of a zoonotic pathogen",
            "publication_year": 2022,
            "authorships": [
                {"author": {"display_name": "Ada Lovelace"}},
                {"author": {"display_name": "Alan Turing"}}
            ],
            "abstract_inverted_index": {
                "We": [0],
                "model": [1],
                "spillover.": [2]
            },
            "primary_location": {"landing_page_url": "https://journals.plos.org/x"}
        })
    }

    #[test]
    fn map_work_extracts_fields() {
        let p = map_work(&work(), "PLoS-Comp-Biol").unwrap();
        assert_eq!(p.key, "W42");
        assert_eq!(p.source, "openalex");
        assert_eq!(p.venue, "PLoS-Comp-Biol");
        assert_eq!(p.year, 2022);
        assert_eq!(p.title, "Transmission dynamics of a zoonotic pathogen");
        assert_eq!(p.authors, "Ada Lovelace, Alan Turing");
        assert_eq!(p.doi.as_deref(), Some("10.1371/journal.pcbi.1000001"));
        assert_eq!(p.url.as_deref(), Some("https://journals.plos.org/x"));
        assert_eq!(p.abstract_text.as_deref(), Some("We model spillover."));
    }

    #[test]
    fn map_work_falls_back_to_doi_url_and_display_name() {
        let w = json!({
            "id": "https://openalex.org/W7",
            "doi": "10.1000/xyz",
            "display_name": "Only a display name",
            "publication_year": 2020,
            "authorships": [],
            "primary_location": {"landing_page_url": null}
        });
        let p = map_work(&w, "Epidemics").unwrap();
        assert_eq!(p.title, "Only a display name");
        assert_eq!(p.authors, "");
        assert_eq!(p.url.as_deref(), Some("https://doi.org/10.1000/xyz"));
        assert!(p.abstract_text.is_none());
    }

    #[test]
    fn map_work_skips_records_without_title_or_year() {
        let no_year = json!({"id": "https://openalex.org/W1", "title": "T"});
        assert!(map_work(&no_year, "V").is_none());
        let no_title = json!({"id": "https://openalex.org/W1", "publication_year": 2021});
        assert!(map_work(&no_title, "V").is_none());
    }

    #[test]
    fn parse_works_page_returns_papers_and_next_cursor() {
        let page = json!({
            "results": [work(), work()],
            "meta": {"next_cursor": "IlsxNjA="}
        });
        let (papers, cursor) = parse_works_page(&page, "V");
        assert_eq!(papers.len(), 2);
        assert_eq!(cursor.as_deref(), Some("IlsxNjA="));
    }

    #[test]
    fn parse_works_page_null_cursor_ends_pagination() {
        let page = json!({ "results": [], "meta": {"next_cursor": null} });
        let (papers, cursor) = parse_works_page(&page, "V");
        assert!(papers.is_empty());
        assert!(cursor.is_none());
    }

    #[test]
    fn build_filter_prefers_source_id() {
        let f = build_filter(&venue("V", &["1234-5678"], Some("S99")), 2020, 2025).unwrap();
        assert!(f.starts_with("primary_location.source.id:S99"));
        assert!(f.contains("from_publication_date:2020-01-01"));
        assert!(f.contains("to_publication_date:2025-12-31"));
    }

    #[test]
    fn build_filter_or_joins_issns() {
        let f = build_filter(&venue("V", &["1234-5678", "2345-6789"], None), 2020, 2025).unwrap();
        assert!(f.starts_with("locations.source.issn:1234-5678|2345-6789"));
    }

    #[test]
    fn build_filter_errors_without_identifiers() {
        assert!(build_filter(&venue("V", &[], None), 2020, 2025).is_err());
    }

    // Live smoke test against the real OpenAlex API. Ignored by default so the
    // suite stays offline; run with `cargo test -- --ignored` where outbound
    // access to api.openalex.org is permitted.
    #[tokio::test]
    #[ignore = "hits the live OpenAlex API"]
    async fn live_fetch_returns_records() {
        let source = OpenAlex::new(&Secrets::default());
        let v = venue("Epidemics", &["1755-4365"], None);
        let papers = source.fetch_venue(&v, 2024, 2025).await.unwrap();
        assert!(!papers.is_empty());
        assert!(papers.iter().all(|p| p.source == SOURCE_NAME));
        assert!(papers.iter().all(|p| p.year >= 2024));
    }
}
