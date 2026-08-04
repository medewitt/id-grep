//! SQLite storage with an FTS5 full-text index over papers.

use std::path::Path;

use rusqlite::{params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension, Row};

use crate::config::{Config, RankSortOrder};
use crate::query::{FilterExpr, YearRange};
use crate::{Paper, Result};

/// Storage schema version. Bump when the `papers` layout changes; a database
/// written by an older version is dropped and rebuilt on open (the index is a
/// local cache that `update` can always repopulate).
const DB_VERSION: i64 = 2;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS papers (
    id         INTEGER PRIMARY KEY,
    paper_key  TEXT UNIQUE NOT NULL,
    source     TEXT NOT NULL DEFAULT '',
    venue      TEXT NOT NULL,
    year       INTEGER NOT NULL,
    title      TEXT NOT NULL,
    authors    TEXT NOT NULL,
    doi        TEXT,
    url        TEXT,
    abstract   TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_papers_venue_year ON papers(venue, year);
CREATE INDEX IF NOT EXISTS idx_papers_year_venue ON papers(year DESC, venue ASC, id);

CREATE VIRTUAL TABLE IF NOT EXISTS papers_fts USING fts5(
    title, authors, abstract,
    content = 'papers',
    content_rowid = 'id',
    tokenize = 'porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS papers_ai AFTER INSERT ON papers BEGIN
    INSERT INTO papers_fts(rowid, title, authors, abstract)
    VALUES (new.id, new.title, new.authors, new.abstract);
END;

CREATE TRIGGER IF NOT EXISTS papers_ad AFTER DELETE ON papers BEGIN
    INSERT INTO papers_fts(papers_fts, rowid, title, authors, abstract)
    VALUES ('delete', old.id, old.title, old.authors, old.abstract);
END;

CREATE TRIGGER IF NOT EXISTS papers_au AFTER UPDATE ON papers BEGIN
    INSERT INTO papers_fts(papers_fts, rowid, title, authors, abstract)
    VALUES ('delete', old.id, old.title, old.authors, old.abstract);
    INSERT INTO papers_fts(rowid, title, authors, abstract)
    VALUES (new.id, new.title, new.authors, new.abstract);
END;

-- User data, not a rebuildable cache: never dropped by the schema-version
-- rebuild in `Database::init` below.
CREATE TABLE IF NOT EXISTS saved_searches (
    id           INTEGER PRIMARY KEY,
    name         TEXT UNIQUE NOT NULL,
    query        TEXT NOT NULL,
    last_run_at  TEXT
);
"#;

const PAPER_COLUMNS_WITH_ALIAS: &str =
    "p.paper_key, p.source, p.venue, p.year, p.title, p.authors, p.doi, p.url, p.abstract";

/// How to order search results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Sort {
    /// BM25 relevance for full-text searches; otherwise year descending.
    #[default]
    Relevance,
    Year,
    Venue,
    /// Venue groups in priority order; venues outside the groups sort last.
    Rank(RankSortOrder),
}

/// A compiled search request.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub fts: Option<String>,
    pub filter: Option<FilterExpr>,
    pub sort: Sort,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct MissingPaper {
    pub id: i64,
    pub paper: Paper,
}

/// A named query persisted for repeated `search run` invocations.
///
/// `last_run_at` is `None` until the first successful run; once set, it's
/// the same `datetime('now')` text format as `papers.updated_at`, so the two
/// are directly comparable via `FilterExpr::AddedSince`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSearch {
    pub name: String,
    pub query: String,
    pub last_run_at: Option<String>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let has_papers: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'papers'",
            [],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if has_papers && version < DB_VERSION {
            // Incompatible schema from an older build: drop and rebuild. The
            // index is a rebuildable cache, so no data migration is needed.
            conn.execute_batch(
                r#"
                DROP TRIGGER IF EXISTS papers_ai;
                DROP TRIGGER IF EXISTS papers_ad;
                DROP TRIGGER IF EXISTS papers_au;
                DROP TABLE IF EXISTS papers_fts;
                DROP TABLE IF EXISTS papers;
                "#,
            )?;
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", DB_VERSION)?;
        Ok(Self { conn })
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM papers", [], |r| r.get(0))?)
    }

    /// Insert or update papers keyed by `paper_key`. Returns rows affected.
    ///
    /// Identity is the source-scoped `key`; cross-source de-duplication by DOI
    /// or normalized title is deferred until multiple ingestion sources exist.
    pub fn upsert_papers(&mut self, papers: &[Paper]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO papers (paper_key, source, venue, year, title, authors, doi, url, abstract, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))
                ON CONFLICT(paper_key) DO UPDATE SET
                    source = excluded.source,
                    venue = excluded.venue,
                    year = excluded.year,
                    title = excluded.title,
                    authors = excluded.authors,
                    doi = excluded.doi,
                    url = excluded.url,
                    abstract = COALESCE(excluded.abstract, papers.abstract),
                    updated_at = datetime('now')
                WHERE
                    papers.source IS NOT excluded.source OR
                    papers.venue IS NOT excluded.venue OR
                    papers.year IS NOT excluded.year OR
                    papers.title IS NOT excluded.title OR
                    papers.authors IS NOT excluded.authors OR
                    papers.doi IS NOT excluded.doi OR
                    papers.url IS NOT excluded.url OR
                    (excluded.abstract IS NOT NULL AND papers.abstract IS NOT excluded.abstract)
                "#,
            )?;
            for p in papers {
                n += stmt.execute(rusqlite::params![
                    p.key,
                    p.source,
                    p.venue,
                    p.year,
                    p.title,
                    p.authors,
                    p.doi,
                    p.url,
                    p.abstract_text,
                ])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn set_abstracts(&mut self, abstracts: &[(String, String)]) -> Result<()> {
        if abstracts.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE papers SET abstract = ?2, updated_at = datetime('now') WHERE paper_key = ?1",
            )?;
            for (paper_key, abstract_text) in abstracts {
                stmt.execute(rusqlite::params![paper_key, abstract_text])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn papers_missing_abstract_batch(
        &self,
        venues: &[String],
        years: &[YearRange],
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<MissingPaper>> {
        let mut parts = missing_abstract_parts(venues, years, Some(after_id));
        let next = parts.args.len() + 1;
        let sql = format!(
            "SELECT p.id, {PAPER_COLUMNS_WITH_ALIAS} FROM papers p {} \
             ORDER BY p.id ASC LIMIT ?{next}",
            parts.where_sql
        );
        parts.args.push((limit as i64).into());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(parts.args.iter()), row_to_missing_paper)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    pub fn count_missing_abstracts(&self, venues: &[String], years: &[YearRange]) -> Result<usize> {
        let parts = missing_abstract_parts(venues, years, None);
        let sql = format!("SELECT COUNT(*) FROM papers p {}", parts.where_sql);
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(parts.args.iter()), |r| r.get(0))?;
        Ok(count as usize)
    }

    pub fn search(&self, q: &Search) -> Result<Vec<Paper>> {
        let mut parts = search_query_parts(q);
        let order = order_clause(q, &mut parts.args);

        let mut sql = format!(
            "SELECT {PAPER_COLUMNS_WITH_ALIAS} FROM {} {} {order}",
            parts.from, parts.where_sql
        );
        if let Some(n) = q.limit {
            let next = parts.args.len() + 1;
            sql.push_str(&format!(" LIMIT ?{next}"));
            parts.args.push((n as i64).into());
        }
        if let Some(n) = q.offset {
            if q.limit.is_none() {
                sql.push_str(" LIMIT -1");
            }
            let next = parts.args.len() + 1;
            sql.push_str(&format!(" OFFSET ?{next}"));
            parts.args.push((n as i64).into());
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(parts.args.iter()), row_to_paper)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    pub fn search_count(&self, q: &Search) -> Result<usize> {
        let parts = search_query_parts(q);
        let sql = format!("SELECT COUNT(*) FROM {} {}", parts.from, parts.where_sql);
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(parts.args.iter()), |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Persist a named query, validating it parses first. Resaving an
    /// existing name replaces its query and resets `last_run_at` to `None`
    /// (last writer wins; a changed query starts tracking fresh).
    pub fn save_search(&mut self, name: &str, query: &str, config: &Config) -> Result<()> {
        crate::query::parse(query, config)?;
        self.conn.execute(
            "INSERT INTO saved_searches (name, query, last_run_at) VALUES (?1, ?2, NULL)
             ON CONFLICT(name) DO UPDATE SET query = excluded.query, last_run_at = NULL",
            rusqlite::params![name, query],
        )?;
        Ok(())
    }

    pub fn get_saved_search(&self, name: &str) -> Result<Option<SavedSearch>> {
        self.conn
            .query_row(
                "SELECT name, query, last_run_at FROM saved_searches WHERE name = ?1",
                rusqlite::params![name],
                row_to_saved_search,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, query, last_run_at FROM saved_searches ORDER BY name")?;
        let rows = stmt
            .query_map([], row_to_saved_search)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Returns whether a saved search with this name existed and was removed.
    pub fn remove_saved_search(&mut self, name: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM saved_searches WHERE name = ?1", [name])?;
        Ok(n > 0)
    }

    pub fn touch_saved_search_last_run(&mut self, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE saved_searches SET last_run_at = datetime('now') WHERE name = ?1",
            [name],
        )?;
        Ok(())
    }
}

fn row_to_saved_search(row: &Row) -> rusqlite::Result<SavedSearch> {
    Ok(SavedSearch {
        name: row.get(0)?,
        query: row.get(1)?,
        last_run_at: row.get(2)?,
    })
}

fn in_clause(column: &str, start: usize, value_count: usize) -> String {
    format!("{column} IN ({})", placeholders(start, value_count))
}

fn placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn append_string_args(args: &mut Vec<Value>, values: &[String]) {
    args.extend(values.iter().cloned().map(Value::from));
}

fn year_ranges_clause(column: &str, ranges: &[YearRange], args: &mut Vec<Value>) -> String {
    let mut next = args.len() + 1;
    let clauses = ranges
        .iter()
        .map(|range| match range.bounds() {
            (Some(min), Some(max)) if min == max => {
                let placeholder = next;
                next += 1;
                args.push((min as i64).into());
                format!("{column} = ?{placeholder}")
            }
            (Some(min), Some(max)) => {
                let min_placeholder = next;
                let max_placeholder = next + 1;
                next += 2;
                args.push((min as i64).into());
                args.push((max as i64).into());
                format!("({column} >= ?{min_placeholder} AND {column} <= ?{max_placeholder})")
            }
            (Some(min), None) => {
                let placeholder = next;
                next += 1;
                args.push((min as i64).into());
                format!("{column} >= ?{placeholder}")
            }
            (None, Some(max)) => {
                let placeholder = next;
                next += 1;
                args.push((max as i64).into());
                format!("{column} <= ?{placeholder}")
            }
            (None, None) => unreachable!("year parser rejects empty ranges"),
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("({clauses})")
}

fn order_clause(q: &Search, args: &mut Vec<Value>) -> String {
    match &q.sort {
        Sort::Relevance if q.fts.is_some() => "ORDER BY bm25(papers_fts), p.year DESC".to_string(),
        Sort::Venue => "ORDER BY p.venue ASC, p.year DESC".to_string(),
        Sort::Rank(order) if order.iter().any(|venues| !venues.is_empty()) => {
            let rank_expr = rank_groups_clause("p.venue", args.len() + 1, order);
            for venues in order {
                append_string_args(args, venues);
            }
            format!("ORDER BY {rank_expr}, p.year DESC, p.venue ASC")
        }
        Sort::Relevance | Sort::Year | Sort::Rank(_) => {
            "ORDER BY p.year DESC, p.venue ASC".to_string()
        }
    }
}

fn rank_groups_clause(column: &str, start: usize, groups: &[Vec<String>]) -> String {
    let mut next = start;
    let mut clauses = Vec::new();
    for (rank_index, venues) in groups.iter().enumerate() {
        if venues.is_empty() {
            continue;
        }
        let clause = in_clause(column, next, venues.len());
        next += venues.len();
        clauses.push(format!("WHEN {clause} THEN {rank_index}"));
    }
    format!("CASE {} ELSE {} END", clauses.join(" "), groups.len())
}

struct SearchQueryParts {
    from: String,
    where_sql: String,
    args: Vec<Value>,
}

fn search_query_parts(q: &Search) -> SearchQueryParts {
    let mut args: Vec<Value> = Vec::new();
    let mut where_clauses: Vec<String> = Vec::new();

    let from = if let Some(fts) = &q.fts {
        args.push(fts.clone().into());
        where_clauses.push("papers_fts MATCH ?1".to_string());
        "papers_fts f JOIN papers p ON p.id = f.rowid".to_string()
    } else {
        "papers p".to_string()
    };

    if let Some(filter) = &q.filter {
        where_clauses.push(filter_clause(filter, &mut args));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    SearchQueryParts {
        from,
        where_sql,
        args,
    }
}

fn filter_clause(filter: &FilterExpr, args: &mut Vec<Value>) -> String {
    match filter {
        FilterExpr::Venue(venues) => {
            if venues.is_empty() {
                return "0".to_string();
            }
            let clause = in_clause("p.venue", args.len() + 1, venues.len());
            append_string_args(args, venues);
            clause
        }
        FilterExpr::Year(range) => year_ranges_clause("p.year", std::slice::from_ref(range), args),
        FilterExpr::Doi(term) => {
            let next = args.len() + 1;
            args.push(format!("%{}%", escape_like(term)).into());
            format!("(COALESCE(p.doi, '') LIKE ?{next} ESCAPE '\\')")
        }
        FilterExpr::AddedSince(date) => {
            let next = args.len() + 1;
            args.push(date.clone().into());
            format!("p.updated_at >= ?{next}")
        }
        FilterExpr::And(filters) => boolean_filter_clause("AND", filters, args),
        FilterExpr::Or(filters) => boolean_filter_clause("OR", filters, args),
        FilterExpr::Not(filter) => format!("NOT ({})", filter_clause(filter, args)),
    }
}

fn boolean_filter_clause(operator: &str, filters: &[FilterExpr], args: &mut Vec<Value>) -> String {
    let clauses = filters
        .iter()
        .map(|filter| filter_clause(filter, args))
        .collect::<Vec<_>>();
    format!("({})", clauses.join(&format!(" {operator} ")))
}

fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn missing_abstract_parts(
    venues: &[String],
    years: &[YearRange],
    after_id: Option<i64>,
) -> SearchQueryParts {
    let mut args: Vec<Value> = Vec::new();
    let mut where_clauses = vec![
        "p.url IS NOT NULL".to_string(),
        "(p.abstract IS NULL OR p.abstract = '')".to_string(),
    ];
    if !venues.is_empty() {
        where_clauses.push(in_clause("p.venue", args.len() + 1, venues.len()));
        append_string_args(&mut args, venues);
    }
    if !years.is_empty() {
        where_clauses.push(year_ranges_clause("p.year", years, &mut args));
    }
    if let Some(after_id) = after_id {
        let next = args.len() + 1;
        where_clauses.push(format!("p.id > ?{next}"));
        args.push(after_id.into());
    }

    SearchQueryParts {
        from: String::new(),
        where_sql: format!("WHERE {}", where_clauses.join(" AND ")),
        args,
    }
}

fn row_to_paper(row: &Row) -> rusqlite::Result<Paper> {
    Ok(Paper {
        key: row.get(0)?,
        source: row.get(1)?,
        venue: row.get(2)?,
        year: row.get(3)?,
        title: row.get(4)?,
        authors: row.get(5)?,
        doi: row.get(6)?,
        url: row.get(7)?,
        abstract_text: row.get(8)?,
    })
}

fn row_to_missing_paper(row: &Row) -> rusqlite::Result<MissingPaper> {
    Ok(MissingPaper {
        id: row.get(0)?,
        paper: Paper {
            key: row.get(1)?,
            source: row.get(2)?,
            venue: row.get(3)?,
            year: row.get(4)?,
            title: row.get(5)?,
            authors: row.get(6)?,
            doi: row.get(7)?,
            url: row.get(8)?,
            abstract_text: row.get(9)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(key: &str, venue: &str, year: i32, title: &str, abs: Option<&str>) -> Paper {
        Paper {
            key: key.into(),
            source: "dblp".into(),
            venue: venue.into(),
            year,
            title: title.into(),
            authors: "Alice Smith, Bob Jones".into(),
            doi: Some("10.1/x".into()),
            url: Some("https://example.com".into()),
            abstract_text: abs.map(|s| s.into()),
        }
    }

    fn seeded() -> Database {
        let mut db = Database::open_in_memory().unwrap();
        db.upsert_papers(&[
            paper(
                "k1",
                "NDSS",
                2020,
                "Fuzzing the Linux kernel",
                Some("we fuzz kernels"),
            ),
            paper("k2", "CCS", 2021, "Side channel attacks on caches", None),
            paper(
                "k3",
                "SP",
                2019,
                "Kernel exploitation techniques",
                Some("rop chains"),
            ),
        ])
        .unwrap();
        db
    }

    fn paper_by_key(db: &Database, key: &str) -> Paper {
        db.search(&Search::default())
            .unwrap()
            .into_iter()
            .find(|paper| paper.key == key)
            .unwrap()
    }

    #[test]
    fn upsert_is_idempotent_and_updates() {
        let mut db = seeded();
        db.upsert_papers(&[paper(
            "k1",
            "NDSS",
            2020,
            "Fuzzing the Linux kernel v2",
            None,
        )])
        .unwrap();
        assert_eq!(db.count().unwrap(), 3);
        let p = paper_by_key(&db, "k1");
        assert_eq!(p.title, "Fuzzing the Linux kernel v2");
        assert_eq!(p.abstract_text.as_deref(), Some("we fuzz kernels"));
    }

    #[test]
    fn upsert_skips_unchanged_rows() {
        let mut db = seeded();
        let rows = db
            .upsert_papers(&[paper("k1", "NDSS", 2020, "Fuzzing the Linux kernel", None)])
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn fts_search_matches_title_and_abstract() {
        let db = seeded();
        let hits = db
            .search(&Search {
                fts: Some("kernel".into()),
                ..Default::default()
            })
            .unwrap();
        let keys: Vec<_> = hits.iter().map(|p| p.key.as_str()).collect();
        assert!(keys.contains(&"k1"));
        assert!(keys.contains(&"k3"));
        assert!(!keys.contains(&"k2"));
    }

    #[test]
    fn metadata_filters() {
        let db = seeded();
        let by_venue = db
            .search(&Search {
                filter: Some(FilterExpr::Venue(vec!["NDSS".into(), "SP".into()])),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_venue.len(), 2);

        let by_year = db
            .search(&Search {
                filter: Some(FilterExpr::Year(YearRange::new(Some(2020), None).unwrap())),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_year.len(), 2);
    }

    #[test]
    fn added_since_filter() {
        let db = seeded();
        db.conn
            .execute(
                "UPDATE papers SET updated_at = ?1 WHERE paper_key = ?2",
                rusqlite::params!["2000-01-01 00:00:00", "k1"],
            )
            .unwrap();
        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::AddedSince("2020-01-01".into())),
                ..Default::default()
            })
            .unwrap();
        let keys = hits
            .iter()
            .map(|paper| paper.key.as_str())
            .collect::<Vec<_>>();
        assert!(!keys.contains(&"k1"));
        assert!(keys.contains(&"k2"));
        assert!(keys.contains(&"k3"));
    }

    #[test]
    fn repeated_year_filters_match_any_range() {
        let db = seeded();
        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::Or(vec![
                    FilterExpr::Year(YearRange::single(2019)),
                    FilterExpr::Year(YearRange::single(2021)),
                ])),
                ..Default::default()
            })
            .unwrap();
        let keys = hits
            .iter()
            .map(|paper| paper.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["k2", "k3"]);
    }

    #[test]
    fn boolean_metadata_filters() {
        let db = seeded();
        let intersection = db
            .search(&Search {
                filter: Some(FilterExpr::And(vec![
                    FilterExpr::Venue(vec!["NDSS".into(), "SP".into()]),
                    FilterExpr::Venue(vec!["SP".into(), "CCS".into()]),
                ])),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(intersection[0].key, "k3");

        let excluded = db
            .search(&Search {
                filter: Some(FilterExpr::Not(Box::new(FilterExpr::Venue(vec![
                    "CCS".into()
                ])))),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(excluded.len(), 2);
        assert!(excluded.iter().all(|paper| paper.venue != "CCS"));
    }

    #[test]
    fn rank_sort_groups_venues_before_year() {
        let db = seeded();
        let hits = db
            .search(&Search {
                sort: Sort::Rank(vec![vec!["NDSS".into(), "SP".into()], vec!["CCS".into()]]),
                ..Default::default()
            })
            .unwrap();
        let keys = hits
            .iter()
            .map(|paper| paper.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["k1", "k3", "k2"]);
    }

    #[test]
    fn rank_sort_without_ranked_venues_falls_back_to_year() {
        let db = seeded();
        let hits = db
            .search(&Search {
                sort: Sort::Rank(Vec::new()),
                ..Default::default()
            })
            .unwrap();
        let keys = hits
            .iter()
            .map(|paper| paper.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["k2", "k1", "k3"]);
    }

    #[test]
    fn doi_filter_matches_substrings() {
        let db = seeded();
        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::Doi("10.1/x".into())),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn negated_doi_filter_includes_missing_dois() {
        let mut db = seeded();
        let mut missing = paper("k4", "NDSS", 2022, "Paper without DOI", None);
        missing.doi = None;
        db.upsert_papers(&[missing]).unwrap();

        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::Not(Box::new(FilterExpr::Doi("10.1/x".into())))),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "k4");
    }

    #[test]
    fn search_count_and_offset() {
        let db = seeded();
        let search = Search {
            limit: Some(1),
            offset: Some(1),
            ..Default::default()
        };
        assert_eq!(db.search_count(&search).unwrap(), 3);
        let hits = db.search(&search).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "k1");
    }

    #[test]
    fn empty_venue_filter_matches_nothing() {
        let db = seeded();
        let search = Search {
            filter: Some(FilterExpr::Venue(Vec::new())),
            ..Default::default()
        };
        assert_eq!(db.search_count(&search).unwrap(), 0);
        assert!(db.search(&search).unwrap().is_empty());
    }

    #[test]
    fn missing_abstract_batch_uses_keyset_bound() {
        let db = seeded();
        assert_eq!(db.count_missing_abstracts(&[], &[]).unwrap(), 1);
        let missing = db.papers_missing_abstract_batch(&[], &[], 0, 10).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].paper.key, "k2");
        let next = db
            .papers_missing_abstract_batch(&[], &[], missing[0].id, 10)
            .unwrap();
        assert!(next.is_empty());
    }

    #[test]
    fn missing_abstract_batch_filters_years() {
        let db = seeded();
        assert_eq!(
            db.count_missing_abstracts(&[], &[YearRange::single(2020)])
                .unwrap(),
            0
        );
        assert_eq!(
            db.papers_missing_abstract_batch(&[], &[YearRange::single(2021)], 0, 10)
                .unwrap()[0]
                .paper
                .key,
            "k2"
        );
    }

    #[test]
    fn set_abstracts_updates_fts() {
        let mut db = seeded();
        db.set_abstracts(&[("k2".into(), "a batched cache timing leak".into())])
            .unwrap();
        let hits = db
            .search(&Search {
                fts: Some("batched".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "k2");
    }

    #[test]
    fn saved_search_round_trip() {
        let mut db = seeded();
        let config = crate::config::Config::defaults().unwrap();
        db.save_search("weekly-epi", "transmission WHERE venue:eid", &config)
            .unwrap();

        let saved = db.get_saved_search("weekly-epi").unwrap().unwrap();
        assert_eq!(saved.query, "transmission WHERE venue:eid");
        assert_eq!(saved.last_run_at, None);

        let all = db.list_saved_searches().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "weekly-epi");

        assert!(db.remove_saved_search("weekly-epi").unwrap());
        assert!(db.get_saved_search("weekly-epi").unwrap().is_none());
        assert!(!db.remove_saved_search("weekly-epi").unwrap());
    }

    #[test]
    fn saved_search_rejects_invalid_query() {
        let mut db = seeded();
        let config = crate::config::Config::defaults().unwrap();
        assert!(db
            .save_search("bad", "* WHERE venue:nope", &config)
            .is_err());
        assert!(db.get_saved_search("bad").unwrap().is_none());
    }

    #[test]
    fn saved_search_run_uses_last_run_at() {
        let mut db = seeded();
        for key in ["k1", "k2", "k3"] {
            db.conn
                .execute(
                    "UPDATE papers SET updated_at = ?1 WHERE paper_key = ?2",
                    rusqlite::params!["2000-01-01 00:00:00", key],
                )
                .unwrap();
        }
        let config = crate::config::Config::defaults().unwrap();
        db.save_search("recent", "* WHERE year:2019-", &config)
            .unwrap();
        db.touch_saved_search_last_run("recent").unwrap();
        let last_run_at = db
            .get_saved_search("recent")
            .unwrap()
            .unwrap()
            .last_run_at
            .unwrap();

        db.upsert_papers(&[paper("k4", "EID", 2023, "New reservoir finding", None)])
            .unwrap();

        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::AddedSince(last_run_at)),
                ..Default::default()
            })
            .unwrap();
        let keys = hits.iter().map(|p| p.key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys, vec!["k4"]);
    }

    #[test]
    fn saved_searches_survive_schema_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE papers (
                    id       INTEGER PRIMARY KEY,
                    dblp_key TEXT UNIQUE NOT NULL,
                    venue    TEXT NOT NULL,
                    year     INTEGER NOT NULL,
                    title    TEXT NOT NULL,
                    authors  TEXT NOT NULL,
                    doi TEXT, url TEXT, abstract TEXT
                );
                CREATE TABLE saved_searches (
                    id          INTEGER PRIMARY KEY,
                    name        TEXT UNIQUE NOT NULL,
                    query       TEXT NOT NULL,
                    last_run_at TEXT
                );
                INSERT INTO saved_searches (name, query, last_run_at)
                VALUES ('weekly', '* WHERE tag:epi', NULL);
                "#,
            )
            .unwrap();
        }

        // Opening through Database rebuilds the stale `papers`/`papers_fts`
        // schema (see `open_rebuilds_incompatible_old_schema` above), but
        // `saved_searches` must survive since it isn't a rebuildable cache.
        let db = Database::open(&path).unwrap();
        let saved = db.get_saved_search("weekly").unwrap();
        assert_eq!(saved.map(|s| s.query), Some("* WHERE tag:epi".to_string()));
    }

    #[test]
    fn set_abstracts_only_touches_updated_at_for_included_papers() {
        let mut db = seeded();
        db.conn
            .execute(
                "UPDATE papers SET updated_at = ?1",
                rusqlite::params!["2000-01-01 00:00:00"],
            )
            .unwrap();

        // Simulates `enrich`: only k2 is "found" this run; k1 and k3 are
        // skipped (e.g. rate limited) and must not have their updated_at
        // touched, or a subsequent added-since/saved-search filter would
        // wrongly treat them as new.
        db.set_abstracts(&[("k2".into(), "a batched cache timing leak".into())])
            .unwrap();

        let hits = db
            .search(&Search {
                filter: Some(FilterExpr::AddedSince("2020-01-01".into())),
                ..Default::default()
            })
            .unwrap();
        let keys = hits.iter().map(|p| p.key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys, vec!["k2"]);
    }

    #[test]
    fn open_rebuilds_incompatible_old_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            // A database written by the pre-pivot schema (dblp_key, no source,
            // user_version 0).
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE papers (
                    id       INTEGER PRIMARY KEY,
                    dblp_key TEXT UNIQUE NOT NULL,
                    venue    TEXT NOT NULL,
                    year     INTEGER NOT NULL,
                    title    TEXT NOT NULL,
                    authors  TEXT NOT NULL,
                    doi TEXT, url TEXT, abstract TEXT
                );
                INSERT INTO papers (dblp_key, venue, year, title, authors)
                VALUES ('legacy', 'NDSS', 2019, 'Legacy row', 'Old Author');
                "#,
            )
            .unwrap();
        }

        // Opening through Database detects the stale schema and rebuilds it.
        let mut db = Database::open(&path).unwrap();
        assert_eq!(db.count().unwrap(), 0);

        // The rebuilt database uses the current schema end-to-end.
        db.upsert_papers(&[paper("k1", "Epidemics", 2021, "Reservoir hosts", None)])
            .unwrap();
        let hits = db.search(&Search::default()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "k1");
        assert_eq!(hits[0].source, "dblp");
    }
}
