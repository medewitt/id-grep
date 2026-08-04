mod tui;

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use id_grep_core::abstracts::{EnrichResult, Enricher};
use id_grep_core::config::{Config, Paths, Secrets};
use id_grep_core::db::{Database, Search, Sort};
use id_grep_core::output::{self, Column, Format};
use id_grep_core::query;
use id_grep_core::sources::{dblp::Dblp, openalex::OpenAlex, pubmed::PubMed, Source};
use id_grep_core::zotero::{self, ZoteroLibrary};
use id_grep_core::Paper;

/// Upper bound for dblp year filters; papers never exceed this.
const MAX_YEAR: i32 = 2100;

#[derive(Parser)]
#[command(
    name = "id-grep",
    about = "Search infectious-disease ecology, evolution & epidemiology literature",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Search expression: text [WHERE metadata filters].
    #[arg(value_name = "QUERY")]
    query: Vec<String>,

    /// Result ordering (default: relevance).
    #[arg(long, value_enum)]
    sort: Option<SortMode>,

    /// Output format (default: table).
    #[arg(long)]
    format: Option<Format>,

    /// Limit number of results.
    #[arg(long)]
    limit: Option<usize>,

    /// Columns for table/csv output (comma-separated).
    #[arg(long, value_delimiter = ',')]
    fields: Vec<Column>,

    /// Launch the interactive TUI.
    #[arg(long)]
    tui: bool,

    /// Suppress progress/log output on stderr (results still print on stdout).
    #[arg(long, global = true)]
    quiet: bool,

    /// Drop results already in your Zotero library (uses --zotero or ~/Zotero).
    #[arg(long)]
    exclude_owned: bool,

    /// Annotate results with whether they're already in your Zotero library,
    /// without dropping any (uses --zotero or ~/Zotero).
    #[arg(long)]
    mark_owned: bool,

    /// Override database path.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Override user config.yaml path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Path to a Zotero data directory (the folder containing zotero.sqlite).
    #[arg(long, global = true, value_name = "DIR")]
    zotero: Option<PathBuf>,
}

impl Cli {
    fn has_search_args(&self) -> bool {
        !self.query.is_empty()
            || self.sort.is_some()
            || self.format.is_some()
            || self.limit.is_some()
            || !self.fields.is_empty()
            || self.tui
            || self.exclude_owned
            || self.mark_owned
    }
}

#[derive(Subcommand)]
enum Command {
    /// Create the data/config directories and an empty database.
    Init,
    /// Fetch paper metadata from the configured sources (incremental, idempotent).
    Update(UpdateArgs),
    /// Fill missing abstracts on the existing database (no re-fetch of metadata).
    Enrich(EnrichArgs),
    /// Manage saved searches (save/run/list/rm).
    Search(SearchArgs),
}

#[derive(clap::Args)]
struct SearchArgs {
    #[command(subcommand)]
    action: SearchAction,
}

#[derive(Subcommand)]
enum SearchAction {
    /// Save a named query for repeated `search run` invocations.
    Save(SaveArgs),
    /// Run a saved query, showing only rows added/changed since its last run.
    Run(RunArgs),
    /// List saved queries.
    List(ListArgs),
    /// Remove a saved query.
    Rm(RmArgs),
}

#[derive(clap::Args)]
struct SaveArgs {
    /// Name to save this query under.
    name: String,
    /// The query to save, e.g. 'transmission WHERE venue:Epidemics'.
    query: String,
}

#[derive(clap::Args)]
struct RunArgs {
    /// Name of the saved query to run.
    name: String,
    /// Preview matches without advancing the saved search's last-run marker.
    #[arg(long)]
    peek: bool,
    /// Output format (default: table).
    #[arg(long)]
    format: Option<Format>,
    /// Result ordering (default: relevance).
    #[arg(long, value_enum)]
    sort: Option<SortMode>,
    /// Limit number of results.
    #[arg(long)]
    limit: Option<usize>,
    /// Columns for table/csv output (comma-separated).
    #[arg(long, value_delimiter = ',')]
    fields: Vec<Column>,
}

#[derive(clap::Args)]
struct ListArgs {
    /// Output format (default: table).
    #[arg(long)]
    format: Option<Format>,
}

#[derive(clap::Args)]
struct RmArgs {
    /// Name of the saved query to remove.
    name: String,
}

/// Default number of concurrent abstract fetches.
const DEFAULT_JOBS: usize = 8;
const ENRICH_BATCH_SIZE: usize = 500;
const ENRICH_PROGRESS_INTERVAL: usize = 500;

#[derive(clap::Args)]
struct UpdateArgs {
    /// Only ingest from these venues (id or alias).
    #[arg(long, value_delimiter = ',')]
    venue: Vec<String>,
    /// Use these bundled venue sets for this update (overrides config bundles).
    #[arg(long, value_delimiter = ',')]
    bundle: Vec<String>,
    /// Minimum year (overrides config default).
    #[arg(long)]
    since: Option<i32>,
}

#[derive(clap::Args)]
struct EnrichArgs {
    /// Only enrich these venues (id or alias); default is all.
    #[arg(long, value_delimiter = ',')]
    venue: Vec<String>,
    /// Use these bundled venue sets for this enrich run (overrides config bundles).
    #[arg(long, value_delimiter = ',')]
    bundle: Vec<String>,
    /// Only enrich papers from this year onward.
    #[arg(long)]
    since: Option<i32>,
    /// Concurrent abstract fetches.
    #[arg(long, default_value_t = DEFAULT_JOBS)]
    jobs: usize,
    /// Stop after this many papers (useful for sampling / validation).
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SortMode {
    Relevance,
    Year,
    Venue,
    Rank,
}

/// Suppresses stderr progress/log output when set (via `--quiet`).
static QUIET: AtomicBool = AtomicBool::new(false);

fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Outcome of a command, mapped to a process exit code.
enum ExitStatus {
    /// Completed normally.
    Ok,
    /// A search returned no matching papers (exit code 3).
    NoResults,
}

/// Distinct exit codes so a calling agent can branch on the outcome.
mod exit {
    pub const NO_RESULTS: u8 = 3;
    pub const CONFIG: u8 = 4; // bad config / bad usage / not found
    pub const SOURCE: u8 = 5; // network / upstream source failure
    pub const GENERIC: u8 = 1;
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    QUIET.store(cli.quiet, Ordering::Relaxed);
    let format = cli.format.unwrap_or(Format::Table);

    match run(cli).await {
        Ok(ExitStatus::Ok) => ExitCode::SUCCESS,
        Ok(ExitStatus::NoResults) => ExitCode::from(exit::NO_RESULTS),
        Err(e) => {
            report_error(&e, format);
            ExitCode::from(exit_code_for(&e))
        }
    }
}

async fn run(cli: Cli) -> Result<ExitStatus> {
    reject_search_args_for_subcommands(&cli)?;
    let paths = Paths::resolve()?;
    let bundle_override = match &cli.command {
        Some(Command::Update(args)) if !args.bundle.is_empty() => Some(args.bundle.as_slice()),
        Some(Command::Enrich(args)) if !args.bundle.is_empty() => Some(args.bundle.as_slice()),
        _ => None,
    };
    let config = load_config_with_bundles(&cli, &paths, bundle_override)?;

    match &cli.command {
        Some(Command::Init) => cmd_init(&cli, &paths).map(|()| ExitStatus::Ok),
        Some(Command::Update(args)) => cmd_update(args, &cli, &paths, &config)
            .await
            .map(|()| ExitStatus::Ok),
        Some(Command::Enrich(args)) => cmd_enrich(args, &cli, &paths, &config)
            .await
            .map(|()| ExitStatus::Ok),
        Some(Command::Search(args)) => cmd_search_action(args, &cli, &paths, &config),
        None if cli.tui => {
            let db = open_db(&cli, &paths)?;
            tui::run(db, config).map(|()| ExitStatus::Ok)
        }
        None => cmd_search(&cli, &paths, &config),
    }
}

/// Map an error to a distinct exit code by inspecting the core error kind
/// anywhere in the chain.
fn exit_code_for(err: &anyhow::Error) -> u8 {
    use id_grep_core::Error;
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<Error>() {
            return match e {
                Error::Config(_) | Error::Query(_) => exit::CONFIG,
                Error::Http(_) => exit::SOURCE,
                _ => exit::GENERIC,
            };
        }
    }
    exit::GENERIC
}

/// Report an error: a JSON object on stderr under `--format json`, otherwise
/// the human-readable error chain.
fn report_error(err: &anyhow::Error, format: Format) {
    if matches!(format, Format::Json) {
        let payload = serde_json::json!({
            "schema_version": output::SCHEMA_VERSION,
            "error": format!("{err:#}"),
        });
        eprintln!("{payload}");
    } else {
        eprintln!("error: {err:#}");
    }
}

fn reject_search_args_for_subcommands(cli: &Cli) -> Result<()> {
    if cli.command.is_some() && cli.has_search_args() {
        anyhow::bail!(
            "search query/options cannot be used with subcommands; put command-specific options after the subcommand"
        );
    }
    Ok(())
}

fn log_header(title: &str) {
    if quiet() {
        return;
    }
    eprintln!("{title}");
}

fn log_field(label: &str, value: impl std::fmt::Display) {
    if quiet() {
        return;
    }
    eprintln!("  {label:<10} {value}");
}

fn log_blank() {
    if quiet() {
        return;
    }
    eprintln!();
}

fn load_config_with_bundles(
    cli: &Cli,
    paths: &Paths,
    bundle_override: Option<&[String]>,
) -> Result<Config> {
    let user_path = config_path(cli, paths);
    Config::load_with_bundles(Some(&user_path), bundle_override).context("loading venue config")
}

fn config_path(cli: &Cli, paths: &Paths) -> PathBuf {
    cli.config
        .clone()
        .unwrap_or_else(|| paths.user_config_path())
}

fn db_path(cli: &Cli, paths: &Paths) -> PathBuf {
    cli.db.clone().unwrap_or_else(|| paths.db_path())
}

fn open_db(cli: &Cli, paths: &Paths) -> Result<Database> {
    let path = db_path(cli, paths);
    Database::open_existing(&path).with_context(|| {
        format!(
            "no database at {}; run `id-grep init` then `id-grep update`",
            path.display()
        )
    })
}

fn cmd_init(cli: &Cli, paths: &Paths) -> Result<()> {
    paths.ensure_dirs()?;
    let path = db_path(cli, paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Database::open(&path).context("creating database")?;
    let config_path = config_path(cli, paths);
    write_default_config(&config_path)?;
    log_header("id-grep initialized");
    log_field("database", path.display());
    log_field("config", config_path.display());
    log_blank();
    log_field("next", "`id-grep update`");
    Ok(())
}

fn write_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, Config::default_user_config_yaml()?)?;
    Ok(())
}

fn cmd_search(cli: &Cli, paths: &Paths, config: &Config) -> Result<ExitStatus> {
    let db = open_db(cli, paths)?;

    let raw = cli.query.join(" ");
    let search = build_search(
        &raw,
        config,
        cli.sort.unwrap_or(SortMode::Relevance),
        cli.limit,
        None,
    )?;
    let papers = db.search(&search)?;

    // Consult the Zotero library once and reuse the same per-paper owned
    // flags for both dropping (--exclude-owned) and annotating (--mark-owned
    // and the default owned column/field) -- one source of truth for
    // "already in Zotero" either way.
    let owned = if cli.exclude_owned || cli.mark_owned {
        let library = open_zotero_library(cli)?;
        Some(
            papers
                .iter()
                .map(|paper| library.is_owned(paper))
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    let (papers, owned) = if cli.exclude_owned {
        let owned = owned.expect("computed above when exclude_owned is set");
        let (papers, owned): (Vec<_>, Vec<_>) = papers
            .into_iter()
            .zip(owned)
            .filter(|(_, is_owned)| !is_owned)
            .unzip();
        (papers, Some(owned))
    } else {
        (papers, owned)
    };

    let columns = (!cli.fields.is_empty()).then_some(cli.fields.as_slice());
    let format = cli.format.unwrap_or(Format::Table);
    let out = output::render(&papers, format, columns, owned.as_deref())
        .map_err(|e| anyhow::anyhow!(e))?;
    if !out.is_empty() {
        print!("{out}");
        if !out.ends_with('\n') {
            println!();
        }
    }
    if matches!(format, Format::Table) && !quiet() {
        eprintln!("results    {}", papers.len());
    }
    Ok(if papers.is_empty() {
        ExitStatus::NoResults
    } else {
        ExitStatus::Ok
    })
}

fn open_zotero_library(cli: &Cli) -> Result<ZoteroLibrary> {
    let dir = match &cli.zotero {
        Some(dir) => dir.clone(),
        None => zotero::default_data_dir().context(
            "no Zotero library found in ~/Zotero; pass its location with --zotero <DIR>",
        )?,
    };
    ZoteroLibrary::open(&dir)
        .with_context(|| format!("reading Zotero library at {}", dir.display()))
}

pub(crate) fn build_search(
    raw_query: &str,
    config: &Config,
    sort: SortMode,
    limit: Option<usize>,
    offset: Option<usize>,
) -> id_grep_core::Result<Search> {
    let parsed = query::parse(raw_query, config)?;
    let sort = match sort {
        SortMode::Relevance => Sort::Relevance,
        SortMode::Year => Sort::Year,
        SortMode::Venue => Sort::Venue,
        SortMode::Rank => Sort::Rank(config.rank_sort_order()),
    };

    Ok(Search {
        fts: parsed.fts,
        filter: parsed.filter,
        sort,
        limit,
        offset,
    })
}

fn not_found(name: &str) -> anyhow::Error {
    id_grep_core::Error::Config(format!("no saved search named `{name}`")).into()
}

fn cmd_search_action(
    args: &SearchArgs,
    cli: &Cli,
    paths: &Paths,
    config: &Config,
) -> Result<ExitStatus> {
    let mut db = open_db(cli, paths)?;
    match &args.action {
        SearchAction::Save(args) => cmd_search_save(args, &mut db, config),
        SearchAction::Run(args) => cmd_search_run(args, &mut db, config),
        SearchAction::List(args) => cmd_search_list(args, &db),
        SearchAction::Rm(args) => cmd_search_rm(args, &mut db),
    }
}

fn cmd_search_save(args: &SaveArgs, db: &mut Database, config: &Config) -> Result<ExitStatus> {
    db.save_search(&args.name, &args.query, config)?;
    log_header("id-grep search save");
    log_field("name", &args.name);
    log_field("query", &args.query);
    Ok(ExitStatus::Ok)
}

fn cmd_search_run(args: &RunArgs, db: &mut Database, config: &Config) -> Result<ExitStatus> {
    let saved = db
        .get_saved_search(&args.name)?
        .ok_or_else(|| not_found(&args.name))?;

    let mut search = build_search(
        &saved.query,
        config,
        args.sort.unwrap_or(SortMode::Relevance),
        args.limit,
        None,
    )?;
    if let Some(last_run_at) = &saved.last_run_at {
        let added_since = query::FilterExpr::AddedSince(last_run_at.clone());
        search.filter = Some(match search.filter.take() {
            Some(existing) => query::FilterExpr::And(vec![existing, added_since]),
            None => added_since,
        });
    }

    let papers = db.search(&search)?;
    let columns = (!args.fields.is_empty()).then_some(args.fields.as_slice());
    let format = args.format.unwrap_or(Format::Table);
    let out = output::render(&papers, format, columns, None).map_err(|e| anyhow::anyhow!(e))?;
    if !out.is_empty() {
        print!("{out}");
        if !out.ends_with('\n') {
            println!();
        }
    }
    if matches!(format, Format::Table) && !quiet() {
        eprintln!("results    {}", papers.len());
    }

    if !args.peek {
        db.touch_saved_search_last_run(&args.name)?;
    }

    Ok(if papers.is_empty() {
        ExitStatus::NoResults
    } else {
        ExitStatus::Ok
    })
}

fn cmd_search_list(args: &ListArgs, db: &Database) -> Result<ExitStatus> {
    let searches = db.list_saved_searches()?;
    let format = args.format.unwrap_or(Format::Table);
    if matches!(format, Format::Json) {
        let payload = serde_json::json!({
            "schema_version": output::SCHEMA_VERSION,
            "count": searches.len(),
            "saved_searches": searches.iter().map(|s| serde_json::json!({
                "name": s.name,
                "query": s.query,
                "last_run_at": s.last_run_at,
            })).collect::<Vec<_>>(),
        });
        println!("{payload}");
    } else if searches.is_empty() {
        if !quiet() {
            eprintln!("no saved searches");
        }
    } else {
        for s in &searches {
            println!(
                "{:<20} {:<50} {}",
                s.name,
                s.query,
                s.last_run_at.as_deref().unwrap_or("never")
            );
        }
    }
    Ok(ExitStatus::Ok)
}

fn cmd_search_rm(args: &RmArgs, db: &mut Database) -> Result<ExitStatus> {
    if !db.remove_saved_search(&args.name)? {
        return Err(not_found(&args.name));
    }
    log_header("id-grep search rm");
    log_field("name", &args.name);
    Ok(ExitStatus::Ok)
}

async fn cmd_update(args: &UpdateArgs, cli: &Cli, paths: &Paths, config: &Config) -> Result<()> {
    paths.ensure_dirs()?;
    let path = db_path(cli, paths);
    let mut db = Database::open(&path).context("opening database")?;

    let venue_ids = if args.venue.is_empty() {
        config.all_venue_ids()
    } else {
        config.resolve_venues(&args.venue)?
    };
    let min_year = args.since.unwrap_or(config.defaults.min_year);

    log_header("id-grep update");
    log_field("bundles", config.bundles.join(", "));
    log_field("venues", venue_ids.len());
    log_field("since", min_year);
    log_blank();

    let secrets = Secrets::load();
    let openalex = OpenAlex::new(&secrets);
    let pubmed = PubMed::new(&secrets);
    let dblp = Dblp::default();
    let mut total = 0usize;
    let mut failed = Vec::new();
    for id in &venue_ids {
        let venue = config.venue(id).expect("resolved venue");
        if !quiet() {
            eprint!("  {id:<12} ");
            let _ = std::io::stderr().flush();
        }
        // Prefer OpenAlex when the venue carries an OpenAlex id or ISSN; else
        // PubMed when it declares an NLM journal abbreviation; else fall back to
        // DBLP for CS venues that only have a dblp_stream.
        let result = if venue.openalex_source_id.is_some() || !venue.issn.is_empty() {
            openalex.fetch_venue(venue, min_year, MAX_YEAR).await
        } else if venue.pubmed_journal.is_some() {
            pubmed.fetch_venue(venue, min_year, MAX_YEAR).await
        } else {
            dblp.fetch_venue(venue, min_year, MAX_YEAR).await
        };
        match result {
            Ok(papers) => {
                let n = db.upsert_papers(&papers)?;
                total += papers.len();
                if !quiet() {
                    eprintln!("fetched {:>5} papers, {:>5} upserted", papers.len(), n);
                }
            }
            Err(e) => {
                if !quiet() {
                    eprintln!("failed   {e}");
                }
                failed.push(id.clone());
            }
        }
    }
    log_blank();
    log_header("summary");
    log_field("fetched", format_args!("{total} papers"));
    log_field("failed", failed.len());
    log_field("database", format_args!("{} papers", db.count()?));

    if !failed.is_empty() {
        anyhow::bail!("failed to fetch venue(s): {}", failed.join(", "));
    }

    Ok(())
}

async fn cmd_enrich(args: &EnrichArgs, cli: &Cli, paths: &Paths, config: &Config) -> Result<()> {
    let mut db = open_db(cli, paths)?;
    let venue_ids = if args.venue.is_empty() {
        if args.bundle.is_empty() {
            Vec::new()
        } else {
            config.all_venue_ids()
        }
    } else {
        config.resolve_venues(&args.venue)?
    };
    let years = match args.since {
        Some(year) => vec![query::YearRange::new(Some(year), None)?],
        None => Vec::new(),
    };
    enrich_abstracts(&mut db, &venue_ids, &years, args.jobs, args.limit).await
}

/// Fill missing abstracts, running up to `jobs` fetches concurrently.
/// `venue_ids` empty means all venues; `limit` caps how many are attempted.
async fn enrich_abstracts(
    db: &mut Database,
    venue_ids: &[String],
    years: &[query::YearRange],
    jobs: usize,
    limit: Option<usize>,
) -> Result<()> {
    let mut enricher = Enricher::new(Secrets::load());
    let pending = db.count_missing_abstracts(venue_ids, years)?;
    let total = limit.map_or(pending, |limit| limit.min(pending));
    let jobs = jobs.max(1);
    log_header("abstract enrichment");
    log_field("pending", format_args!("{pending} abstracts"));
    if !years.is_empty() {
        log_field("years", format_args!("{}", format_year_ranges(years)));
    }
    if total != pending {
        log_field("selected", format_args!("{total} abstracts"));
    }
    log_field("jobs", jobs);
    log_blank();

    let mut filled = 0usize;
    let mut processed = 0usize;
    let mut misses: BTreeMap<String, usize> = BTreeMap::new();
    let mut after_id = 0;
    while processed < total {
        let remaining = total - processed;
        let batch = db.papers_missing_abstract_batch(
            venue_ids,
            years,
            after_id,
            remaining.min(ENRICH_BATCH_SIZE),
        )?;
        let Some(next_after_id) = batch.last().map(|paper| paper.id) else {
            break;
        };
        after_id = next_after_id;

        let papers = batch
            .into_iter()
            .map(|missing| missing.paper)
            .collect::<Vec<_>>();

        let batch_len = papers.len();
        log_field(
            "batch",
            format_args!(
                "{}-{} / {} ({})",
                processed + 1,
                processed + batch_len,
                total,
                batch_venue_summary(&papers)
            ),
        );
        let results = enricher.enrich_many(papers, jobs).await;

        let mut abstract_updates = Vec::new();
        for (paper, res) in results {
            processed += 1;
            let key = paper.key;
            match res {
                Ok(EnrichResult::Found(abs)) => {
                    abstract_updates.push((key, abs));
                    filled += 1;
                }
                Ok(EnrichResult::Missing(reason)) => {
                    *misses.entry(reason).or_default() += 1;
                }
                Err(e) => eprintln!("warning: abstract fetch failed for {key}: {e}"),
            }
            if processed.is_multiple_of(ENRICH_PROGRESS_INTERVAL) {
                log_field(
                    "progress",
                    format_args!("{processed}/{total} processed, {filled} filled"),
                );
            }
        }
        log_field(
            "batch done",
            format_args!(
                "{} filled, {} missed",
                abstract_updates.len(),
                batch_len - abstract_updates.len()
            ),
        );
        db.set_abstracts(&abstract_updates)?;
    }
    log_blank();
    log_header("summary");
    log_field("filled", format_args!("{filled}/{processed} abstracts"));
    if !misses.is_empty() {
        log_field("missed", processed - filled);
        for (reason, count) in misses {
            eprintln!("  {count:>10} {reason}");
        }
    }
    Ok(())
}

fn format_year_ranges(years: &[query::YearRange]) -> String {
    years
        .iter()
        .map(|year| match year.bounds() {
            (Some(min), Some(max)) if min == max => min.to_string(),
            (Some(min), Some(max)) => format!("{min}-{max}"),
            (Some(min), None) => format!("{min}-"),
            (None, Some(max)) => format!("-{max}"),
            (None, None) => String::new(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn batch_venue_summary(inputs: &[Paper]) -> String {
    let mut counts = BTreeMap::new();
    for paper in inputs {
        *counts.entry(paper.venue.as_str()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(venue, count)| format!("{venue}:{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("id-grep-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn build_search_maps_query_and_options() {
        let config = Config::defaults().unwrap();
        let search = build_search(
            "malaria WHERE tag:epi AND year:2020-",
            &config,
            SortMode::Year,
            Some(10),
            Some(5),
        )
        .unwrap();

        assert!(search.fts.is_some());
        assert!(matches!(search.filter, Some(query::FilterExpr::And(_))));
        assert!(matches!(search.sort, Sort::Year));
        assert_eq!(search.limit, Some(10));
        assert_eq!(search.offset, Some(5));
    }

    #[test]
    fn enrich_options_are_parsed() {
        let cli = Cli::try_parse_from([
            "id-grep",
            "enrich",
            "--bundle",
            "epi,modelling",
            "--venue",
            "jid,eid",
            "--since",
            "2025",
            "--jobs",
            "4",
            "--limit",
            "10",
        ])
        .unwrap();
        let Some(Command::Enrich(args)) = cli.command else {
            panic!("expected enrich command");
        };
        assert_eq!(
            args.bundle,
            vec!["epi".to_string(), "modelling".to_string()]
        );
        assert_eq!(args.venue, vec!["jid".to_string(), "eid".to_string()]);
        assert_eq!(args.since, Some(2025));
        assert_eq!(args.jobs, 4);
        assert_eq!(args.limit, Some(10));
    }

    #[test]
    fn search_save_args_are_parsed() {
        let cli = Cli::try_parse_from([
            "id-grep",
            "search",
            "save",
            "weekly-epi",
            "transmission WHERE venue:Epidemics",
        ])
        .unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchAction::Save(save_args) = args.action else {
            panic!("expected save action");
        };
        assert_eq!(save_args.name, "weekly-epi");
        assert_eq!(save_args.query, "transmission WHERE venue:Epidemics");
    }

    #[test]
    fn search_run_args_are_parsed() {
        let cli = Cli::try_parse_from([
            "id-grep",
            "search",
            "run",
            "weekly-epi",
            "--peek",
            "--format",
            "json",
        ])
        .unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchAction::Run(run_args) = args.action else {
            panic!("expected run action");
        };
        assert_eq!(run_args.name, "weekly-epi");
        assert!(run_args.peek);
        assert_eq!(run_args.format, Some(Format::Json));
    }

    #[test]
    fn search_list_args_are_parsed() {
        let cli = Cli::try_parse_from(["id-grep", "search", "list", "--format", "json"]).unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchAction::List(list_args) = args.action else {
            panic!("expected list action");
        };
        assert_eq!(list_args.format, Some(Format::Json));
    }

    #[test]
    fn search_rm_args_are_parsed() {
        let cli = Cli::try_parse_from(["id-grep", "search", "rm", "weekly-epi"]).unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchAction::Rm(rm_args) = args.action else {
            panic!("expected rm action");
        };
        assert_eq!(rm_args.name, "weekly-epi");
    }

    #[test]
    fn search_output_options_are_parsed_and_rejected_with_commands() {
        let cli =
            Cli::try_parse_from(["id-grep", "--format", "json", "--fields", "year,title"]).unwrap();
        assert_eq!(cli.format, Some(Format::Json));
        assert_eq!(cli.fields, vec![Column::Year, Column::Title]);

        let cli = Cli::try_parse_from(["id-grep", "--format", "json", "update"]).unwrap();
        assert!(reject_search_args_for_subcommands(&cli).is_err());

        let cli = Cli::try_parse_from(["id-grep", "--db", "papers.db", "update"]).unwrap();
        assert!(reject_search_args_for_subcommands(&cli).is_ok());
    }

    #[test]
    fn bundle_override_limits_venue_resolution() {
        let cli = Cli::try_parse_from([
            "id-grep",
            "update",
            "--bundle",
            "epi,modelling",
            "--venue",
            "nope",
        ])
        .unwrap();
        let paths = Paths {
            data_dir: temp_test_path("data"),
            config_dir: temp_test_path("config"),
        };
        let Some(Command::Update(args)) = &cli.command else {
            panic!("expected update command");
        };
        let config = load_config_with_bundles(&cli, &paths, Some(&args.bundle)).unwrap();
        assert!(config.venue("JID").is_some());
        assert!(config.venue("Epidemics").is_some());
        assert!(config.resolve_venues(&args.venue).is_err());
    }

    #[test]
    fn write_default_config_creates_but_does_not_overwrite() {
        let path = temp_test_path("config.yaml");
        let _ = std::fs::remove_file(&path);
        write_default_config(&path).unwrap();
        let default = std::fs::read_to_string(&path).unwrap();
        assert!(default.contains("epi"));
        assert!(default.contains("min_year: 2000"));

        std::fs::write(&path, "bundles: []\n").unwrap();
        write_default_config(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bundles: []\n");
        let _ = std::fs::remove_file(path);
    }
}
