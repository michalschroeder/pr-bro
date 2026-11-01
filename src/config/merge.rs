//! Layered config loading: each `--config` file is parsed into an all-optional
//! [`ConfigLayer`], layers are merged left to right, and built-in defaults are
//! applied once to the merged result.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::config::schema::{default_refresh_interval, default_theme};
use crate::config::{Config, QueryConfig};
use crate::scoring::{merge_scoring_configs, ScoringConfig};

/// One config file as written, before defaults are applied.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigLayer {
    pub scoring: Option<ScoringConfig>,
    pub queries: Option<Vec<QueryLayer>>,
    pub auto_refresh_interval: Option<u64>,
    pub theme: Option<String>,
}

/// One query entry in a layer. `query` may be omitted when overriding an earlier query by name.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QueryLayer {
    pub name: Option<String>,
    pub query: Option<String>,
    pub scoring: Option<ScoringConfig>,
}

/// Where a merged query came from.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryOrigin {
    /// File that introduced the query (or its name)
    pub source: PathBuf,
    /// Later files that overrode it by name, in order
    pub overridden_by: Vec<PathBuf>,
}

/// Accumulator for merging layers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MergedLayers {
    pub scoring: Option<ScoringConfig>,
    pub queries: Vec<MergedQuery>,
    pub auto_refresh_interval: Option<u64>,
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergedQuery {
    pub layer: QueryLayer,
    pub origin: QueryOrigin,
}

/// Final result of loading one or more config files.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    /// Origin of each query, parallel to `config.queries`
    pub origins: Vec<QueryOrigin>,
}

/// Merge `layer` (read from `source`) on top of `acc`.
///
/// Scalars: later wins if set. Scoring: leaf-level via `merge_scoring_configs`.
/// Queries: a named query overrides the first earlier query with the same name
/// (exact match) in place; anything else is appended. Names only match queries
/// from earlier layers, and each earlier query is overridden at most once per
/// layer, so duplicate names within one file are all kept.
pub fn merge(mut acc: MergedLayers, layer: ConfigLayer, source: &Path) -> MergedLayers {
    acc.theme = layer.theme.or(acc.theme);
    acc.auto_refresh_interval = layer.auto_refresh_interval.or(acc.auto_refresh_interval);
    acc.scoring = merge_scoring(acc.scoring, layer.scoring);

    let mut overridden = vec![false; acc.queries.len()];
    for q in layer.queries.into_iter().flatten() {
        let existing = q.name.as_ref().and_then(|name| {
            (0..overridden.len())
                .find(|&i| !overridden[i] && acc.queries[i].layer.name.as_ref() == Some(name))
        });
        match existing {
            Some(i) => {
                overridden[i] = true;
                let e = &mut acc.queries[i];
                e.layer.query = q.query.or(e.layer.query.take());
                e.layer.scoring = merge_scoring(e.layer.scoring.take(), q.scoring);
                e.origin.overridden_by.push(source.to_path_buf());
            }
            None => acc.queries.push(MergedQuery {
                layer: q,
                origin: QueryOrigin {
                    source: source.to_path_buf(),
                    overridden_by: Vec::new(),
                },
            }),
        }
    }
    acc
}

fn merge_scoring(
    earlier: Option<ScoringConfig>,
    later: Option<ScoringConfig>,
) -> Option<ScoringConfig> {
    match (earlier, later) {
        (Some(a), Some(b)) => Some(merge_scoring_configs(&a, Some(&b))),
        (a, b) => b.or(a),
    }
}

/// Apply built-in defaults to the merged layers and check that the result is usable.
pub fn finish(acc: MergedLayers, sources: &[PathBuf]) -> Result<LoadedConfig> {
    if acc.queries.is_empty() {
        bail!(
            "No queries configured in {}. Add at least one query, e.g.:\n  queries:\n    - name: my-reviews\n      query: \"is:pr review-requested:@me\"",
            display_paths(sources)
        );
    }

    let mut queries = Vec::with_capacity(acc.queries.len());
    let mut origins = Vec::with_capacity(acc.queries.len());
    for MergedQuery { layer: q, origin } in acc.queries {
        let Some(query) = q.query else {
            let from = origin.source.display();
            match &q.name {
                Some(name) if sources.len() > 1 => bail!(
                    "Query '{}' (from {}) has no `query` and does not override an earlier query",
                    name,
                    from
                ),
                Some(name) => bail!("Query '{}' (from {}) has no `query`", name, from),
                None => bail!("An unnamed query (from {}) has no `query`", from),
            }
        };
        queries.push(QueryConfig {
            name: q.name,
            query,
            scoring: q.scoring,
        });
        origins.push(origin);
    }

    Ok(LoadedConfig {
        config: Config {
            scoring: acc.scoring,
            queries,
            auto_refresh_interval: acc
                .auto_refresh_interval
                .unwrap_or_else(default_refresh_interval),
            theme: acc.theme.unwrap_or_else(default_theme),
        },
        origins,
    })
}

/// Comma-separated list of paths for messages.
pub fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(yaml: &str) -> ConfigLayer {
        serde_saphyr::from_str(yaml).unwrap()
    }

    fn source(i: usize) -> PathBuf {
        PathBuf::from(format!("f{}.yaml", i))
    }

    /// Merge YAML layers named f0.yaml, f1.yaml, ... and finish.
    fn merge_all(layers: &[&str]) -> Result<LoadedConfig> {
        let sources: Vec<PathBuf> = (0..layers.len()).map(source).collect();
        let acc = layers
            .iter()
            .zip(&sources)
            .fold(MergedLayers::default(), |acc, (yaml, src)| {
                merge(acc, layer(yaml), src)
            });
        finish(acc, &sources)
    }

    enum Expect {
        /// Merged config equals this config, parsed as a single file with `Config`
        Merges(&'static str),
        /// Merging fails with an error containing all of these substrings
        Fails(&'static [&'static str]),
    }

    #[test]
    fn test_merge_cases() {
        use Expect::*;
        let cases: &[(&str, &[&str], Expect)] = &[
            (
                "theme-only layer keeps earlier refresh interval",
                &[
                    "auto_refresh_interval: 60\nqueries: [{query: q}]",
                    "theme: dark",
                ],
                Merges("auto_refresh_interval: 60\ntheme: dark\nqueries: [{query: q}]"),
            ),
            (
                "refresh-only layer keeps earlier theme",
                &["theme: light\nqueries: [{query: q}]", "auto_refresh_interval: 60"],
                Merges("auto_refresh_interval: 60\ntheme: light\nqueries: [{query: q}]"),
            ),
            (
                "later theme and refresh win",
                &[
                    "theme: light\nauto_refresh_interval: 60\nqueries: [{query: q}]",
                    "theme: dark\nauto_refresh_interval: 10",
                ],
                Merges("theme: dark\nauto_refresh_interval: 10\nqueries: [{query: q}]"),
            ),
            (
                "defaults applied when no layer sets theme or refresh",
                &["queries: [{query: q}]", "scoring: {draft: x0.5}"],
                Merges(
                    "theme: auto\nauto_refresh_interval: 300\nscoring: {draft: x0.5}\nqueries: [{query: q}]",
                ),
            ),
            (
                "scoring (None, Some) takes the later block",
                &["queries: [{query: q}]", "scoring: {age: '+2 per 1h'}"],
                Merges("scoring: {age: '+2 per 1h'}\nqueries: [{query: q}]"),
            ),
            (
                "scoring (Some, None) keeps the earlier block",
                &["scoring: {age: '+2 per 1h'}\nqueries: [{query: q}]", "theme: dark"],
                Merges("theme: dark\nscoring: {age: '+2 per 1h'}\nqueries: [{query: q}]"),
            ),
            (
                "scoring (Some, Some) merges leaf-level",
                &[
                    r#"
scoring:
  base_score: 50
  age: "+1 per 1h"
  size: {exclude: ["*.lock"], buckets: [{range: "<100", effect: x2}]}
  labels: [{name: urgent, effect: "+20"}, {name: wip, effect: x0.5}]
queries: [{query: q}]
"#,
                    r#"
scoring:
  age: "+5 per 1h"
  size: {exclude: []}
  labels: [{name: URGENT, effect: "+50"}, {name: docs, effect: x0.9}]
"#,
                ],
                Merges(
                    r#"
scoring:
  base_score: 50
  age: "+5 per 1h"
  size: {exclude: [], buckets: [{range: "<100", effect: x2}]}
  labels: [{name: URGENT, effect: "+50"}, {name: wip, effect: x0.5}, {name: docs, effect: x0.9}]
queries: [{query: q}]
"#,
                ),
            ),
            (
                "team file without scoring + later partial scoring = exactly the partial block",
                &["queries: [{query: q}]", "scoring: {draft: x0.5}"],
                Merges("scoring: {draft: x0.5}\nqueries: [{query: q}]"),
            ),
            (
                "override by name replaces query, merges scoring, keeps position",
                &[
                    r#"
queries:
  - {name: a, query: qa, scoring: {age: "+2 per 1h", draft: x0.1}}
  - {name: b, query: qb}
"#,
                    r#"queries: [{name: a, query: qa2, scoring: {draft: x0.5}}]"#,
                ],
                Merges(
                    r#"
queries:
  - {name: a, query: qa2, scoring: {age: "+2 per 1h", draft: x0.5}}
  - {name: b, query: qb}
"#,
                ),
            ),
            (
                "override with only scoring keeps the earlier query string",
                &[
                    "queries: [{name: a, query: qa}, {name: b, query: qb}]",
                    "queries: [{name: b, scoring: {draft: x0.5}}]",
                ],
                Merges(
                    "queries: [{name: a, query: qa}, {name: b, query: qb, scoring: {draft: x0.5}}]",
                ),
            ),
            (
                "new-name and unnamed queries are appended in file order",
                &[
                    "queries: [{name: a, query: qa}]",
                    "queries: [{query: q1}, {name: c, query: qc}, {query: q2}]",
                ],
                Merges(
                    "queries: [{name: a, query: qa}, {query: q1}, {name: c, query: qc}, {query: q2}]",
                ),
            ),
            (
                "names match case-sensitively",
                &[
                    "queries: [{name: backend, query: q1}]",
                    "queries: [{name: Backend, query: q2}]",
                ],
                Merges("queries: [{name: backend, query: q1}, {name: Backend, query: q2}]"),
            ),
            (
                "duplicate earlier names: first match is overridden",
                &[
                    "queries: [{name: a, query: q1}, {name: a, query: q2}]",
                    "queries: [{name: a, query: q3}]",
                ],
                Merges("queries: [{name: a, query: q3}, {name: a, query: q2}]"),
            ),
            (
                "duplicate earlier names: later duplicates override them pairwise",
                &[
                    "queries: [{name: a, query: q1}, {name: a, query: q2}]",
                    "queries: [{name: a, query: q3}, {name: a, query: q4}]",
                ],
                Merges("queries: [{name: a, query: q3}, {name: a, query: q4}]"),
            ),
            (
                "later duplicate name overrides once, then is appended",
                &[
                    "queries: [{name: a, query: q0}]",
                    "queries: [{name: a, query: q1}, {name: a, query: q2}]",
                ],
                Merges("queries: [{name: a, query: q1}, {name: a, query: q2}]"),
            ),
            (
                "labels keep order: overrides in place, new ones appended",
                &[
                    "scoring: {labels: [{name: urgent, effect: '+20'}, {name: wip, effect: x0.5}]}\nqueries: [{query: q}]",
                    "scoring: {labels: [{name: docs, effect: x0.9}, {name: Urgent, effect: '+5'}]}",
                ],
                Merges(
                    "scoring: {labels: [{name: Urgent, effect: '+5'}, {name: wip, effect: x0.5}, {name: docs, effect: x0.9}]}\nqueries: [{query: q}]",
                ),
            ),
            (
                "duplicate names within one file are all kept (single-file behaviour)",
                &["queries: [{name: a, query: q1}, {name: a, query: q2}]"],
                Merges("queries: [{name: a, query: q1}, {name: a, query: q2}]"),
            ),
            (
                "later query never moves an earlier one",
                &[
                    "queries: [{name: a, query: qa}, {name: b, query: qb}, {name: c, query: qc}]",
                    "queries: [{name: d, query: qd}, {name: c, query: qc2}, {name: a, query: qa2}]",
                ],
                Merges(
                    "queries: [{name: a, query: qa2}, {name: b, query: qb}, {name: c, query: qc2}, {name: d, query: qd}]",
                ),
            ),
            (
                "same file twice is applied twice",
                &[
                    "queries: [{name: a, query: qa}, {query: q}]",
                    "queries: [{name: a, query: qa}, {query: q}]",
                ],
                Merges("queries: [{name: a, query: qa}, {query: q}, {query: q}]"),
            ),
            (
                "empty later layer changes nothing",
                &["theme: dark\nqueries: [{name: a, query: qa}]", ""],
                Merges("theme: dark\nqueries: [{name: a, query: qa}]"),
            ),
            (
                "query without `query` after merge names query and file",
                &[
                    "queries: [{name: a, query: qa}]",
                    "queries: [{name: backend, scoring: {draft: x0.5}}]",
                ],
                Fails(&["Query 'backend'", "f1.yaml", "has no `query`"]),
            ),
            (
                "unnamed query without `query` names its file",
                &["queries: [{scoring: {draft: x0.5}}]"],
                Fails(&["unnamed query", "f0.yaml", "has no `query`"]),
            ),
            (
                "zero queries after merge",
                &["theme: dark", "scoring: {draft: x0.5}"],
                Fails(&["No queries configured", "f0.yaml", "f1.yaml"]),
            ),
            (
                "explicit empty queries list",
                &["queries: []"],
                Fails(&["No queries configured", "f0.yaml"]),
            ),
        ];

        for (name, layers, expect) in cases {
            let result = merge_all(layers);
            match expect {
                Merges(yaml) => {
                    let expected: Config = serde_saphyr::from_str(yaml)
                        .unwrap_or_else(|e| panic!("{name}: bad expected yaml: {e}"));
                    let actual = result.unwrap_or_else(|e| panic!("{name}: {e:#}")).config;
                    assert_eq!(
                        format!("{actual:#?}"),
                        format!("{expected:#?}"),
                        "case: {name}"
                    );
                }
                Fails(parts) => {
                    let err = match result {
                        Ok(r) => panic!("{name}: expected error, got {:?}", r.config),
                        Err(e) => format!("{e:#}"),
                    };
                    for part in *parts {
                        assert!(err.contains(part), "case: {name}: {err:?} lacks {part:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn test_merge_empty_layer_is_identity() {
        let acc = merge(
            MergedLayers::default(),
            layer(include_str!("testdata/docs_full_example.yaml")),
            &source(0),
        );
        assert_eq!(merge(acc.clone(), ConfigLayer::default(), &source(1)), acc);
    }

    #[test]
    fn test_origins_track_source_and_overrides() {
        let loaded = merge_all(&[
            "queries: [{name: a, query: qa}, {name: b, query: qb}]",
            "queries: [{name: a, query: qa2}, {name: c, query: qc}]",
            "queries: [{name: a, scoring: {draft: x0.5}}]",
        ])
        .unwrap();
        assert_eq!(
            loaded.origins,
            vec![
                QueryOrigin {
                    source: source(0),
                    overridden_by: vec![source(1), source(2)],
                },
                QueryOrigin {
                    source: source(0),
                    overridden_by: vec![],
                },
                QueryOrigin {
                    source: source(1),
                    overridden_by: vec![],
                },
            ]
        );
    }

    #[test]
    fn test_docs_team_local_example() {
        let loaded = merge_all(&[
            include_str!("testdata/team.yaml"),
            include_str!("testdata/local.yaml"),
        ])
        .unwrap();
        let config = loaded.config;
        assert_eq!(config.theme, "dark");
        assert_eq!(config.auto_refresh_interval, 300);
        assert_eq!(
            config.scoring,
            Some(ScoringConfig {
                base_score: None,
                age: Some("+5 per 1h".to_string()),
                approvals: None,
                size: None,
                labels: None,
                previously_reviewed: None,
                draft: None,
            })
        );
        let names: Vec<_> = config.queries.iter().map(|q| q.name.as_deref()).collect();
        assert_eq!(names, [Some("backend"), Some("frontend"), Some("infra")]);
        let backend: &QueryConfig = &config.queries[0];
        assert_eq!(
            backend.query,
            "is:pr is:open repo:acme/api -author:app/dependabot"
        );
        assert_eq!(
            backend.scoring.as_ref().and_then(|s| s.age.as_deref()),
            Some("+2 per 1h")
        );
    }
}
