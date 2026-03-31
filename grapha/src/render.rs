use std::collections::{BTreeMap, BTreeSet};

use grapha_core::graph::NodeKind;

use crate::query::{
    ContextResult, SymbolInfo, SymbolRef, SymbolTreeRef, dataflow::DataflowEdge,
    dataflow::DataflowEdgeKind, dataflow::DataflowNode, dataflow::DataflowNodeKind,
    dataflow::DataflowResult, entries::EntriesResult, impact::ImpactResult, impact::ImpactTreeNode,
    reverse::AffectedEntry, reverse::ReverseResult, trace::Flow, trace::TraceResult,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderOptions {
    color_enabled: bool,
}

impl RenderOptions {
    pub const fn plain() -> Self {
        Self {
            color_enabled: false,
        }
    }

    pub const fn color() -> Self {
        Self {
            color_enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Palette {
    enabled: bool,
}

impl Palette {
    fn new(options: RenderOptions) -> Self {
        Self {
            enabled: options.color_enabled,
        }
    }

    fn paint(self, sgr: &str, text: impl AsRef<str>) -> String {
        let text = text.as_ref();
        if self.enabled {
            format!("\x1b[{sgr}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn symbol_name(self, text: impl AsRef<str>) -> String {
        self.paint("1;97", text)
    }

    fn section_header(self, text: impl AsRef<str>) -> String {
        self.paint("1;36", text)
    }

    fn tag(self, text: impl AsRef<str>) -> String {
        self.paint("33", text)
    }

    fn file(self, text: impl AsRef<str>) -> String {
        self.paint("2;34", text)
    }

    fn key(self, text: impl AsRef<str>) -> String {
        self.paint("35", text)
    }

    fn number(self, value: impl std::fmt::Display) -> String {
        self.paint("32", value.to_string())
    }
}

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

fn format_name_kind_file(name: &str, kind: NodeKind, file: &str, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!(
        "{} {} {}",
        palette.symbol_name(name),
        palette.tag(format!("[{}]", kind_label(kind))),
        palette.file(format!("({file})")),
    )
}

fn format_symbol_info(symbol: &SymbolInfo, options: RenderOptions) -> String {
    format_name_kind_file(&symbol.name, symbol.kind, &symbol.file, options)
}

fn format_symbol_ref(symbol: &SymbolRef, options: RenderOptions) -> String {
    format_name_kind_file(&symbol.name, symbol.kind, &symbol.file, options)
}

fn format_symbol_tree_ref(symbol: &SymbolTreeRef, options: RenderOptions) -> String {
    format_name_kind_file(&symbol.name, symbol.kind, &symbol.file, options)
}

fn format_section_count(label: &str, count: usize, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!(
        "{} ({})",
        palette.section_header(label),
        palette.number(count),
    )
}

fn format_summary(parts: &[(&str, String)], options: RenderOptions) -> String {
    let palette = Palette::new(options);
    let rendered = parts
        .iter()
        .map(|(key, value)| format!("{key}={}", palette.number(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}: {rendered}", palette.section_header("summary"))
}

fn format_key_value(key: &str, value: &str, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!("{}: {value}", palette.key(key))
}

fn format_key_number(key: &str, value: usize, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!("{}: {}", palette.key(key), palette.number(value))
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

fn symbol_tree_ref_to_tree_node(symbol: &SymbolTreeRef, options: RenderOptions) -> TreeNode {
    TreeNode::branch(
        format_symbol_tree_ref(symbol, options),
        symbol
            .contains
            .iter()
            .map(|child| symbol_tree_ref_to_tree_node(child, options))
            .collect(),
    )
}

fn push_symbol_section(
    children: &mut Vec<TreeNode>,
    label: &str,
    symbols: &[SymbolRef],
    options: RenderOptions,
) {
    if symbols.is_empty() {
        return;
    }

    children.push(TreeNode::branch(
        format_section_count(label, symbols.len(), options),
        symbols
            .iter()
            .map(|symbol| TreeNode::leaf(format_symbol_ref(symbol, options)))
            .collect(),
    ));
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

fn impact_tree_to_tree_node(node: &ImpactTreeNode, options: RenderOptions) -> TreeNode {
    TreeNode::branch(
        format_symbol_ref(&node.symbol, options),
        node.children
            .iter()
            .map(|child| impact_tree_to_tree_node(child, options))
            .collect(),
    )
}

fn format_trace_terminal(flow: &Flow, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    let last_segment = flow
        .path
        .last()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    match &flow.terminal {
        Some(terminal) => format!(
            "{} {}",
            palette.symbol_name(last_segment),
            palette.tag(format!(
                "[terminal:{} {} {}]",
                terminal.kind, terminal.direction, terminal.operation
            ))
        ),
        None => palette.symbol_name(last_segment),
    }
}

fn insert_trace_flow(tree: &mut PathMergeNode, flow: &Flow, options: RenderOptions) {
    let palette = Palette::new(options);
    if flow.path.len() < 2 {
        return;
    }

    let mut segments: Vec<String> = flow
        .path
        .iter()
        .skip(1)
        .map(|segment| palette.symbol_name(segment))
        .collect();
    if let Some(last) = segments.last_mut() {
        *last = format_trace_terminal(flow, options);
    }

    let leaf = tree.insert_path(segments);
    for condition in &flow.conditions {
        leaf.notes
            .insert(format_key_value("condition", condition, options));
    }
    for boundary in &flow.async_boundaries {
        leaf.notes
            .insert(format_key_value("async", boundary, options));
    }
}

fn reverse_root_label(result: &ReverseResult, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    let mut label = format_symbol_ref(&result.target_ref, options);
    if result
        .affected_entries
        .iter()
        .any(|entry| entry.distance == 0)
    {
        label.push(' ');
        label.push_str(&palette.tag("[entry]"));
    }
    label
}

fn reverse_leaf_label(entry: &AffectedEntry, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!(
        "{} {} {} {}",
        palette.symbol_name(&entry.entry.name),
        palette.tag("[entry]"),
        palette.tag(format!("[{}]", kind_label(entry.entry.kind))),
        palette.file(format!("({})", entry.entry.file)),
    )
}

#[allow(dead_code)]
pub fn render_context(result: &ContextResult) -> String {
    render_context_with_options(result, RenderOptions::plain())
}

pub fn render_context_with_options(result: &ContextResult, options: RenderOptions) -> String {
    let mut children = Vec::new();

    push_symbol_section(&mut children, "callers", &result.callers, options);
    push_symbol_section(&mut children, "callees", &result.callees, options);

    if !result.contains_tree.is_empty() {
        children.push(TreeNode::branch(
            format_section_count("contains", result.contains_tree.len(), options),
            result
                .contains_tree
                .iter()
                .map(|symbol| symbol_tree_ref_to_tree_node(symbol, options))
                .collect(),
        ));
    }

    push_symbol_section(&mut children, "contained_by", &result.contained_by, options);
    push_symbol_section(&mut children, "implementors", &result.implementors, options);
    push_symbol_section(&mut children, "implements", &result.implements, options);
    push_symbol_section(&mut children, "type_refs", &result.type_refs, options);

    render_tree(&TreeNode::branch(
        format_symbol_info(&result.symbol, options),
        children,
    ))
}

#[allow(dead_code)]
pub fn render_entries(result: &EntriesResult) -> String {
    render_entries_with_options(result, RenderOptions::plain())
}

pub fn render_entries_with_options(result: &EntriesResult, options: RenderOptions) -> String {
    let children = sorted_symbol_refs(&result.entries)
        .into_iter()
        .map(|entry| TreeNode::leaf(format_symbol_ref(&entry, options)))
        .collect();

    render_tree(&TreeNode::branch(
        format_section_count("entry points", result.total, options),
        children,
    ))
}

#[allow(dead_code)]
pub fn render_trace(result: &TraceResult) -> String {
    render_trace_with_options(result, RenderOptions::plain())
}

pub fn render_trace_with_options(result: &TraceResult, options: RenderOptions) -> String {
    let mut flows = PathMergeNode::default();
    for flow in &result.flows {
        insert_trace_flow(&mut flows, flow, options);
    }

    let root = TreeNode::branch(
        format_symbol_ref(&result.entry_ref, options),
        vec![
            TreeNode::leaf(format_summary(
                &[
                    ("flows", result.summary.total_flows.to_string()),
                    ("reads", result.summary.reads.to_string()),
                    ("writes", result.summary.writes.to_string()),
                    (
                        "async_crossings",
                        result.summary.async_crossings.to_string(),
                    ),
                ],
                options,
            )),
            TreeNode::branch(
                format_section_count("flows", result.summary.total_flows, options),
                flows.into_tree_children(),
            ),
        ],
    );

    render_tree(&root)
}

fn dataflow_edge_kind_label(kind: DataflowEdgeKind) -> &'static str {
    match kind {
        DataflowEdgeKind::Call => "call",
        DataflowEdgeKind::Read => "read",
        DataflowEdgeKind::Write => "write",
        DataflowEdgeKind::Publish => "publish",
        DataflowEdgeKind::Subscribe => "subscribe",
    }
}

fn dataflow_node_label(node: &DataflowNode, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    match node.kind {
        DataflowNodeKind::Entry => format!(
            "{} {} {}",
            palette.symbol_name(&node.name),
            palette.tag("[entry]"),
            palette.file(format!("({})", node.file.as_deref().unwrap_or("unknown")))
        ),
        DataflowNodeKind::Symbol => format!(
            "{} {} {}",
            palette.symbol_name(&node.name),
            palette.tag("[symbol]"),
            palette.file(format!("({})", node.file.as_deref().unwrap_or("unknown")))
        ),
        DataflowNodeKind::Effect => format!(
            "{} {}",
            palette.symbol_name(&node.name),
            palette.tag(format!(
                "[effect:{}]",
                node.effect_kind.as_deref().unwrap_or("unknown")
            ))
        ),
    }
}

fn dataflow_edge_label(
    edge: &DataflowEdge,
    target: &DataflowNode,
    options: RenderOptions,
) -> String {
    let palette = Palette::new(options);
    format!(
        "{} -> {}",
        palette.key(dataflow_edge_kind_label(edge.kind)),
        dataflow_node_label(target, options)
    )
}

fn render_dataflow_children(
    current: &str,
    adjacency: &BTreeMap<String, Vec<DataflowEdge>>,
    node_index: &BTreeMap<String, DataflowNode>,
    visited: &mut BTreeSet<String>,
    options: RenderOptions,
) -> Vec<TreeNode> {
    let mut children = Vec::new();
    let mut edges = adjacency.get(current).cloned().unwrap_or_default();
    edges.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.operation.cmp(&right.operation))
    });

    for edge in edges {
        let Some(target) = node_index.get(&edge.target) else {
            continue;
        };
        let mut edge_children = Vec::new();
        if let Some(operation) = edge.operation.as_deref() {
            edge_children.push(TreeNode::leaf(format_key_value(
                "operation",
                operation,
                options,
            )));
        }
        for condition in &edge.conditions {
            edge_children.push(TreeNode::leaf(format_key_value(
                "condition",
                condition,
                options,
            )));
        }
        if edge.async_boundary == Some(true) {
            edge_children.push(TreeNode::leaf(format_key_value(
                "async_boundary",
                "true",
                options,
            )));
        }
        if !edge.provenance.is_empty() {
            edge_children.push(TreeNode::leaf(format_key_number(
                "provenance",
                edge.provenance.len(),
                options,
            )));
        }

        if target.kind != DataflowNodeKind::Effect {
            if visited.insert(target.id.clone()) {
                edge_children.extend(render_dataflow_children(
                    &target.id, adjacency, node_index, visited, options,
                ));
                visited.remove(&target.id);
            } else {
                edge_children.push(TreeNode::leaf("cycle"));
            }
        }

        children.push(TreeNode::branch(
            dataflow_edge_label(&edge, target, options),
            edge_children,
        ));
    }

    children
}

#[allow(dead_code)]
pub fn render_dataflow(result: &DataflowResult) -> String {
    render_dataflow_with_options(result, RenderOptions::plain())
}

pub fn render_dataflow_with_options(result: &DataflowResult, options: RenderOptions) -> String {
    let node_index: BTreeMap<String, DataflowNode> = result
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut adjacency: BTreeMap<String, Vec<DataflowEdge>> = BTreeMap::new();
    for edge in &result.edges {
        adjacency
            .entry(edge.source.clone())
            .or_default()
            .push(edge.clone());
    }

    let mut visited = BTreeSet::new();
    visited.insert(result.entry.clone());

    let root = TreeNode::branch(
        format_symbol_ref(&result.entry_ref, options),
        vec![
            TreeNode::leaf(format_summary(
                &[
                    ("symbols", result.summary.symbols.to_string()),
                    ("effects", result.summary.effects.to_string()),
                    ("edges", result.summary.edges.to_string()),
                    ("calls", result.summary.calls.to_string()),
                    ("reads", result.summary.reads.to_string()),
                    ("writes", result.summary.writes.to_string()),
                    ("publishes", result.summary.publishes.to_string()),
                    ("subscribes", result.summary.subscribes.to_string()),
                ],
                options,
            )),
            TreeNode::branch(
                Palette::new(options).section_header("graph"),
                render_dataflow_children(
                    &result.entry,
                    &adjacency,
                    &node_index,
                    &mut visited,
                    options,
                ),
            ),
        ],
    );

    render_tree(&root)
}

#[allow(dead_code)]
pub fn render_reverse(result: &ReverseResult) -> String {
    render_reverse_with_options(result, RenderOptions::plain())
}

pub fn render_reverse_with_options(result: &ReverseResult, options: RenderOptions) -> String {
    let mut tree = PathMergeNode::default();
    let palette = Palette::new(options);

    for affected in &result.affected_entries {
        let reversed: Vec<String> = affected.path.iter().rev().cloned().collect();
        if reversed.len() <= 1 {
            continue;
        }

        let mut segments: Vec<String> = reversed
            .into_iter()
            .skip(1)
            .map(|segment| palette.symbol_name(segment))
            .collect();
        if let Some(last) = segments.last_mut() {
            *last = reverse_leaf_label(affected, options);
        }
        tree.insert_path(segments);
    }

    let root = TreeNode::branch(
        reverse_root_label(result, options),
        vec![TreeNode::branch(
            format_section_count("affected entries", result.total_entries, options),
            tree.into_tree_children(),
        )],
    );

    render_tree(&root)
}

#[allow(dead_code)]
pub fn render_impact(result: &ImpactResult) -> String {
    render_impact_with_options(result, RenderOptions::plain())
}

pub fn render_impact_with_options(result: &ImpactResult, options: RenderOptions) -> String {
    let dependents = result
        .tree
        .children
        .iter()
        .map(|node| impact_tree_to_tree_node(node, options))
        .collect();

    let root = TreeNode::branch(
        format_symbol_ref(&result.source_ref, options),
        vec![
            TreeNode::leaf(format_summary(
                &[
                    ("depth_1", result.depth_1.len().to_string()),
                    ("depth_2", result.depth_2.len().to_string()),
                    ("depth_3_plus", result.depth_3_plus.len().to_string()),
                    ("total", result.total_affected.to_string()),
                ],
                options,
            )),
            TreeNode::branch(
                format_section_count("dependents", result.total_affected, options),
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
        ContextResult, SymbolInfo, SymbolRef, SymbolTreeRef, dataflow::DataflowEdge,
        dataflow::DataflowEdgeKind, dataflow::DataflowNode, dataflow::DataflowNodeKind,
        dataflow::DataflowResult, dataflow::DataflowSummary, entries::EntriesResult,
        impact::ImpactResult, impact::ImpactTreeNode, reverse::AffectedEntry,
        reverse::ReverseResult, trace::Flow, trace::TerminalInfo, trace::TraceResult,
        trace::TraceSummary,
    };

    use super::*;

    fn strip_ansi(input: &str) -> String {
        let mut stripped = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
            stripped.push(ch);
        }
        stripped
    }

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
            contains: Vec::new(),
            contains_tree: Vec::new(),
            contained_by: Vec::new(),
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
    fn context_renders_structural_sections() {
        let result = ContextResult {
            symbol: symbol_info("body", NodeKind::Property, "ContentView.swift"),
            callers: Vec::new(),
            callees: Vec::new(),
            contains: vec![symbol_ref("VStack", NodeKind::View, "ContentView.swift")],
            contains_tree: vec![SymbolTreeRef {
                id: "ContentView.swift::body::VStack".into(),
                name: "VStack".into(),
                kind: NodeKind::View,
                file: "ContentView.swift".into(),
                contains: vec![
                    SymbolTreeRef {
                        id: "ContentView.swift::body::Text".into(),
                        name: "Text".into(),
                        kind: NodeKind::View,
                        file: "ContentView.swift".into(),
                        contains: Vec::new(),
                    },
                    SymbolTreeRef {
                        id: "ContentView.swift::body::Row".into(),
                        name: "Row".into(),
                        kind: NodeKind::View,
                        file: "ContentView.swift".into(),
                        contains: Vec::new(),
                    },
                ],
            }],
            contained_by: vec![symbol_ref(
                "ContentView",
                NodeKind::Struct,
                "ContentView.swift",
            )],
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let rendered = render_context(&result);
        assert!(rendered.contains("contains (1)"));
        assert!(rendered.contains("├── contains (1)"));
        assert!(rendered.contains("│   └── VStack [view] (ContentView.swift)"));
        assert!(rendered.contains("│       ├── Text [view] (ContentView.swift)"));
        assert!(rendered.contains("│       └── Row [view] (ContentView.swift)"));
        assert!(rendered.contains("contained_by (1)"));
        assert!(rendered.contains("ContentView [struct] (ContentView.swift)"));
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

    #[test]
    fn colorized_context_highlights_symbol_sections_and_files() {
        let result = ContextResult {
            symbol: symbol_info("helper", NodeKind::Function, "main.rs"),
            callers: vec![symbol_ref("main", NodeKind::Function, "main.rs")],
            callees: Vec::new(),
            contains: Vec::new(),
            contains_tree: Vec::new(),
            contained_by: Vec::new(),
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let plain = render_context(&result);
        let rendered = render_context_with_options(&result, RenderOptions::color());

        assert!(rendered.contains("\x1b[1;97mhelper\x1b[0m"));
        assert!(rendered.contains("\x1b[33m[function]\x1b[0m"));
        assert!(rendered.contains("\x1b[2;34m(main.rs)\x1b[0m"));
        assert!(rendered.contains("\x1b[1;36mcallers\x1b[0m"));
        assert!(rendered.contains("\x1b[32m1\x1b[0m"));
        assert_eq!(strip_ansi(&rendered), plain);
    }

    #[test]
    fn colorized_dataflow_highlights_edge_labels_and_summary() {
        let result = DataflowResult {
            entry: "main.rs::handler".to_string(),
            nodes: vec![DataflowNode {
                id: "effect::persist".to_string(),
                name: "UPSERT persist".to_string(),
                kind: DataflowNodeKind::Effect,
                file: None,
                effect_kind: Some("persistence".to_string()),
                operation: Some("UPSERT".to_string()),
                target: Some("persist".to_string()),
            }],
            edges: vec![DataflowEdge {
                source: "main.rs::handler".to_string(),
                target: "effect::persist".to_string(),
                kind: DataflowEdgeKind::Read,
                operation: Some("UPSERT".to_string()),
                conditions: vec!["user.isAdmin".to_string()],
                async_boundary: Some(true),
                provenance: vec![],
            }],
            entry_ref: symbol_ref("handler", NodeKind::Function, "main.rs"),
            summary: DataflowSummary {
                symbols: 0,
                effects: 1,
                edges: 1,
                calls: 0,
                reads: 1,
                writes: 0,
                publishes: 0,
                subscribes: 0,
            },
        };

        let rendered = render_dataflow_with_options(&result, RenderOptions::color());
        assert!(rendered.contains("\x1b[1;36msummary\x1b[0m"));
        assert!(rendered.contains("symbols=\x1b[32m0\x1b[0m"));
        assert!(rendered.contains("\x1b[35mread\x1b[0m ->"));
        assert!(rendered.contains("\x1b[33m[effect:persistence]\x1b[0m"));
        assert!(rendered.contains("\x1b[35moperation\x1b[0m: UPSERT"));
        assert!(rendered.contains("\x1b[35mcondition\x1b[0m: user.isAdmin"));
    }
}
