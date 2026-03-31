use std::collections::{BTreeMap, BTreeSet};

use grapha_core::graph::NodeKind;

use crate::query::{
    ContextResult, SymbolInfo, SymbolRef, entries::EntriesResult, impact::ImpactResult,
    impact::ImpactTreeNode, reverse::AffectedEntry, reverse::ReverseResult, trace::Flow,
    trace::TraceResult,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeNode {
    label: String,
    children: Vec<TreeNode>,
}

impl TreeNode {
    fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            children: Vec::new(),
        }
    }

    fn branch(label: impl Into<String>, children: Vec<TreeNode>) -> Self {
        Self {
            label: label.into(),
            children,
        }
    }
}

#[derive(Debug, Default)]
struct PathMergeNode {
    children: BTreeMap<String, PathMergeNode>,
    notes: BTreeSet<String>,
}

impl PathMergeNode {
    fn insert_path<I>(&mut self, segments: I) -> &mut Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut current = self;
        for segment in segments {
            current = current.children.entry(segment).or_default();
        }
        current
    }

    fn into_tree_node(self, label: String) -> TreeNode {
        let mut children: Vec<TreeNode> = self
            .children
            .into_iter()
            .map(|(child_label, child)| child.into_tree_node(child_label))
            .collect();
        children.extend(self.notes.into_iter().map(|note| TreeNode::leaf(note)));
        TreeNode::branch(label, children)
    }

    fn into_tree_children(self) -> Vec<TreeNode> {
        self.children
            .into_iter()
            .map(|(child_label, child)| child.into_tree_node(child_label))
            .collect()
    }
}

fn kind_label(kind: NodeKind) -> String {
    serde_json::to_string(&kind)
        .unwrap_or_else(|_| format!("{kind:?}"))
        .trim_matches('"')
        .to_string()
}

fn format_symbol_info(symbol: &SymbolInfo) -> String {
    format!(
        "{} [{}] ({})",
        symbol.name,
        kind_label(symbol.kind),
        symbol.file
    )
}

fn format_symbol_ref(symbol: &SymbolRef) -> String {
    format!(
        "{} [{}] ({})",
        symbol.name,
        kind_label(symbol.kind),
        symbol.file
    )
}

fn sorted_symbol_refs(symbols: &[SymbolRef]) -> Vec<SymbolRef> {
    let mut sorted = symbols.to_vec();
    sorted.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.id.cmp(&right.id))
    });
    sorted
}

fn render_tree(root: &TreeNode) -> String {
    let mut lines = vec![root.label.clone()];
    render_children(&root.children, "", &mut lines);
    lines.join("\n")
}

fn render_children(children: &[TreeNode], prefix: &str, lines: &mut Vec<String>) {
    for (index, child) in children.iter().enumerate() {
        let is_last = index + 1 == children.len();
        let branch = if is_last { "└── " } else { "├── " };
        lines.push(format!("{prefix}{branch}{}", child.label));
        let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
        render_children(&child.children, &child_prefix, lines);
    }
}

fn impact_tree_to_tree_node(node: &ImpactTreeNode) -> TreeNode {
    TreeNode::branch(
        format_symbol_ref(&node.symbol),
        node.children.iter().map(impact_tree_to_tree_node).collect(),
    )
}

fn format_trace_terminal(flow: &Flow) -> String {
    let last_segment = flow
        .path
        .last()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    match &flow.terminal {
        Some(terminal) => format!(
            "{last_segment} [terminal:{} {} {}]",
            terminal.kind, terminal.direction, terminal.operation
        ),
        None => last_segment,
    }
}

fn insert_trace_flow(tree: &mut PathMergeNode, flow: &Flow) {
    if flow.path.len() < 2 {
        return;
    }

    let mut segments: Vec<String> = flow.path.iter().skip(1).cloned().collect();
    if let Some(last) = segments.last_mut() {
        *last = format_trace_terminal(flow);
    }

    let leaf = tree.insert_path(segments);
    for condition in &flow.conditions {
        leaf.notes.insert(format!("condition: {condition}"));
    }
    for boundary in &flow.async_boundaries {
        leaf.notes.insert(format!("async: {boundary}"));
    }
}

fn reverse_root_label(result: &ReverseResult) -> String {
    let mut label = format_symbol_ref(&result.target_ref);
    if result
        .affected_entries
        .iter()
        .any(|entry| entry.distance == 0)
    {
        label.push_str(" [entry]");
    }
    label
}

fn reverse_leaf_label(entry: &AffectedEntry) -> String {
    format!(
        "{} [entry] [{}] ({})",
        entry.entry.name,
        kind_label(entry.entry.kind),
        entry.entry.file
    )
}

pub fn render_context(result: &ContextResult) -> String {
    let sections = [
        ("callers", sorted_symbol_refs(&result.callers)),
        ("callees", sorted_symbol_refs(&result.callees)),
        ("implementors", sorted_symbol_refs(&result.implementors)),
        ("implements", sorted_symbol_refs(&result.implements)),
        ("type_refs", sorted_symbol_refs(&result.type_refs)),
    ];

    let children = sections
        .into_iter()
        .filter(|(_, symbols)| !symbols.is_empty())
        .map(|(label, symbols)| {
            TreeNode::branch(
                format!("{label} ({})", symbols.len()),
                symbols
                    .into_iter()
                    .map(|symbol| TreeNode::leaf(format_symbol_ref(&symbol)))
                    .collect(),
            )
        })
        .collect();

    render_tree(&TreeNode::branch(
        format_symbol_info(&result.symbol),
        children,
    ))
}

pub fn render_entries(result: &EntriesResult) -> String {
    let children = sorted_symbol_refs(&result.entries)
        .into_iter()
        .map(|entry| TreeNode::leaf(format_symbol_ref(&entry)))
        .collect();

    render_tree(&TreeNode::branch(
        format!("entry points ({})", result.total),
        children,
    ))
}

pub fn render_trace(result: &TraceResult) -> String {
    let mut flows = PathMergeNode::default();
    for flow in &result.flows {
        insert_trace_flow(&mut flows, flow);
    }

    let root = TreeNode::branch(
        format_symbol_ref(&result.entry_ref),
        vec![
            TreeNode::leaf(format!(
                "summary: flows={}, reads={}, writes={}, async_crossings={}",
                result.summary.total_flows,
                result.summary.reads,
                result.summary.writes,
                result.summary.async_crossings
            )),
            TreeNode::branch(
                format!("flows ({})", result.summary.total_flows),
                flows.into_tree_children(),
            ),
        ],
    );

    render_tree(&root)
}

pub fn render_reverse(result: &ReverseResult) -> String {
    let mut tree = PathMergeNode::default();

    for affected in &result.affected_entries {
        let reversed: Vec<String> = affected.path.iter().rev().cloned().collect();
        if reversed.len() <= 1 {
            continue;
        }

        let mut segments: Vec<String> = reversed.into_iter().skip(1).collect();
        if let Some(last) = segments.last_mut() {
            *last = reverse_leaf_label(affected);
        }
        tree.insert_path(segments);
    }

    let root = TreeNode::branch(
        reverse_root_label(result),
        vec![TreeNode::branch(
            format!("affected entries ({})", result.total_entries),
            tree.into_tree_children(),
        )],
    );

    render_tree(&root)
}

pub fn render_impact(result: &ImpactResult) -> String {
    let dependents = result
        .tree
        .children
        .iter()
        .map(impact_tree_to_tree_node)
        .collect();

    let root = TreeNode::branch(
        format_symbol_ref(&result.source_ref),
        vec![
            TreeNode::leaf(format!(
                "summary: depth_1={}, depth_2={}, depth_3_plus={}, total={}",
                result.depth_1.len(),
                result.depth_2.len(),
                result.depth_3_plus.len(),
                result.total_affected
            )),
            TreeNode::branch(
                format!("dependents ({})", result.total_affected),
                dependents,
            ),
        ],
    );

    render_tree(&root)
}

#[cfg(test)]
mod tests {
    use grapha_core::graph::NodeKind;

    use crate::query::{
        ContextResult, SymbolInfo, SymbolRef, entries::EntriesResult, impact::ImpactResult,
        impact::ImpactTreeNode, reverse::AffectedEntry, reverse::ReverseResult, trace::Flow,
        trace::TerminalInfo, trace::TraceResult, trace::TraceSummary,
    };

    use super::*;

    fn symbol_ref(name: &str, kind: NodeKind, file: &str) -> SymbolRef {
        SymbolRef {
            id: format!("{file}::{name}"),
            name: name.to_string(),
            kind,
            file: file.to_string(),
        }
    }

    fn symbol_info(name: &str, kind: NodeKind, file: &str) -> SymbolInfo {
        SymbolInfo {
            id: format!("{file}::{name}"),
            name: name.to_string(),
            kind,
            file: file.to_string(),
            span: [1, 2],
        }
    }

    #[test]
    fn context_omits_empty_sections() {
        let result = ContextResult {
            symbol: symbol_info("helper", NodeKind::Function, "main.rs"),
            callers: vec![symbol_ref("main", NodeKind::Function, "main.rs")],
            callees: Vec::new(),
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let rendered = render_context(&result);
        assert!(rendered.contains("helper [function] (main.rs)"));
        assert!(rendered.contains("callers (1)"));
        assert!(rendered.contains("main [function] (main.rs)"));
        assert!(!rendered.contains("callees"));
        assert!(rendered.contains("└──"));
    }

    #[test]
    fn entries_render_as_tree() {
        let result = EntriesResult {
            entries: vec![
                symbol_ref("boot", NodeKind::Function, "boot.rs"),
                symbol_ref("main", NodeKind::Function, "main.rs"),
            ],
            total: 2,
        };

        let rendered = render_entries(&result);
        assert!(rendered.contains("entry points (2)"));
        assert!(rendered.contains("boot [function] (boot.rs)"));
        assert!(rendered.contains("main [function] (main.rs)"));
    }

    #[test]
    fn trace_merges_shared_prefixes_and_renders_notes() {
        let result = TraceResult {
            entry: "main.rs::main".to_string(),
            flows: vec![
                Flow {
                    path: vec!["main".into(), "service".into(), "db".into()],
                    terminal: Some(TerminalInfo {
                        kind: "persistence".into(),
                        operation: "save".into(),
                        direction: "write".into(),
                    }),
                    conditions: vec!["user.isAdmin".into()],
                    async_boundaries: vec!["service -> db".into()],
                },
                Flow {
                    path: vec!["main".into(), "service".into(), "cache".into()],
                    terminal: Some(TerminalInfo {
                        kind: "cache".into(),
                        operation: "put".into(),
                        direction: "write".into(),
                    }),
                    conditions: Vec::new(),
                    async_boundaries: Vec::new(),
                },
            ],
            summary: TraceSummary {
                total_flows: 2,
                reads: 0,
                writes: 2,
                async_crossings: 1,
            },
            entry_ref: symbol_ref("main", NodeKind::Function, "main.rs"),
        };

        let rendered = render_trace(&result);
        assert!(rendered.contains("main [function] (main.rs)"));
        assert!(rendered.contains("summary: flows=2, reads=0, writes=2, async_crossings=1"));
        assert!(rendered.contains("service"));
        assert!(rendered.contains("db [terminal:persistence write save]"));
        assert!(rendered.contains("cache [terminal:cache write put]"));
        assert!(rendered.contains("condition: user.isAdmin"));
        assert!(rendered.contains("async: service -> db"));
    }

    #[test]
    fn reverse_merges_paths_and_marks_entries() {
        let result = ReverseResult {
            symbol: "target.rs::db".to_string(),
            affected_entries: vec![
                AffectedEntry {
                    entry: symbol_ref("entry1", NodeKind::Function, "a.rs"),
                    distance: 2,
                    path: vec!["entry1".into(), "service".into(), "db".into()],
                },
                AffectedEntry {
                    entry: symbol_ref("entry2", NodeKind::Function, "b.rs"),
                    distance: 2,
                    path: vec!["entry2".into(), "service".into(), "db".into()],
                },
            ],
            total_entries: 2,
            target_ref: symbol_ref("db", NodeKind::Function, "target.rs"),
        };

        let rendered = render_reverse(&result);
        assert!(rendered.contains("db [function] (target.rs)"));
        assert!(rendered.contains("affected entries (2)"));
        assert!(rendered.contains("service"));
        assert!(rendered.contains("entry1 [entry] [function] (a.rs)"));
        assert!(rendered.contains("entry2 [entry] [function] (b.rs)"));
    }

    #[test]
    fn impact_renders_summary_and_dependency_tree() {
        let tree = ImpactTreeNode {
            symbol: symbol_ref("source", NodeKind::Function, "core.rs"),
            children: vec![ImpactTreeNode {
                symbol: symbol_ref("alpha", NodeKind::Function, "a.rs"),
                children: vec![ImpactTreeNode {
                    symbol: symbol_ref("beta", NodeKind::Function, "b.rs"),
                    children: Vec::new(),
                }],
            }],
        };

        let result = ImpactResult {
            source: "core.rs::source".to_string(),
            depth_1: vec![symbol_ref("alpha", NodeKind::Function, "a.rs")],
            depth_2: vec![symbol_ref("beta", NodeKind::Function, "b.rs")],
            depth_3_plus: Vec::new(),
            total_affected: 2,
            source_ref: symbol_ref("source", NodeKind::Function, "core.rs"),
            tree,
        };

        let rendered = render_impact(&result);
        assert!(rendered.contains("source [function] (core.rs)"));
        assert!(rendered.contains("summary: depth_1=1, depth_2=1, depth_3_plus=0, total=2"));
        assert!(rendered.contains("dependents (2)"));
        assert!(rendered.contains("alpha [function] (a.rs)"));
        assert!(rendered.contains("beta [function] (b.rs)"));
    }
}
