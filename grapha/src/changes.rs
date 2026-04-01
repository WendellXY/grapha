use std::collections::{HashMap, HashSet};
use std::path::Path;

use git2::{DiffOptions, Repository};
use serde::Serialize;

use crate::query::impact;
use grapha_core::graph::Graph;

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

    let changed_symbols = collect_changed_symbols(&changed_hunks, graph);

    let mut affected_symbols = Vec::new();
    for sym in &changed_symbols {
        let impact_result = impact::query_impact(graph, &sym.id, 3).map_err(|error| {
            anyhow::anyhow!(
                "failed to resolve changed symbol {} during impact analysis: {error}",
                sym.id
            )
        })?;
        affected_symbols.push(impact_result);
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

fn collect_changed_symbols(changed_hunks: &[Hunk], graph: &Graph) -> Vec<ChangedSymbol> {
    let node_index: HashMap<&str, &grapha_core::graph::Node> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut changed_symbols = Vec::new();
    let mut seen_ids = HashSet::new();

    for hunk in changed_hunks {
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

        for edge in &graph.edges {
            if edge.provenance.iter().any(|provenance| {
                provenance.file.to_string_lossy().as_ref() == hunk.file
                    && ranges_overlap(
                        hunk.start_line,
                        hunk.end_line,
                        provenance.span.start[0],
                        provenance.span.end[0],
                    )
            }) && let Some(source_node) = node_index.get(edge.source.as_str())
                && seen_ids.insert(source_node.id.clone())
            {
                changed_symbols.push(ChangedSymbol {
                    id: source_node.id.clone(),
                    name: source_node.name.clone(),
                    file: hunk.file.clone(),
                });
            }
        }
    }

    changed_symbols
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
    use grapha_core::graph::{
        Edge, EdgeKind, EdgeProvenance, Graph, Node, NodeKind, Span, Visibility,
    };
    use std::path::PathBuf;

    #[test]
    fn ranges_overlap_works() {
        assert!(ranges_overlap(0, 10, 5, 15));
        assert!(ranges_overlap(5, 15, 0, 10));
        assert!(!ranges_overlap(0, 5, 10, 15));
        assert!(ranges_overlap(0, 10, 10, 20));
    }

    #[test]
    fn collect_changed_symbols_matches_edge_provenance() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "src/lib.rs::handler".to_string(),
                kind: NodeKind::Function,
                name: "handler".to_string(),
                file: PathBuf::from("src/lib.rs"),
                span: Span {
                    start: [0, 0],
                    end: [1, 0],
                },
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
                snippet: None,
            }],
            edges: vec![Edge {
                source: "src/lib.rs::handler".to_string(),
                target: "src/lib.rs::db_call".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: vec![EdgeProvenance {
                    file: PathBuf::from("src/lib.rs"),
                    span: Span {
                        start: [8, 4],
                        end: [8, 20],
                    },
                    symbol_id: "src/lib.rs::handler".to_string(),
                }],
            }],
        };

        let changed_symbols = collect_changed_symbols(
            &[Hunk {
                file: "src/lib.rs".to_string(),
                start_line: 8,
                end_line: 8,
            }],
            &graph,
        );

        assert_eq!(changed_symbols.len(), 1);
        assert_eq!(changed_symbols[0].id, "src/lib.rs::handler");
    }
}
