use std::collections::HashSet;
use std::path::Path;

use git2::{DiffOptions, Repository};
use serde::Serialize;

use grapha_core::graph::Graph;
use crate::query::impact;

#[derive(Debug, Serialize)]
pub struct ChangeReport {
    pub changed_files: Vec<String>,
    pub changed_symbols: Vec<ChangedSymbol>,
    pub affected_symbols: Vec<impact::ImpactResult>,
    pub risk_summary: RiskSummary,
}

#[derive(Debug, Serialize)]
pub struct ChangedSymbol {
    pub id: String,
    pub name: String,
    pub file: String,
}

#[derive(Debug, Serialize)]
pub struct RiskSummary {
    pub changed_count: usize,
    pub directly_affected: usize,
    pub transitively_affected: usize,
    pub risk_level: String,
}

pub fn detect_changes(
    repo_path: &Path,
    graph: &Graph,
    scope: &str,
) -> anyhow::Result<ChangeReport> {
    let repo = Repository::discover(repo_path)?;

    let changed_hunks = match scope {
        "unstaged" => diff_unstaged(&repo)?,
        "staged" => diff_staged(&repo)?,
        "all" => {
            let mut hunks = diff_unstaged(&repo)?;
            hunks.extend(diff_staged(&repo)?);
            hunks
        }
        base_ref => diff_against_ref(&repo, base_ref)?,
    };

    let changed_files: Vec<String> = changed_hunks
        .iter()
        .map(|h| h.file.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut changed_symbols = Vec::new();
    let mut seen_ids = HashSet::new();

    for hunk in &changed_hunks {
        for node in &graph.nodes {
            let node_file = node.file.to_string_lossy();
            if node_file.as_ref() == hunk.file
                && ranges_overlap(
                    hunk.start_line,
                    hunk.end_line,
                    node.span.start[0],
                    node.span.end[0],
                )
                && seen_ids.insert(node.id.clone())
            {
                changed_symbols.push(ChangedSymbol {
                    id: node.id.clone(),
                    name: node.name.clone(),
                    file: hunk.file.clone(),
                });
            }
        }
    }

    let mut affected_symbols = Vec::new();
    for sym in &changed_symbols {
        if let Some(impact_result) = impact::query_impact(graph, &sym.id, 3) {
            affected_symbols.push(impact_result);
        }
    }

    let directly_affected: usize = affected_symbols.iter().map(|r| r.depth_1.len()).sum();
    let transitively_affected: usize = affected_symbols.iter().map(|r| r.total_affected).sum();

    let risk_level = if transitively_affected > 20 {
        "high"
    } else if transitively_affected > 5 {
        "medium"
    } else {
        "low"
    }
    .to_string();

    let changed_count = changed_symbols.len();

    Ok(ChangeReport {
        changed_files,
        changed_symbols,
        affected_symbols,
        risk_summary: RiskSummary {
            changed_count,
            directly_affected,
            transitively_affected,
            risk_level,
        },
    })
}

struct Hunk {
    file: String,
    start_line: usize,
    end_line: usize,
}

fn ranges_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start <= b_end && b_start <= a_end
}

fn diff_unstaged(repo: &Repository) -> anyhow::Result<Vec<Hunk>> {
    let mut opts = DiffOptions::new();
    let diff = repo.diff_index_to_workdir(None, Some(&mut opts))?;
    extract_hunks(&diff)
}

fn diff_staged(repo: &Repository) -> anyhow::Result<Vec<Hunk>> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    let diff = repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?;
    extract_hunks(&diff)
}

fn diff_against_ref(repo: &Repository, refspec: &str) -> anyhow::Result<Vec<Hunk>> {
    let obj = repo.revparse_single(refspec)?;
    let tree = obj.peel_to_tree()?;
    let mut opts = DiffOptions::new();
    let diff = repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?;
    extract_hunks(&diff)
}

fn extract_hunks(diff: &git2::Diff) -> anyhow::Result<Vec<Hunk>> {
    let mut hunks = Vec::new();

    diff.foreach(
        &mut |_delta, _progress| true,
        None,
        Some(&mut |delta, hunk| {
            if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                hunks.push(Hunk {
                    file: path.to_string(),
                    start_line: hunk.new_start() as usize,
                    end_line: (hunk.new_start() + hunk.new_lines()) as usize,
                });
            }
            true
        }),
        None,
    )?;

    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_overlap_works() {
        assert!(ranges_overlap(0, 10, 5, 15));
        assert!(ranges_overlap(5, 15, 0, 10));
        assert!(!ranges_overlap(0, 5, 10, 15));
        assert!(ranges_overlap(0, 10, 10, 20));
    }
}
