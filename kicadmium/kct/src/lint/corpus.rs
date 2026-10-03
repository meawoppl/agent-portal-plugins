//! Labelled corpus evaluation: run lint over a manifest of boards and check
//! per-rule finding counts against expectations.
//!
//! A manifest is schema 1 JSON with `cases: [{name, board, board_id, config?,
//! expected: [{rule, min, max, subjects?}]}]`; paths resolve relative to the
//! manifest. An expected rule that was not `evaluated` is a mismatch even when
//! `min` is zero.

use super::{lint, rules, Config};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub cases: Vec<Case>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub board: PathBuf,
    pub board_id: String,
    pub config: Option<PathBuf>,
    pub expected: Vec<Expectation>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub rule: String,
    pub min: usize,
    pub max: usize,
    #[serde(default)]
    pub subjects: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct Evaluation {
    pub name: String,
    pub passed: bool,
    pub counts: BTreeMap<String, usize>,
    pub mismatches: Vec<String>,
    pub error: Option<String>,
}

/// Read a lint `Config` JSON file, or the default when `path` is `None`.
pub fn read_config(path: Option<&Path>) -> Result<Config> {
    Ok(if let Some(p) = path {
        serde_json::from_str(
            &fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
        )?
    } else {
        Config::default()
    })
}

impl Manifest {
    /// Load and structurally validate a manifest (schema 1, at least one case).
    pub fn load(path: &Path) -> Result<Self> {
        let m: Manifest = serde_json::from_str(
            &fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
        )?;
        if m.schema != 1 {
            bail!("unsupported corpus schema")
        }
        if m.cases.is_empty() {
            bail!("corpus must have at least one case")
        }
        Ok(m)
    }

    /// Every board and config file the manifest reads, resolved against `base`.
    pub fn inputs(&self, base: &Path) -> Vec<PathBuf> {
        self.cases
            .iter()
            .flat_map(|c| std::iter::once(&c.board).chain(c.config.as_ref()))
            .map(|p| base.join(p))
            .collect()
    }

    /// Lint every case and compare finding counts with its expectations.
    ///
    /// Malformed cases (duplicate names, no expectations, `min > max`, or an
    /// expectation for a rule that is not implemented) are errors; a case
    /// whose board or config cannot be read is a failed evaluation instead.
    pub fn evaluate(self, base: &Path) -> Result<Vec<Evaluation>> {
        let catalog = rules::catalog();
        let mut names = BTreeSet::new();
        let mut results = vec![];
        for case in self.cases {
            if !names.insert(case.name.clone()) || case.expected.is_empty() {
                bail!("case names must be unique and each case needs expectations")
            }
            for x in &case.expected {
                if x.min > x.max
                    || !catalog
                        .iter()
                        .any(|r| r.id == x.rule && r.status == "implemented")
                {
                    bail!("invalid expectation {}", x.rule)
                }
            }
            let result = (|| {
                let text = fs::read_to_string(base.join(&case.board))?;
                lint(
                    &text,
                    &case.board_id,
                    read_config(case.config.as_ref().map(|p| base.join(p)).as_deref())?,
                )
            })();
            match result {
                Ok(r) => {
                    let mut counts = BTreeMap::new();
                    for f in &r.findings {
                        *counts.entry(f.rule.clone()).or_default() += 1;
                    }
                    let mut mismatches = vec![];
                    for x in case.expected {
                        if !r
                            .coverage
                            .iter()
                            .any(|c| c.rule == x.rule && c.status == "evaluated")
                        {
                            mismatches.push(format!("{} not evaluated", x.rule));
                            continue;
                        }
                        let n = r
                            .findings
                            .iter()
                            .filter(|f| {
                                f.rule == x.rule
                                    && x.subjects.iter().all(|s| f.subjects.contains(s))
                            })
                            .count();
                        if n < x.min || n > x.max {
                            mismatches.push(format!(
                                "{}: found {n}, expected {}..={} on {:?}",
                                x.rule, x.min, x.max, x.subjects
                            ));
                        }
                    }
                    results.push(Evaluation {
                        name: case.name,
                        passed: mismatches.is_empty(),
                        counts,
                        mismatches,
                        error: None,
                    });
                }
                Err(err) => results.push(Evaluation {
                    name: case.name,
                    passed: false,
                    counts: BTreeMap::new(),
                    mismatches: vec![],
                    error: Some(format!("{err:#}")),
                }),
            }
        }
        Ok(results)
    }
}
