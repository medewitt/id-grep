//! Read a local Zotero library and use it to deduplicate search results.
//!
//! Zotero keeps its metadata in a single SQLite file (`zotero.sqlite`) and
//! holds a write lock on it while running. To read it safely we copy the file
//! to a throwaway location and open the copy read-only (`immutable=1`, which
//! also lets us ignore any WAL side-files), then materialize the items we care
//! about into [`Paper`] records.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::{Error, Paper, Result};

/// Value stored in [`Paper::source`] for records read from a Zotero library.
pub const SOURCE_NAME: &str = "zotero";

/// Item types that are not standalone bibliographic entries.
const NON_ITEM_TYPES: &[&str] = &["attachment", "note", "annotation"];

/// A read-only snapshot of a local Zotero library.
#[derive(Debug, Clone)]
pub struct ZoteroLibrary {
    papers: Vec<Paper>,
    dois: HashSet<String>,
    titles: HashSet<String>,
}

impl ZoteroLibrary {
    /// Open the `zotero.sqlite` database found inside `zotero_dir`.
    ///
    /// The database is copied to a temporary file before opening, so this works
    /// even while Zotero is running and holding its write lock.
    pub fn open(zotero_dir: &Path) -> Result<Self> {
        let src = zotero_dir.join("zotero.sqlite");
        if !src.is_file() {
            return Err(Error::Config(format!(
                "no zotero.sqlite in {}; pass the Zotero data directory via --zotero",
                zotero_dir.display()
            )));
        }

        let tmp = tempfile::tempdir()?;
        let copy = tmp.path().join("zotero.sqlite");
        std::fs::copy(&src, &copy)?;

        // `immutable=1` promises SQLite the file will not change, which lets it
        // open a WAL-mode database read-only without creating shared-memory or
        // WAL side-files.
        let uri = format!("file:{}?immutable=1", copy.display());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;

        let papers = read_papers(&conn)?;
        let mut dois = HashSet::new();
        let mut titles = HashSet::new();
        for paper in &papers {
            if let Some(doi) = &paper.doi {
                let norm = normalize_doi(doi);
                if !norm.is_empty() {
                    dois.insert(norm);
                }
            }
            let title = normalize_title(&paper.title);
            if !title.is_empty() {
                titles.insert(title);
            }
        }

        Ok(Self {
            papers,
            dois,
            titles,
        })
    }

    /// Normalized (lowercased) DOIs of every item in the library.
    pub fn owned_dois(&self) -> HashSet<String> {
        self.dois.clone()
    }

    /// Normalized (lowercased, alphanumeric-collapsed) titles of every item.
    pub fn owned_titles(&self) -> HashSet<String> {
        self.titles.clone()
    }

    /// Materialize the library's items as [`Paper`] records.
    pub fn into_papers(&self) -> Vec<Paper> {
        self.papers.clone()
    }

    /// Number of items read from the library.
    pub fn len(&self) -> usize {
        self.papers.len()
    }

    /// Whether the library contains no bibliographic items.
    pub fn is_empty(&self) -> bool {
        self.papers.is_empty()
    }

    /// Whether `paper` is already in the library, matched by DOI when present,
    /// otherwise by normalized title.
    pub fn is_owned(&self, paper: &Paper) -> bool {
        if let Some(doi) = &paper.doi {
            let norm = normalize_doi(doi);
            if !norm.is_empty() {
                return self.dois.contains(&norm);
            }
        }
        let title = normalize_title(&paper.title);
        !title.is_empty() && self.titles.contains(&title)
    }
}

/// Locate a Zotero data directory in the common default location (`~/Zotero`).
pub fn default_data_dir() -> Option<PathBuf> {
    let home = directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    [home.join("Zotero")]
        .into_iter()
        .find(|dir| dir.join("zotero.sqlite").is_file())
}

fn read_papers(conn: &Connection) -> Result<Vec<Paper>> {
    let item_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT i.itemID \
             FROM items i \
             JOIN itemTypes t ON i.itemTypeID = t.itemTypeID \
             WHERE t.typeName NOT IN (?1, ?2, ?3) \
               AND i.itemID NOT IN (SELECT itemID FROM deletedItems) \
             ORDER BY i.itemID",
        )?;
        let rows = stmt.query_map(
            params![NON_ITEM_TYPES[0], NON_ITEM_TYPES[1], NON_ITEM_TYPES[2]],
            |row| row.get::<_, i64>(0),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut field_stmt = conn.prepare(
        "SELECT v.value \
         FROM itemData d \
         JOIN itemDataValues v ON d.valueID = v.valueID \
         JOIN fields f ON d.fieldID = f.fieldID \
         WHERE d.itemID = ?1 AND f.fieldName = ?2",
    )?;
    let mut creator_stmt = conn.prepare(
        "SELECT c.lastName \
         FROM itemCreators ic \
         JOIN creators c ON ic.creatorID = c.creatorID \
         WHERE ic.itemID = ?1 \
         ORDER BY ic.orderIndex",
    )?;

    let mut papers = Vec::with_capacity(item_ids.len());
    for item_id in item_ids {
        let title = field_value(&mut field_stmt, item_id, "title")?.unwrap_or_default();
        let doi = field_value(&mut field_stmt, item_id, "DOI")?.filter(|s| !s.is_empty());
        let url = field_value(&mut field_stmt, item_id, "url")?.filter(|s| !s.is_empty());
        let venue = field_value(&mut field_stmt, item_id, "publicationTitle")?.unwrap_or_default();
        let abstract_text =
            field_value(&mut field_stmt, item_id, "abstractNote")?.filter(|s| !s.is_empty());
        let year = field_value(&mut field_stmt, item_id, "date")?
            .as_deref()
            .map(parse_year)
            .unwrap_or(0);

        let last_names = creator_stmt
            .query_map(params![item_id], |row| row.get::<_, Option<String>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let authors = last_names
            .into_iter()
            .flatten()
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", ");

        papers.push(Paper {
            key: format!("zotero:{item_id}"),
            source: SOURCE_NAME.to_string(),
            venue,
            year,
            title,
            authors,
            doi,
            url,
            abstract_text,
        });
    }

    Ok(papers)
}

fn field_value(
    stmt: &mut rusqlite::Statement<'_>,
    item_id: i64,
    field: &str,
) -> Result<Option<String>> {
    Ok(stmt
        .query_row(params![item_id, field], |row| row.get::<_, String>(0))
        .optional()?)
}

/// Extract the first four-digit run from a Zotero `date` value as a year.
fn parse_year(date: &str) -> i32 {
    let bytes = date.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].iter().all(u8::is_ascii_digit) {
            return date[i..i + 4].parse().unwrap_or(0);
        }
        i += 1;
    }
    0
}

/// Lowercase a DOI and strip common URL / scheme prefixes.
fn normalize_doi(doi: &str) -> String {
    let lowered = doi.trim().to_ascii_lowercase();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ] {
        if let Some(rest) = lowered.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    lowered
}

/// Lowercase a title and drop every non-alphanumeric character.
fn normalize_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Build a minimal fixture with the subset of the Zotero schema we query.
    fn fixture(dir: &Path) {
        let conn = Connection::open(dir.join("zotero.sqlite")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT);
            CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER);
            CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT);
            CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
            CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
            CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
            CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
            CREATE TABLE deletedItems (itemID INTEGER PRIMARY KEY);

            INSERT INTO itemTypes VALUES (1,'journalArticle'), (2,'attachment'), (3,'note');

            INSERT INTO fields VALUES
                (1,'title'), (2,'DOI'), (3,'date'), (4,'publicationTitle'),
                (5,'url'), (6,'abstractNote');

            -- item 1: a normal article with a DOI and two creators
            INSERT INTO items VALUES (1,1);
            INSERT INTO itemDataValues VALUES
                (10,'Spatial spread of a vector-borne pathogen'),
                (11,'10.1000/XYZ'),
                (12,'2017-06-12'),
                (13,'Epidemics'),
                (14,'https://example.com/spread'),
                (15,'we model the wavefront');
            INSERT INTO itemData VALUES
                (1,1,10),(1,2,11),(1,3,12),(1,4,13),(1,5,14),(1,6,15);
            INSERT INTO creators VALUES (100,'Ada','Lovelace',0), (101,'Alan','Turing',0);
            INSERT INTO itemCreators VALUES (1,101,1,1), (1,100,1,0);

            -- item 2: an article without a DOI (dedup falls back to title)
            INSERT INTO items VALUES (2,1);
            INSERT INTO itemDataValues VALUES
                (20,'Reservoir hosts and cross-species transmission'),
                (21,'2016'),
                (22,'Ecology Letters');
            INSERT INTO itemData VALUES (2,1,20),(2,3,21),(2,4,22);
            INSERT INTO creators VALUES (200,'Rosalind','Franklin',0);
            INSERT INTO itemCreators VALUES (2,200,1,0);

            -- item 3: an attachment (must be ignored)
            INSERT INTO items VALUES (3,2);

            -- item 4: a deleted article (must be ignored)
            INSERT INTO items VALUES (4,1);
            INSERT INTO itemDataValues VALUES (40,'Trashed Paper');
            INSERT INTO itemData VALUES (4,1,40);
            INSERT INTO deletedItems VALUES (4);
            "#,
        )
        .unwrap();
    }

    fn paper(title: &str, doi: Option<&str>) -> Paper {
        Paper {
            key: "x".into(),
            source: "openalex".into(),
            venue: String::new(),
            year: 0,
            title: title.into(),
            authors: String::new(),
            doi: doi.map(str::to_string),
            url: None,
            abstract_text: None,
        }
    }

    fn open_fixture() -> ZoteroLibrary {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        ZoteroLibrary::open(dir.path()).unwrap()
    }

    #[test]
    fn open_missing_database_is_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = ZoteroLibrary::open(dir.path()).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn reads_only_real_items() {
        let lib = open_fixture();
        assert_eq!(lib.len(), 2);
        let papers = lib.into_papers();
        let keys: Vec<_> = papers.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, vec!["zotero:1", "zotero:2"]);
        assert!(papers.iter().all(|p| p.source == SOURCE_NAME));
    }

    #[test]
    fn materializes_fields_and_ordered_authors() {
        let papers = open_fixture().into_papers();
        let first = &papers[0];
        assert_eq!(first.title, "Spatial spread of a vector-borne pathogen");
        assert_eq!(first.venue, "Epidemics");
        assert_eq!(first.year, 2017);
        assert_eq!(first.doi.as_deref(), Some("10.1000/XYZ"));
        assert_eq!(first.url.as_deref(), Some("https://example.com/spread"));
        assert_eq!(
            first.abstract_text.as_deref(),
            Some("we model the wavefront")
        );
        // orderIndex 0 (Lovelace) before orderIndex 1 (Turing)
        assert_eq!(first.authors, "Lovelace, Turing");

        let second = &papers[1];
        assert_eq!(second.year, 2016);
        assert_eq!(second.doi, None);
        assert_eq!(second.authors, "Franklin");
    }

    #[test]
    fn owned_dois_and_titles_are_normalized() {
        let lib = open_fixture();
        let dois = lib.owned_dois();
        assert!(dois.contains("10.1000/xyz"));
        assert_eq!(dois.len(), 1);

        let titles = lib.owned_titles();
        assert!(titles.contains("spatialspreadofavectorbornepathogen"));
        assert!(titles.contains("reservoirhostsandcrossspeciestransmission"));
    }

    #[test]
    fn is_owned_matches_by_doi_then_title() {
        let lib = open_fixture();

        // DOI match wins, case-insensitive.
        assert!(lib.is_owned(&paper("totally different title", Some("10.1000/xyz"))));
        // A present-but-unknown DOI is not owned even if the title matches.
        assert!(!lib.is_owned(&paper(
            "Spatial spread of a vector-borne pathogen",
            Some("10.9999/nope")
        )));
        // No DOI: fall back to normalized title (punctuation/case ignored).
        assert!(lib.is_owned(&paper(
            "Reservoir hosts and cross-species transmission!",
            None
        )));
        // Unknown item is not owned.
        assert!(!lib.is_owned(&paper("Some Paper We Do Not Have", None)));
    }

    #[test]
    fn parse_year_extracts_leading_four_digits() {
        assert_eq!(parse_year("2021-03-15"), 2021);
        assert_eq!(parse_year("March 2019"), 2019);
        assert_eq!(parse_year("n.d."), 0);
        assert_eq!(parse_year(""), 0);
    }
}
