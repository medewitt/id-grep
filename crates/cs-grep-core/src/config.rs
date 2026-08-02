//! Venue catalog, secrets, and filesystem paths.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const DEFAULT_BUNDLES: &[&str] = &["epi", "modelling", "ecoevo"];
const BUNDLED_VENUES: &[(&str, &str)] = &[
    ("epi", include_str!("../venues/epi.yaml")),
    ("modelling", include_str!("../venues/modelling.yaml")),
    ("ecoevo", include_str!("../venues/ecoevo.yaml")),
    ("preprints", include_str!("../venues/preprints.yaml")),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Venue {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// DBLP RDF stream id (e.g. `conf/ndss`), used by the DBLP source only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dblp_stream: Option<String>,
    /// ISSN(s) identifying this venue for OpenAlex ingestion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issn: Vec<String>,
    /// OpenAlex source id (e.g. `S123456789`), preferred over ISSN when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openalex_source_id: Option<String>,
    /// NLM title abbreviation used by the PubMed source (e.g. `Emerg Infect Dis`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pubmed_journal: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub rank: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Venue {
    /// Case-insensitive match against the venue id or any alias.
    pub fn matches(&self, needle: &str) -> bool {
        self.id.eq_ignore_ascii_case(needle)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(needle))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Defaults {
    #[serde(default = "default_min_year")]
    pub min_year: i32,
}

fn default_min_year() -> i32 {
    2000
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            min_year: default_min_year(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "default_bundles")]
    pub bundles: Vec<String>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub venues: Vec<Venue>,
}

pub type RankSortOrder = Vec<Vec<String>>;

#[derive(Debug, Serialize, Deserialize, Default)]
struct ConfigFile {
    bundles: Option<Vec<String>>,
    defaults: Option<Defaults>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    venues: Vec<Venue>,
}

impl ConfigFile {
    fn from_yaml(yaml: &str) -> Result<Self> {
        Ok(serde_yaml::from_str(yaml)?)
    }
}

#[derive(Debug, Deserialize)]
struct BundleFile {
    #[serde(default)]
    venues: Vec<Venue>,
}

fn default_bundles() -> Vec<String> {
    DEFAULT_BUNDLES
        .iter()
        .map(|bundle| (*bundle).to_string())
        .collect()
}

impl Config {
    /// Load the default bundle set.
    pub fn defaults() -> Result<Self> {
        Self::from_file(ConfigFile::default(), None)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        Self::from_file(ConfigFile::from_yaml(yaml)?, None)
    }

    pub fn default_user_config_yaml() -> Result<String> {
        Ok(serde_yaml::to_string(&ConfigFile {
            bundles: Some(default_bundles()),
            defaults: Some(Defaults::default()),
            venues: Vec::new(),
        })?)
    }

    pub fn load_with_bundles(
        user_override: Option<&Path>,
        bundle_override: Option<&[String]>,
    ) -> Result<Self> {
        let file = match user_override {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => ConfigFile::from_yaml(&text)?,
                Err(e) if e.kind() == ErrorKind::NotFound => ConfigFile::default(),
                Err(e) => return Err(e.into()),
            },
            None => ConfigFile::default(),
        };
        Self::from_file(file, bundle_override)
    }

    fn from_file(file: ConfigFile, bundle_override: Option<&[String]>) -> Result<Self> {
        let bundles = bundle_override
            .map(|bundles| bundles.to_vec())
            .or(file.bundles)
            .unwrap_or_else(default_bundles);
        let mut cfg = Self {
            bundles: bundles.clone(),
            defaults: file.defaults.unwrap_or_default(),
            venues: Vec::new(),
        };
        for bundle in &bundles {
            cfg.merge_bundle(bundle)?;
        }
        cfg.merge_venues(file.venues);
        Ok(cfg)
    }

    fn merge_bundle(&mut self, bundle: &str) -> Result<()> {
        let Some((_, yaml)) = BUNDLED_VENUES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(bundle))
        else {
            return Err(Error::Config(format!("unknown bundle: {bundle}")));
        };
        let bundle: BundleFile = serde_yaml::from_str(yaml)?;
        self.merge_venues(bundle.venues);
        Ok(())
    }

    fn merge_venues(&mut self, venues: Vec<Venue>) {
        for venue in venues {
            self.upsert_venue(venue);
        }
    }

    fn upsert_venue(&mut self, venue: Venue) {
        match self
            .venues
            .iter_mut()
            .find(|v| v.id.eq_ignore_ascii_case(&venue.id))
        {
            Some(existing) => *existing = venue,
            None => self.venues.push(venue),
        }
    }

    /// Resolve a venue by id or alias.
    pub fn venue(&self, needle: &str) -> Option<&Venue> {
        self.venues.iter().find(|v| v.matches(needle))
    }

    /// Resolve venue selectors (id or alias) to canonical ids.
    /// Unknown selectors produce an error listing them.
    pub fn resolve_venues(&self, selectors: &[String]) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut unknown = Vec::new();
        for sel in selectors {
            match self.venue(sel) {
                Some(v) => {
                    if seen.insert(v.id.clone()) {
                        ids.push(v.id.clone());
                    }
                }
                None => unknown.push(sel.clone()),
            }
        }
        if !unknown.is_empty() {
            return Err(Error::Config(format!(
                "unknown venue(s): {}",
                unknown.join(", ")
            )));
        }
        Ok(ids)
    }

    /// All configured venue ids in catalog order.
    pub fn all_venue_ids(&self) -> Vec<String> {
        self.venues.iter().map(|v| v.id.clone()).collect()
    }

    /// Venue ids matching the given rank labels (case-insensitive).
    pub fn venues_by_rank(&self, ranks: &[String]) -> Vec<String> {
        if ranks.is_empty() {
            return Vec::new();
        }
        self.venues
            .iter()
            .filter(|v| {
                venue_rank(v).is_some_and(|r| ranks.iter().any(|q| q.eq_ignore_ascii_case(r)))
            })
            .map(|v| v.id.clone())
            .collect()
    }

    /// Venue ids carrying any of the given tags (case-insensitive).
    pub fn venues_by_tag(&self, tags: &[String]) -> Vec<String> {
        if tags.is_empty() {
            return Vec::new();
        }
        self.venues
            .iter()
            .filter(|v| {
                v.tags
                    .iter()
                    .any(|t| tags.iter().any(|q| q.eq_ignore_ascii_case(t)))
            })
            .map(|v| v.id.clone())
            .collect()
    }

    /// Venue ids grouped by rank, ordered for rank-based result sorting.
    pub fn rank_sort_order(&self) -> RankSortOrder {
        struct RankGroup {
            label: String,
            sort_key: u8,
        }

        let mut groups: Vec<RankGroup> = Vec::new();
        for venue in &self.venues {
            let Some(rank) = venue_rank(venue).map(str::to_ascii_uppercase) else {
                continue;
            };
            if groups.iter().any(|group| group.label == rank) {
                continue;
            }
            groups.push(RankGroup {
                sort_key: rank_sort_key(&rank),
                label: rank,
            });
        }
        groups.sort_by_key(|group| group.sort_key);
        groups
            .into_iter()
            .map(|group| self.venues_by_rank(std::slice::from_ref(&group.label)))
            .collect()
    }
}

fn venue_rank(venue: &Venue) -> Option<&str> {
    venue.rank.as_deref().filter(|rank| !rank.is_empty())
}

fn rank_sort_key(rank: &str) -> u8 {
    match rank.to_ascii_uppercase().as_str() {
        "A*" => 0,
        "A" => 1,
        "B" => 2,
        "C" => 3,
        _ => 4,
    }
}

/// API keys, read from the environment (optionally seeded from a `.env` file).
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    pub openalex_api_key: Option<String>,
    /// Contact email for OpenAlex's polite pool (faster, more reliable).
    pub openalex_mailto: Option<String>,
    pub semantic_scholar_key: Option<String>,
    pub openreview_username: Option<String>,
    pub openreview_password: Option<String>,
    /// NCBI E-utilities API key (raises the PubMed rate limit to 10 req/s).
    pub ncbi_api_key: Option<String>,
    /// Contact email sent with NCBI E-utilities requests.
    pub ncbi_email: Option<String>,
}

impl Secrets {
    /// Best-effort load: source `.env` if present, then read known vars.
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();
        Self {
            openalex_api_key: non_empty_env("OPENALEX_API_KEY"),
            openalex_mailto: non_empty_env("OPENALEX_MAILTO"),
            semantic_scholar_key: non_empty_env("SEMANTIC_SCHOLAR_S2_KEY"),
            openreview_username: non_empty_env("OPENREVIEW_USERNAME"),
            openreview_password: non_empty_env("OPENREVIEW_PASSWORD"),
            ncbi_api_key: non_empty_env("NCBI_API_KEY"),
            ncbi_email: non_empty_env("NCBI_EMAIL"),
        }
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Resolved on-disk locations for the database and user config.
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", "cs-grep")
            .ok_or_else(|| Error::Config("cannot determine home directory".into()))?;
        Ok(Self {
            data_dir: dirs.data_dir().to_path_buf(),
            config_dir: dirs.config_dir().to_path_buf(),
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("papers.db")
    }

    pub fn user_config_path(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.config_dir)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venue(id: &str, rank: &str, tags: &[&str]) -> Venue {
        Venue {
            id: id.to_string(),
            name: String::new(),
            dblp_stream: Some(format!("conf/{}", id.to_ascii_lowercase())),
            issn: Vec::new(),
            openalex_source_id: None,
            pubmed_journal: None,
            aliases: Vec::new(),
            rank: (!rank.is_empty()).then(|| rank.to_string()),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
        }
    }

    #[test]
    fn bundled_venue_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (bundle, yaml) in BUNDLED_VENUES {
            let bundle_file: BundleFile = serde_yaml::from_str(yaml).unwrap();
            for venue in bundle_file.venues {
                assert!(
                    seen.insert(venue.id.to_ascii_lowercase()),
                    "duplicate bundled venue id {} in {bundle}",
                    venue.id
                );
            }
        }
    }

    #[test]
    fn bundled_tags_match_venue_families() {
        let cfg = Config::defaults().unwrap();

        // Default catalog tags (the preprints bundle is opt-in, not loaded here).
        let actual_tags = cfg
            .venues
            .iter()
            .flat_map(|venue| venue.tags.iter().map(String::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            actual_tags,
            ["ecology", "epi", "evolution", "modelling"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );

        // Spot-check a representative venue per tag.
        let by_tag = |tag: &str| {
            cfg.venues_by_tag(&[tag.into()])
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert!(by_tag("epi").contains("JID"));
        assert!(by_tag("epi").contains("EID"));
        assert!(by_tag("modelling").contains("Epidemics"));
        assert!(by_tag("modelling").contains("PLoS-Comp-Biol"));
        assert!(by_tag("ecology").contains("Ecology"));
        assert!(by_tag("evolution").contains("MBE"));

        // Every venue has at least one tag and no duplicates.
        for venue in &cfg.venues {
            assert!(!venue.tags.is_empty(), "{} has no tag", venue.id);
            assert_eq!(
                venue
                    .tags
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                venue.tags.len(),
                "{} has duplicate tags",
                venue.id
            );
        }
    }

    #[test]
    fn bundled_venues_have_openalex_identifier() {
        // Every bundled venue must be fetchable via OpenAlex (ISSN or source id),
        // so a catalog edit can't ship an unfetchable venue.
        for (bundle, yaml) in BUNDLED_VENUES {
            let bundle_file: BundleFile = serde_yaml::from_str(yaml).unwrap();
            for venue in bundle_file.venues {
                assert!(
                    !venue.issn.is_empty() || venue.openalex_source_id.is_some(),
                    "venue {} in {bundle} has no OpenAlex identifier",
                    venue.id
                );
            }
        }
    }

    #[test]
    fn lookup_by_alias_is_case_insensitive() {
        let cfg = Config::defaults().unwrap();
        assert_eq!(cfg.venue("lancet-id").unwrap().id, "Lancet-ID");
        assert_eq!(cfg.venue("EID").unwrap().id, "EID");
        assert_eq!(cfg.venue("Epidemics").unwrap().id, "Epidemics");
        assert_eq!(cfg.venue("plos-cb").unwrap().id, "PLoS-Comp-Biol");
        assert_eq!(cfg.venue("tree").unwrap().id, "TREE");
        assert!(cfg.venue("nope").is_none());
    }

    #[test]
    fn merge_replaces_existing_and_adds_new() {
        let cfg = Config::from_yaml(
            r#"
defaults:
  min_year: 2015
venues:
  - id: NDSS
    dblp_stream: conf/ndss
    rank: B
    aliases: [ndss]
  - id: MYVENUE
    dblp_stream: conf/myv
    aliases: [myv]
"#,
        )
        .unwrap();
        assert_eq!(cfg.defaults.min_year, 2015);
        assert_eq!(cfg.venue("NDSS").unwrap().rank.as_deref(), Some("B"));
        assert!(cfg.venue("myv").is_some());
    }

    #[test]
    fn config_without_defaults_uses_default_min_year() {
        let cfg = Config::from_yaml(
            r#"
venues:
  - id: MYVENUE
    dblp_stream: conf/myv
"#,
        )
        .unwrap();
        assert_eq!(cfg.defaults.min_year, 2000);
        assert!(cfg.venue("MYVENUE").is_some());
    }

    #[test]
    fn generated_default_config_parses() {
        let yaml = Config::default_user_config_yaml().unwrap();
        assert!(!yaml.contains("venues: []"));
        let cfg = Config::from_yaml(&yaml).unwrap();
        assert_eq!(cfg.bundles, vec!["epi", "modelling", "ecoevo"]);
        assert_eq!(cfg.defaults.min_year, 2000);
        assert!(cfg.venue("Epidemics").is_some());
        assert!(cfg.venue("Ecology").is_some());
    }

    #[test]
    fn bundle_selection_limits_bundled_venues() {
        let cfg = Config::from_yaml("bundles: [epi]\n").unwrap();
        assert_eq!(cfg.bundles, vec!["epi"]);
        assert!(cfg.venue("JID").is_some());
        assert!(cfg.venue("Ecology").is_none());
    }

    #[test]
    fn empty_bundles_loads_only_custom_venues() {
        let cfg = Config::from_yaml(
            r#"
bundles: []
venues:
  - id: LOCAL
    issn: ["1234-5678"]
"#,
        )
        .unwrap();
        assert!(cfg.venue("LOCAL").is_some());
        assert!(cfg.venue("Epidemics").is_none());
    }

    #[test]
    fn unknown_bundle_errors() {
        assert!(Config::from_yaml("bundles: [bogus]\n").is_err());
    }

    #[test]
    fn bundle_override_preserves_user_venues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            r#"
bundles: [epi]
venues:
  - id: LOCAL
    issn: ["1234-5678"]
"#,
        )
        .unwrap();
        let bundles = vec!["ecoevo".to_string()];
        let cfg = Config::load_with_bundles(Some(&path), Some(&bundles)).unwrap();
        assert!(cfg.venue("Ecology").is_some());
        assert!(cfg.venue("JID").is_none());
        assert!(cfg.venue("LOCAL").is_some());
    }

    #[test]
    fn bundle_override_keeps_user_override_for_unselected_bundle_venue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            r#"
bundles: [epi]
venues:
  - id: Epidemics
    issn: ["9999-9999"]
    rank: custom
"#,
        )
        .unwrap();
        let bundles = vec!["ecoevo".to_string()];
        let cfg = Config::load_with_bundles(Some(&path), Some(&bundles)).unwrap();
        let venue = cfg.venue("Epidemics").unwrap();
        assert_eq!(venue.issn, vec!["9999-9999".to_string()]);
        assert_eq!(venue.rank.as_deref(), Some("custom"));
    }

    #[test]
    fn rank_sort_order_sorts_by_rank() {
        let cfg = Config {
            bundles: Vec::new(),
            defaults: Defaults::default(),
            venues: vec![
                venue("BVENUE", "B", &[]),
                venue("ASTAR1", "a*", &[]),
                venue("AVENUE", "A", &[]),
                venue("ASTAR2", "A*", &[]),
                venue("UNRANKED", "", &[]),
            ],
        };
        let groups = cfg.rank_sort_order();
        assert_eq!(
            groups.first(),
            Some(&vec!["ASTAR1".to_string(), "ASTAR2".to_string()])
        );
        assert_eq!(groups.get(1), Some(&vec!["AVENUE".to_string()]));
        assert_eq!(groups.get(2), Some(&vec!["BVENUE".to_string()]));
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn resolve_venues_reports_unknown() {
        let cfg = Config::defaults().unwrap();
        let ok = cfg
            .resolve_venues(&["epidemics".into(), "eid".into()])
            .unwrap();
        assert_eq!(ok, vec!["Epidemics".to_string(), "EID".to_string()]);
        assert!(cfg.resolve_venues(&["bogus".into()]).is_err());
    }
}
