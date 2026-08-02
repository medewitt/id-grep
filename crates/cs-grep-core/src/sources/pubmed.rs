//! PubMed (NCBI E-utilities) ingestion: fetch records for a journal by year.
//!
//! Two calls per page: `esearch` returns PMIDs for a journal + year range, then
//! `efetch` pulls the full records as XML. PubMed complements OpenAlex with
//! strong biomedical/clinical-ID coverage and reliable abstracts. The XML→
//! [`Paper`] mapping and PMID parsing are pure functions, unit-tested offline.

use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::Value;
use tokio::time::sleep;

use crate::abstracts::normalized_doi;
use crate::config::{Secrets, Venue};
use crate::sources::Source;
use crate::{Error, Paper, Result};

/// Value stored in [`Paper::source`] for records from this backend.
pub const SOURCE_NAME: &str = "pubmed";

const DEFAULT_BASE_URL: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";
/// Identifies this client to NCBI (recommended alongside `email`/`api_key`).
const TOOL: &str = "cs-grep";
/// PMIDs requested per `esearch` page.
const ESEARCH_RETMAX: usize = 500;
/// PMIDs fetched per `efetch` call (keeps the GET URL comfortably short).
const EFETCH_BATCH: usize = 200;
/// Safety bound on `esearch` pages (500/page => up to 500k PMIDs per venue).
const MAX_PAGES: usize = 1000;
/// Minimum spacing between requests without an API key (< 3 req/s).
const NO_KEY_DELAY: Duration = Duration::from_millis(350);
/// Minimum spacing between requests with an API key (< 10 req/s).
const KEY_DELAY: Duration = Duration::from_millis(120);

/// Build the `esearch` `term`: journal NLM abbreviation + publication-year range.
fn build_search_term(journal: &str, min_year: i32, max_year: i32) -> String {
    format!("\"{journal}\"[ta] AND {min_year}:{max_year}[dp]")
}

/// Parse an `esearch` JSON response into `(pmids, total_count)`. Lenient: a
/// malformed or unexpected body yields no PMIDs and a zero count so callers stop
/// paginating cleanly.
fn parse_esearch_pmids(json: &str) -> (Vec<String>, usize) {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return (Vec::new(), 0);
    };
    let result = value.get("esearchresult");
    let pmids = result
        .and_then(|r| r.get("idlist"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let count = result
        .and_then(|r| r.get("count"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    (pmids, count)
}

/// Extract a 4-digit year from a free-text `MedlineDate` (e.g. `2020 Jan-Feb`).
fn year_from_medline_date(date: &str) -> Option<i32> {
    date.split(|c: char| !c.is_ascii_digit())
        .find(|group| group.len() == 4)
        .and_then(|group| group.parse::<i32>().ok())
}

/// Accumulator for a single author as its child elements stream in.
#[derive(Default)]
struct AuthorAcc {
    last: String,
    fore: String,
    initials: String,
    collective: String,
}

impl AuthorAcc {
    /// Render as `ForeName LastName` (or initials / collective fallbacks), or
    /// `None` when the author carries no usable name.
    fn into_name(self) -> Option<String> {
        if !self.collective.is_empty() {
            return Some(self.collective);
        }
        let given = if !self.fore.is_empty() {
            self.fore
        } else {
            self.initials
        };
        let name = match (given.is_empty(), self.last.is_empty()) {
            (false, false) => format!("{given} {}", self.last),
            (true, false) => self.last,
            (false, true) => given,
            (true, true) => return None,
        };
        Some(name)
    }
}

/// Accumulator for a single `PubmedArticle` as its elements stream in.
#[derive(Default)]
struct ArticleAcc {
    pmid: String,
    title: String,
    year: Option<i32>,
    medline_date: String,
    doi: Option<String>,
    abstract_sections: Vec<String>,
    authors: Vec<String>,
}

impl ArticleAcc {
    /// Finalize into a [`Paper`], or `None` if it lacks a PMID, title, or year.
    fn into_paper(self, venue_id: &str) -> Option<Paper> {
        let pmid = self.pmid.trim();
        let title = self.title.trim();
        if pmid.is_empty() || title.is_empty() {
            return None;
        }
        let year = self
            .year
            .or_else(|| year_from_medline_date(&self.medline_date))?;

        let abstract_text = (!self.abstract_sections.is_empty())
            .then(|| self.abstract_sections.join(" "))
            .filter(|s| !s.is_empty());

        Some(Paper {
            key: format!("pmid:{pmid}"),
            source: SOURCE_NAME.to_string(),
            venue: venue_id.to_string(),
            year,
            title: title.to_string(),
            authors: self.authors.join(", "),
            doi: self.doi,
            url: Some(format!("https://pubmed.ncbi.nlm.nih.gov/{pmid}/")),
            abstract_text,
        })
    }
}

/// Which text field a capture is currently accumulating into.
#[derive(Clone, Copy)]
enum Capture {
    Title,
    Abstract,
    Year,
    MedlineDate,
    Pmid,
    Doi,
    AuthorLast,
    AuthorFore,
    AuthorInitials,
    AuthorCollective,
}

/// Parse an `efetch` `PubmedArticleSet` XML document into papers for a venue.
///
/// Pure and lenient: malformed input stops parsing and returns what was read so
/// far. MeSH `DescriptorName` elements are tolerated (skipped, not folded in).
pub fn parse_efetch(xml: &str, venue_id: &str) -> Vec<Paper> {
    let mut reader = Reader::from_str(xml);
    let mut papers = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut article: Option<ArticleAcc> = None;
    let mut author: Option<AuthorAcc> = None;

    let mut capture: Option<Capture> = None;
    let mut capture_depth = 0usize;
    let mut buf = String::new();
    let mut label = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                let parent = path.last().cloned();
                path.push(name.clone());
                start_element(
                    &name,
                    parent.as_deref(),
                    &e,
                    path.len(),
                    &mut article,
                    &mut author,
                    &mut capture,
                    &mut capture_depth,
                    &mut buf,
                    &mut label,
                );
            }
            Ok(Event::Empty(e)) => {
                // Self-closing element: enter and immediately leave.
                let name = local_name(e.name().as_ref());
                let parent = path.last().cloned();
                path.push(name.clone());
                start_element(
                    &name,
                    parent.as_deref(),
                    &e,
                    path.len(),
                    &mut article,
                    &mut author,
                    &mut capture,
                    &mut capture_depth,
                    &mut buf,
                    &mut label,
                );
                end_element(
                    &name,
                    &path,
                    &mut article,
                    &mut author,
                    &mut capture,
                    capture_depth,
                    &mut buf,
                    &label,
                    venue_id,
                    &mut papers,
                );
                path.pop();
            }
            Ok(Event::Text(t)) if capture.is_some() => {
                buf.push_str(&t.unescape().unwrap_or_default());
            }
            Ok(Event::CData(t)) if capture.is_some() => {
                buf.push_str(&String::from_utf8_lossy(&t.into_inner()));
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                end_element(
                    &name,
                    &path,
                    &mut article,
                    &mut author,
                    &mut capture,
                    capture_depth,
                    &mut buf,
                    &label,
                    venue_id,
                    &mut papers,
                );
                path.pop();
            }
            Ok(Event::Eof) => break,
            // Lenient: stop at the first malformed event, keep what we parsed.
            Err(_) => break,
            _ => {}
        }
    }

    papers
}

/// Strip any namespace prefix and decode an element name to an owned `String`.
fn local_name(raw: &[u8]) -> String {
    let name = raw.rsplit(|b| *b == b':').next().unwrap_or(raw);
    String::from_utf8_lossy(name).into_owned()
}

/// Read an attribute value (unescaped) by key from a start/empty tag.
fn attr_value(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.as_ref() == key)
            .then(|| a.unescape_value().ok().map(|v| v.into_owned()))
            .flatten()
    })
}

/// Handle a start (or self-closing) tag: open records/authors and begin a text
/// capture when the element + parent identify a field of interest.
#[allow(clippy::too_many_arguments)]
fn start_element(
    name: &str,
    parent: Option<&str>,
    e: &quick_xml::events::BytesStart<'_>,
    depth: usize,
    article: &mut Option<ArticleAcc>,
    author: &mut Option<AuthorAcc>,
    capture: &mut Option<Capture>,
    capture_depth: &mut usize,
    buf: &mut String,
    label: &mut String,
) {
    match name {
        "PubmedArticle" => *article = Some(ArticleAcc::default()),
        "Author" => *author = Some(AuthorAcc::default()),
        _ => {}
    }

    // Only one capture is active at a time; nested markup (e.g. <i> in a title)
    // is captured as text rather than starting a new field.
    if capture.is_some() {
        return;
    }

    let kind = match (name, parent) {
        ("ArticleTitle", Some("Article")) => Some(Capture::Title),
        ("AbstractText", Some("Abstract")) => Some(Capture::Abstract),
        ("Year", Some("PubDate")) => Some(Capture::Year),
        ("MedlineDate", Some("PubDate")) => Some(Capture::MedlineDate),
        ("PMID", Some("MedlineCitation")) => Some(Capture::Pmid),
        ("ArticleId", Some("ArticleIdList")) => match attr_value(e, b"IdType").as_deref() {
            Some("doi") => Some(Capture::Doi),
            _ => None,
        },
        ("LastName", Some("Author")) => Some(Capture::AuthorLast),
        ("ForeName", Some("Author")) => Some(Capture::AuthorFore),
        ("Initials", Some("Author")) => Some(Capture::AuthorInitials),
        ("CollectiveName", Some("Author")) => Some(Capture::AuthorCollective),
        _ => None,
    };

    if let Some(kind) = kind {
        *capture = Some(kind);
        *capture_depth = depth;
        buf.clear();
        if matches!(kind, Capture::Abstract) {
            *label = attr_value(e, b"Label").unwrap_or_default();
        }
    }
}

/// Handle an end tag: finalize an active capture, and close authors/records.
#[allow(clippy::too_many_arguments)]
fn end_element(
    name: &str,
    path: &[String],
    article: &mut Option<ArticleAcc>,
    author: &mut Option<AuthorAcc>,
    capture: &mut Option<Capture>,
    capture_depth: usize,
    buf: &mut String,
    label: &str,
    venue_id: &str,
    papers: &mut Vec<Paper>,
) {
    if let Some(kind) = *capture {
        if path.len() == capture_depth {
            finalize_capture(kind, buf.trim(), label, article, author);
            *capture = None;
            buf.clear();
        }
    }

    match name {
        "Author" => {
            if let (Some(acc), Some(art)) = (author.take(), article.as_mut()) {
                if let Some(name) = acc.into_name() {
                    art.authors.push(name);
                }
            }
        }
        "PubmedArticle" => {
            if let Some(acc) = article.take() {
                if let Some(paper) = acc.into_paper(venue_id) {
                    papers.push(paper);
                }
            }
        }
        _ => {}
    }
}

/// Assign a finished capture buffer to the right accumulator field.
fn finalize_capture(
    kind: Capture,
    text: &str,
    label: &str,
    article: &mut Option<ArticleAcc>,
    author: &mut Option<AuthorAcc>,
) {
    match kind {
        Capture::Title => {
            if let Some(art) = article.as_mut() {
                art.title = text.to_string();
            }
        }
        Capture::Abstract => {
            if !text.is_empty() {
                if let Some(art) = article.as_mut() {
                    let section = if label.is_empty() {
                        text.to_string()
                    } else {
                        format!("{label}: {text}")
                    };
                    art.abstract_sections.push(section);
                }
            }
        }
        Capture::Year => {
            if let Some(art) = article.as_mut() {
                if art.year.is_none() {
                    art.year = text.parse::<i32>().ok();
                }
            }
        }
        Capture::MedlineDate => {
            if let Some(art) = article.as_mut() {
                art.medline_date = text.to_string();
            }
        }
        Capture::Pmid => {
            if let Some(art) = article.as_mut() {
                if art.pmid.is_empty() {
                    art.pmid = text.to_string();
                }
            }
        }
        Capture::Doi => {
            if let Some(art) = article.as_mut() {
                art.doi = normalized_doi(text);
            }
        }
        Capture::AuthorLast => {
            if let Some(a) = author.as_mut() {
                a.last = text.to_string();
            }
        }
        Capture::AuthorFore => {
            if let Some(a) = author.as_mut() {
                a.fore = text.to_string();
            }
        }
        Capture::AuthorInitials => {
            if let Some(a) = author.as_mut() {
                a.initials = text.to_string();
            }
        }
        Capture::AuthorCollective => {
            if let Some(a) = author.as_mut() {
                a.collective = text.to_string();
            }
        }
    }
}

/// HTTP client over the NCBI E-utilities `esearch`/`efetch` endpoints.
pub struct PubMed {
    base_url: String,
    client: reqwest::Client,
    api_key: Option<String>,
    email: Option<String>,
}

impl PubMed {
    pub fn new(secrets: &Secrets) -> Self {
        Self::with_base_url(DEFAULT_BASE_URL, secrets)
    }

    pub fn with_base_url(base_url: &str, secrets: &Secrets) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("cs-grep/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            api_key: secrets.ncbi_api_key.clone(),
            email: secrets.ncbi_email.clone(),
        }
    }

    /// Minimum spacing between requests to respect NCBI's rate limits.
    fn request_delay(&self) -> Duration {
        if self.api_key.is_some() {
            KEY_DELAY
        } else {
            NO_KEY_DELAY
        }
    }

    /// Query params common to every E-utilities call (`db`, `tool`, creds).
    fn common_params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("db", "pubmed".to_string()), ("tool", TOOL.to_string())];
        if let Some(email) = self.email.as_deref() {
            params.push(("email", email.to_string()));
        }
        if let Some(key) = self.api_key.as_deref() {
            params.push(("api_key", key.to_string()));
        }
        params
    }

    /// Page through `esearch` collecting all PMIDs for a term.
    async fn search_pmids(&self, term: &str) -> Result<Vec<String>> {
        let url = format!("{}/esearch.fcgi", self.base_url);
        let mut pmids = Vec::new();
        let mut retstart = 0usize;

        for _ in 0..MAX_PAGES {
            sleep(self.request_delay()).await;
            let mut params = self.common_params();
            params.push(("term", term.to_string()));
            params.push(("retmode", "json".to_string()));
            params.push(("retmax", ESEARCH_RETMAX.to_string()));
            params.push(("retstart", retstart.to_string()));

            let body = self
                .client
                .get(&url)
                .query(&params)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;

            let (ids, count) = parse_esearch_pmids(&body);
            if ids.is_empty() {
                break;
            }
            pmids.extend(ids);
            retstart += ESEARCH_RETMAX;
            if pmids.len() >= count {
                break;
            }
        }

        Ok(pmids)
    }

    /// Fetch and parse the records for a batch of PMIDs.
    async fn fetch_records(&self, pmids: &[String], venue_id: &str) -> Result<Vec<Paper>> {
        let url = format!("{}/efetch.fcgi", self.base_url);
        let mut out = Vec::new();

        for chunk in pmids.chunks(EFETCH_BATCH) {
            sleep(self.request_delay()).await;
            let mut params = self.common_params();
            params.push(("id", chunk.join(",")));
            params.push(("retmode", "xml".to_string()));

            let xml = self
                .client
                .get(&url)
                .query(&params)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;

            out.extend(parse_efetch(&xml, venue_id));
        }

        Ok(out)
    }
}

impl Source for PubMed {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn fetch_venue(&self, venue: &Venue, min_year: i32, max_year: i32) -> Result<Vec<Paper>> {
        let journal = venue
            .pubmed_journal
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Config(format!("venue `{}` has no pubmed_journal", venue.id)))?;
        let term = build_search_term(journal, min_year, max_year);
        let pmids = self.search_pmids(&term).await?;
        if pmids.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_records(&pmids, &venue.id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venue(id: &str, journal: Option<&str>) -> Venue {
        Venue {
            id: id.into(),
            name: id.into(),
            dblp_stream: None,
            issn: Vec::new(),
            openalex_source_id: None,
            pubmed_journal: journal.map(str::to_string),
            aliases: Vec::new(),
            rank: None,
            tags: Vec::new(),
        }
    }

    const EFETCH_XML: &str = r#"<?xml version="1.0" ?>
<!DOCTYPE PubmedArticleSet SYSTEM "https://dtd.nlm.nih.gov/ncbi/pubmed/out/pubmed_190101.dtd">
<PubmedArticleSet>
  <PubmedArticle>
    <MedlineCitation Status="MEDLINE">
      <PMID Version="1">40000001</PMID>
      <Article PubModel="Print">
        <Journal>
          <JournalIssue>
            <PubDate>
              <Year>2023</Year>
              <Month>Jun</Month>
            </PubDate>
          </JournalIssue>
        </Journal>
        <ArticleTitle>Spatial dynamics of <i>Plasmodium</i> transmission</ArticleTitle>
        <Abstract>
          <AbstractText Label="BACKGROUND">We study malaria.</AbstractText>
          <AbstractText Label="RESULTS">Transmission is seasonal &amp; clustered.</AbstractText>
        </Abstract>
        <AuthorList CompleteYN="Y">
          <Author ValidYN="Y">
            <LastName>Lovelace</LastName>
            <ForeName>Ada</ForeName>
            <Initials>A</Initials>
          </Author>
          <Author ValidYN="Y">
            <LastName>Turing</LastName>
            <ForeName>Alan M</ForeName>
            <Initials>AM</Initials>
          </Author>
        </AuthorList>
      </Article>
      <MeshHeadingList>
        <MeshHeading>
          <DescriptorName UI="D008288" MajorTopicYN="N">Malaria</DescriptorName>
          <QualifierName UI="Q000453" MajorTopicYN="Y">epidemiology</QualifierName>
        </MeshHeading>
      </MeshHeadingList>
    </MedlineCitation>
    <PubmedData>
      <ArticleIdList>
        <ArticleId IdType="pubmed">40000001</ArticleId>
        <ArticleId IdType="doi">10.1234/ABC.2023.001</ArticleId>
      </ArticleIdList>
    </PubmedData>
  </PubmedArticle>
  <PubmedArticle>
    <MedlineCitation Status="MEDLINE">
      <PMID Version="1">40000002</PMID>
      <Article>
        <Journal>
          <JournalIssue>
            <PubDate>
              <MedlineDate>2024 Jan-Feb</MedlineDate>
            </PubDate>
          </JournalIssue>
        </Journal>
        <ArticleTitle>Herd immunity thresholds</ArticleTitle>
        <Abstract>
          <AbstractText>A single unlabelled abstract.</AbstractText>
        </Abstract>
        <AuthorList>
          <Author>
            <CollectiveName>The Study Group</CollectiveName>
          </Author>
          <Author>
            <LastName>Nightingale</LastName>
            <Initials>F</Initials>
          </Author>
        </AuthorList>
      </Article>
    </MedlineCitation>
    <PubmedData>
      <ArticleIdList>
        <ArticleId IdType="pubmed">40000002</ArticleId>
      </ArticleIdList>
    </PubmedData>
  </PubmedArticle>
</PubmedArticleSet>"#;

    #[test]
    fn build_search_term_uses_ta_and_dp() {
        let term = build_search_term("Emerg Infect Dis", 2020, 2025);
        assert_eq!(term, "\"Emerg Infect Dis\"[ta] AND 2020:2025[dp]");
    }

    #[test]
    fn parse_esearch_extracts_pmids_and_count() {
        let json = r#"{"esearchresult":{"count":"42","retmax":"2","retstart":"0",
            "idlist":["40000001","40000002"]}}"#;
        let (pmids, count) = parse_esearch_pmids(json);
        assert_eq!(pmids, vec!["40000001".to_string(), "40000002".to_string()]);
        assert_eq!(count, 42);
    }

    #[test]
    fn parse_esearch_handles_empty_and_malformed() {
        let (pmids, count) = parse_esearch_pmids(r#"{"esearchresult":{"count":"0","idlist":[]}}"#);
        assert!(pmids.is_empty());
        assert_eq!(count, 0);
        let (pmids, count) = parse_esearch_pmids("not json");
        assert!(pmids.is_empty());
        assert_eq!(count, 0);
    }

    #[test]
    fn parse_efetch_maps_first_article() {
        let papers = parse_efetch(EFETCH_XML, "EID");
        assert_eq!(papers.len(), 2);
        let p = &papers[0];
        assert_eq!(p.key, "pmid:40000001");
        assert_eq!(p.source, "pubmed");
        assert_eq!(p.venue, "EID");
        assert_eq!(p.year, 2023);
        assert_eq!(p.title, "Spatial dynamics of Plasmodium transmission");
        assert_eq!(p.authors, "Ada Lovelace, Alan M Turing");
        assert_eq!(p.doi.as_deref(), Some("10.1234/abc.2023.001"));
        assert_eq!(
            p.url.as_deref(),
            Some("https://pubmed.ncbi.nlm.nih.gov/40000001/")
        );
        assert_eq!(
            p.abstract_text.as_deref(),
            Some("BACKGROUND: We study malaria. RESULTS: Transmission is seasonal & clustered.")
        );
    }

    #[test]
    fn parse_efetch_handles_collective_initials_and_medline_date() {
        let papers = parse_efetch(EFETCH_XML, "EID");
        let p = &papers[1];
        assert_eq!(p.key, "pmid:40000002");
        // Year falls back to the leading 4 digits of the MedlineDate.
        assert_eq!(p.year, 2024);
        assert_eq!(p.title, "Herd immunity thresholds");
        // CollectiveName author, then LastName + Initials fallback.
        assert_eq!(p.authors, "The Study Group, F Nightingale");
        assert!(p.doi.is_none());
        assert_eq!(
            p.abstract_text.as_deref(),
            Some("A single unlabelled abstract.")
        );
    }

    #[test]
    fn parse_efetch_skips_records_without_title_or_year() {
        let xml = r#"<PubmedArticleSet>
  <PubmedArticle>
    <MedlineCitation>
      <PMID>1</PMID>
      <Article>
        <Journal><JournalIssue><PubDate><Year>2020</Year></PubDate></JournalIssue></Journal>
      </Article>
    </MedlineCitation>
  </PubmedArticle>
  <PubmedArticle>
    <MedlineCitation>
      <PMID>2</PMID>
      <Article>
        <ArticleTitle>No year here</ArticleTitle>
      </Article>
    </MedlineCitation>
  </PubmedArticle>
</PubmedArticleSet>"#;
        assert!(parse_efetch(xml, "V").is_empty());
    }

    #[tokio::test]
    async fn fetch_venue_errors_without_pubmed_journal() {
        let source = PubMed::new(&Secrets::default());
        let v = venue("V", None);
        let err = source.fetch_venue(&v, 2020, 2025).await.unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    // Live smoke test against the real NCBI E-utilities API. Ignored by default
    // so the suite stays offline; run with `cargo test -- --ignored` where
    // outbound access to eutils.ncbi.nlm.nih.gov is permitted.
    #[tokio::test]
    #[ignore = "hits the live NCBI E-utilities API"]
    async fn live_fetch_returns_records() {
        let source = PubMed::new(&Secrets::default());
        let v = venue("EID", Some("Emerg Infect Dis"));
        let papers = source.fetch_venue(&v, 2024, 2024).await.unwrap();
        assert!(!papers.is_empty());
        assert!(papers.iter().all(|p| p.source == SOURCE_NAME));
        assert!(papers.iter().all(|p| p.year == 2024));
        assert!(papers.iter().all(|p| p.key.starts_with("pmid:")));
    }
}
