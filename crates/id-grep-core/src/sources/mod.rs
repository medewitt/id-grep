//! Source-agnostic ingestion.
//!
//! A [`Source`] fetches paper metadata for a venue within a year range. This
//! decouples ingestion from any single backend so new sources (OpenAlex,
//! PubMed, Zotero, ...) can be added without touching the storage, query, or
//! output layers.

use crate::config::Venue;
use crate::{Paper, Result};

pub mod dblp;
pub mod openalex;
pub mod pubmed;

/// A backend that fetches paper metadata for a venue within a year range.
///
/// Implementors set [`Paper::source`] to their [`Source::name`] so records can
/// be traced back to where they came from.
// `async fn` in a trait is exactly what we want here (each source awaits its own
// HTTP client); we only ever use static dispatch, so the auto-trait caveat the
// lint warns about does not apply.
#[allow(async_fn_in_trait)]
pub trait Source {
    /// Short identifier stored in [`Paper::source`] (e.g. `dblp`, `openalex`).
    fn name(&self) -> &str;

    /// Fetch all papers for `venue` published in `[min_year, max_year]`.
    async fn fetch_venue(&self, venue: &Venue, min_year: i32, max_year: i32) -> Result<Vec<Paper>>;
}
