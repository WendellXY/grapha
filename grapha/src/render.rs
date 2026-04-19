use std::collections::{BTreeMap, BTreeSet};

use grapha_core::graph::{NodeKind, NodeRole, Visibility};

use crate::concepts::{
    ConceptBindingView, ConceptEvidence, ConceptSearchResult, ConceptShowResult,
};
use crate::fields::FieldSet;
use crate::query::{
    ContextResult, SymbolInfo, SymbolRef, SymbolTreeRef, dataflow::DataflowEdge,
    dataflow::DataflowEdgeKind, dataflow::DataflowNode, dataflow::DataflowNodeKind,
    dataflow::DataflowResult, entries::EntriesResult, impact::ImpactResult, impact::ImpactTreeNode,
    localize::LocalizeResult, origin::OriginPath, origin::OriginResult, origin::OriginSnippet,
    reverse::AffectedEntry, reverse::ReverseResult, trace::Flow, trace::TraceResult,
    usages::UsagesResult,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderOptions {
    color_enabled: bool,
    pub fields: FieldSet,
}

impl RenderOptions {
    pub const fn plain() -> Self {
        Self {
            color_enabled: false,
            fields: FieldSet {
                file: true,
                id: false,
                locator: false,
                module: false,
                span: false,
                snippet: false,
                visibility: false,
                signature: false,
                role: false,
            },
        }
    }

    pub const fn color() -> Self {
        Self {
            color_enabled: true,
            fields: FieldSet {
                file: true,
                id: false,
                locator: false,
                module: false,
                span: false,
                snippet: false,
                visibility: false,
                signature: false,
                role: false,
            },
        }
    }

    pub fn with_fields(self, fields: FieldSet) -> Self {
        Self { fields, ..self }
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
        // Keep primary text on the terminal's default foreground so light and
        // dark themes both stay readable.
        self.paint("1", text)
    }

    fn section_header(self, text: impl AsRef<str>) -> String {
        self.paint("1;36", text)
    }

    fn tag(self, text: impl AsRef<str>) -> String {
        self.paint("33", text)
    }

    fn file(self, text: impl AsRef<str>) -> String {
        text.as_ref().to_string()
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
        children.extend(self.notes.into_iter().map(TreeNode::leaf));
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

fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Crate => "crate",
        Visibility::Private => "private",
    }
}

fn role_label(role: &NodeRole) -> String {
    match role {
        NodeRole::EntryPoint => "entry_point".to_string(),
        NodeRole::Terminal { kind } => format!(
            "terminal:{}",
            serde_json::to_string(kind)
                .unwrap_or_else(|_| format!("{kind:?}"))
                .trim_matches('"')
        ),
        NodeRole::Internal => "internal".to_string(),
    }
}

fn format_span(span: [usize; 2]) -> String {
    if span[0] == span[1] {
        span[0].to_string()
    } else {
        format!("{}..{}", span[0], span[1])
    }
}

fn format_key(key: &str, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    palette.key(key)
}

fn format_file_suffix(file: Option<&str>, options: RenderOptions) -> String {
    if !options.fields.file {
        return String::new();
    }

    let Some(file) = file else {
        return String::new();
    };

    format!(" {}", Palette::new(options).file(format!("({file})")))
}

fn format_name_kind_file(name: &str, kind: NodeKind, file: &str, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    let mut label = format!(
        "{} {}",
        palette.symbol_name(name),
        palette.tag(format!("[{}]", kind_label(kind))),
    );
    label.push_str(&format_file_suffix(Some(file), options));
    label
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

fn push_detail(
    children: &mut Vec<TreeNode>,
    key: &str,
    value: Option<String>,
    options: RenderOptions,
) {
    if let Some(value) = value {
        if value.contains('\n') {
            children.push(TreeNode::branch(
                format_key(key, options),
                value
                    .lines()
                    .map(|line| TreeNode::leaf(line.to_string()))
                    .collect(),
            ));
        } else {
            children.push(TreeNode::leaf(format_key_value(key, &value, options)));
        }
    }
}

fn symbol_info_details(symbol: &SymbolInfo, options: RenderOptions) -> Vec<TreeNode> {
    let mut children = Vec::new();
    let fields = options.fields;

    if fields.id {
        push_detail(&mut children, "id", Some(symbol.id.clone()), options);
    }
    if fields.locator {
        push_detail(&mut children, "locator", symbol.locator.clone(), options);
    }
    if fields.module {
        push_detail(&mut children, "module", symbol.module.clone(), options);
    }
    if fields.span {
        push_detail(
            &mut children,
            "span",
            Some(format_span(symbol.span)),
            options,
        );
    }
    if fields.visibility {
        push_detail(
            &mut children,
            "visibility",
            symbol
                .visibility
                .map(|value| visibility_label(value).to_string()),
            options,
        );
    }
    if fields.signature {
        push_detail(
            &mut children,
            "signature",
            symbol.signature.clone(),
            options,
        );
    }
    if fields.role {
        push_detail(
            &mut children,
            "role",
            symbol.role.as_ref().map(role_label),
            options,
        );
    }
    if fields.snippet {
        push_detail(&mut children, "snippet", symbol.snippet.clone(), options);
    }

    children
}

fn symbol_ref_details(symbol: &SymbolRef, options: RenderOptions) -> Vec<TreeNode> {
    let mut children = Vec::new();
    let fields = options.fields;

    if fields.id {
        push_detail(&mut children, "id", Some(symbol.id.clone()), options);
    }
    if fields.locator {
        push_detail(&mut children, "locator", symbol.locator.clone(), options);
    }
    if fields.module {
        push_detail(&mut children, "module", symbol.module.clone(), options);
    }
    if fields.span {
        push_detail(&mut children, "span", symbol.span.map(format_span), options);
    }
    if fields.visibility {
        push_detail(
            &mut children,
            "visibility",
            symbol
                .visibility
                .map(|value| visibility_label(value).to_string()),
            options,
        );
    }
    if fields.signature {
        push_detail(
            &mut children,
            "signature",
            symbol.signature.clone(),
            options,
        );
    }
    if fields.role {
        push_detail(
            &mut children,
            "role",
            symbol.role.as_ref().map(role_label),
            options,
        );
    }
    if fields.snippet {
        push_detail(&mut children, "snippet", symbol.snippet.clone(), options);
    }

    children
}

fn symbol_tree_ref_details(symbol: &SymbolTreeRef, options: RenderOptions) -> Vec<TreeNode> {
    let mut children = Vec::new();
    let fields = options.fields;

    if fields.id {
        push_detail(&mut children, "id", Some(symbol.id.clone()), options);
    }
    if fields.locator {
        push_detail(&mut children, "locator", symbol.locator.clone(), options);
    }
    if fields.module {
        push_detail(&mut children, "module", symbol.module.clone(), options);
    }
    if fields.span {
        push_detail(&mut children, "span", symbol.span.map(format_span), options);
    }
    if fields.visibility {
        push_detail(
            &mut children,
            "visibility",
            symbol
                .visibility
                .map(|value| visibility_label(value).to_string()),
            options,
        );
    }
    if fields.signature {
        push_detail(
            &mut children,
            "signature",
            symbol.signature.clone(),
            options,
        );
    }
    if fields.role {
        push_detail(
            &mut children,
            "role",
            symbol.role.as_ref().map(role_label),
            options,
        );
    }
    if fields.snippet {
        push_detail(&mut children, "snippet", symbol.snippet.clone(), options);
    }

    children
}

fn tree_node(label: String, children: Vec<TreeNode>) -> TreeNode {
    if children.is_empty() {
        TreeNode::leaf(label)
    } else {
        TreeNode::branch(label, children)
    }
}

fn symbol_info_node(
    symbol: &SymbolInfo,
    mut children: Vec<TreeNode>,
    options: RenderOptions,
) -> TreeNode {
    let mut detail_children = symbol_info_details(symbol, options);
    detail_children.append(&mut children);
    tree_node(format_symbol_info(symbol, options), detail_children)
}

fn symbol_ref_node(
    symbol: &SymbolRef,
    mut children: Vec<TreeNode>,
    options: RenderOptions,
) -> TreeNode {
    let mut detail_children = symbol_ref_details(symbol, options);
    detail_children.append(&mut children);
    tree_node(format_symbol_ref(symbol, options), detail_children)
}

fn format_section_count(label: &str, count: usize, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!(
        "{} ({})",
        palette.section_header(label),
        palette.number(count),
    )
}

fn format_section_progress(
    label: &str,
    shown: usize,
    total: usize,
    options: RenderOptions,
) -> String {
    let palette = Palette::new(options);
    format!(
        "{} ({} shown / {} total)",
        palette.section_header(label),
        palette.number(shown),
        palette.number(total),
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

fn symbol_tree_ref_to_tree_node(symbol: &SymbolTreeRef, options: RenderOptions) -> TreeNode {
    let mut children = symbol_tree_ref_details(symbol, options);
    children.extend(
        symbol
            .contains
            .iter()
            .map(|child| symbol_tree_ref_to_tree_node(child, options)),
    );
    tree_node(format_symbol_tree_ref(symbol, options), children)
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
            .map(|symbol| symbol_ref_node(symbol, Vec::new(), options))
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
    symbol_ref_node(
        &node.symbol,
        node.children
            .iter()
            .map(|child| impact_tree_to_tree_node(child, options))
            .collect(),
        options,
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
    let mut label = format!(
        "{} {} {}",
        palette.symbol_name(&entry.entry.name),
        palette.tag("[entry]"),
        palette.tag(format!("[{}]", kind_label(entry.entry.kind))),
    );
    label.push_str(&format_file_suffix(Some(&entry.entry.file), options));
    label
}

pub fn render_context_with_options(result: &ContextResult, options: RenderOptions) -> String {
    let mut children = Vec::new();

    push_symbol_section(&mut children, "callers", &result.callers, options);
    push_symbol_section(&mut children, "callees", &result.callees, options);
    push_symbol_section(&mut children, "reads", &result.reads, options);
    push_symbol_section(&mut children, "read_by", &result.read_by, options);
    push_symbol_section(
        &mut children,
        "invalidation_sources",
        &result.invalidation_sources,
        options,
    );

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

    render_tree(&symbol_info_node(&result.symbol, children, options))
}

pub fn render_entries_with_options(result: &EntriesResult, options: RenderOptions) -> String {
    let children = result
        .entries
        .iter()
        .map(|entry| TreeNode::leaf(format_symbol_ref(entry, options)))
        .collect();

    render_tree(&TreeNode::branch(
        format_section_progress("entry points", result.shown, result.total, options),
        children,
    ))
}

pub fn render_localize_with_options(result: &LocalizeResult, options: RenderOptions) -> String {
    let mut children = Vec::new();
    children.push(TreeNode::leaf(format_summary(
        &[
            ("matches", result.matches.len().to_string()),
            ("unmatched", result.unmatched.len().to_string()),
        ],
        options,
    )));

    if !result.matches.is_empty() {
        let match_nodes = result
            .matches
            .iter()
            .map(|item| {
                let mut item_children = Vec::new();
                if !item.ui_path.is_empty() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "ui_path",
                        &item.ui_path.join(" -> "),
                        options,
                    )));
                }
                if let Some(wrapper_name) = item.reference.wrapper_name.as_deref() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "wrapper",
                        wrapper_name,
                        options,
                    )));
                }
                item_children.push(TreeNode::leaf(format_key_value(
                    "record",
                    &format!(
                        "{}.{}{}",
                        item.record.table,
                        item.record.key,
                        format_file_suffix(Some(&item.record.catalog_file), options)
                    ),
                    options,
                )));
                item_children.push(TreeNode::leaf(format_key_value(
                    "source_value",
                    &item.record.source_value,
                    options,
                )));
                item_children.push(TreeNode::leaf(format_key_value(
                    "status",
                    &item.record.status,
                    options,
                )));
                if let Some(comment) = item.record.comment.as_deref() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "comment", comment, options,
                    )));
                }
                TreeNode::branch(format_symbol_info(&item.view, options), item_children)
            })
            .collect();

        children.push(TreeNode::branch(
            format_section_count("matches", result.matches.len(), options),
            match_nodes,
        ));
    }

    if !result.unmatched.is_empty() {
        let unmatched_nodes = result
            .unmatched
            .iter()
            .map(|item| {
                let mut item_children = Vec::new();
                if !item.ui_path.is_empty() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "ui_path",
                        &item.ui_path.join(" -> "),
                        options,
                    )));
                }
                if let Some(wrapper_name) = item.reference.wrapper_name.as_deref() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "wrapper",
                        wrapper_name,
                        options,
                    )));
                }
                if let Some(literal) = item.reference.literal.as_deref() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "literal", literal, options,
                    )));
                }
                item_children.push(TreeNode::leaf(format_key_value(
                    "reason",
                    &item.reason,
                    options,
                )));
                TreeNode::branch(format_symbol_info(&item.view, options), item_children)
            })
            .collect();

        children.push(TreeNode::branch(
            format_section_count("unmatched", result.unmatched.len(), options),
            unmatched_nodes,
        ));
    }

    render_tree(&TreeNode::branch(
        format_symbol_info(&result.symbol, options),
        children,
    ))
}

pub fn render_usages_with_options(result: &UsagesResult, options: RenderOptions) -> String {
    let mut children = Vec::new();
    children.push(TreeNode::leaf(format_summary(
        &[
            ("records", result.records.len().to_string()),
            (
                "usages",
                result
                    .records
                    .iter()
                    .map(|record| record.usages.len())
                    .sum::<usize>()
                    .to_string(),
            ),
        ],
        options,
    )));

    for record in &result.records {
        let usage_children = record
            .usages
            .iter()
            .map(|usage| {
                let mut item_children = Vec::new();
                item_children.push(TreeNode::leaf(format_key_value(
                    "owner",
                    &usage.owner.name,
                    options,
                )));
                if !usage.ui_path.is_empty() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "ui_path",
                        &usage.ui_path.join(" -> "),
                        options,
                    )));
                }
                if let Some(wrapper_name) = usage.reference.wrapper_name.as_deref() {
                    item_children.push(TreeNode::leaf(format_key_value(
                        "wrapper",
                        wrapper_name,
                        options,
                    )));
                }
                TreeNode::branch(format_symbol_info(&usage.view, options), item_children)
            })
            .collect();
        children.push(TreeNode::branch(
            format!(
                "{}{}",
                Palette::new(options)
                    .symbol_name(format!("{}.{}", record.record.table, record.record.key)),
                format_file_suffix(Some(&record.record.catalog_file), options)
            ),
            usage_children,
        ));
    }

    render_tree(&TreeNode::branch(
        Palette::new(options).section_header(format!("usages for {}", result.query.key)),
        children,
    ))
}

fn concept_evidence_node(evidence: &ConceptEvidence, options: RenderOptions) -> TreeNode {
    let palette = Palette::new(options);
    let mut children = vec![TreeNode::leaf(format_key_value(
        "match_kind",
        &evidence.match_kind,
        options,
    ))];
    push_detail(&mut children, "table", evidence.table.clone(), options);
    push_detail(&mut children, "key", evidence.key.clone(), options);
    push_detail(
        &mut children,
        "source_value",
        evidence.source_value.clone(),
        options,
    );
    if !evidence.ui_path.is_empty() {
        children.push(TreeNode::leaf(format_key_value(
            "ui_path",
            &evidence.ui_path.join(" -> "),
            options,
        )));
    }
    push_detail(&mut children, "note", evidence.note.clone(), options);
    TreeNode::branch(
        format!(
            "{} {}",
            palette.tag(format!("[{}]", evidence.kind)),
            evidence.value
        ),
        children,
    )
}

fn concept_binding_node(binding: &ConceptBindingView, options: RenderOptions) -> TreeNode {
    let mut children = vec![
        TreeNode::leaf(format_key_value("status", &binding.status, options)),
        TreeNode::leaf(format_key_value(
            "stale",
            if binding.stale { "true" } else { "false" },
            options,
        )),
    ];
    if !binding.evidence.is_empty() {
        children.push(TreeNode::branch(
            format_section_count("evidence", binding.evidence.len(), options),
            binding
                .evidence
                .iter()
                .map(|evidence| concept_evidence_node(evidence, options))
                .collect(),
        ));
    }
    match &binding.symbol {
        Some(symbol) => symbol_info_node(symbol, children, options),
        None => TreeNode::branch(
            Palette::new(options).symbol_name(&binding.symbol_id),
            children,
        ),
    }
}

pub fn render_concept_search_with_options(
    result: &ConceptSearchResult,
    options: RenderOptions,
) -> String {
    let mut children = vec![
        TreeNode::leaf(format_key_value(
            "resolved_from",
            &result.resolved_from,
            options,
        )),
        TreeNode::leaf(format_summary(
            &[("scopes", result.scopes.len().to_string())],
            options,
        )),
    ];
    if let Some(concept) = result.matched_concept.as_deref() {
        children.push(TreeNode::leaf(format_key_value(
            "matched_concept",
            concept,
            options,
        )));
    }
    if !result.scopes.is_empty() {
        children.push(TreeNode::branch(
            format_section_count("scopes", result.scopes.len(), options),
            result
                .scopes
                .iter()
                .map(|scope| {
                    let mut scope_children = vec![
                        TreeNode::leaf(format_key_value("status", &scope.status, options)),
                        TreeNode::leaf(format_key_value(
                            "score",
                            &format!("{:.1}", scope.score),
                            options,
                        )),
                    ];
                    if !scope.evidence.is_empty() {
                        scope_children.push(TreeNode::branch(
                            format_section_count("evidence", scope.evidence.len(), options),
                            scope
                                .evidence
                                .iter()
                                .map(|evidence| concept_evidence_node(evidence, options))
                                .collect(),
                        ));
                    }
                    symbol_info_node(&scope.symbol, scope_children, options)
                })
                .collect(),
        ));
    }

    render_tree(&TreeNode::branch(
        Palette::new(options).section_header(format!("concept search {}", result.query)),
        children,
    ))
}

pub fn render_concept_show_with_options(
    result: &ConceptShowResult,
    options: RenderOptions,
) -> String {
    let mut children = vec![TreeNode::leaf(format_key_value(
        "query",
        &result.query,
        options,
    ))];
    if !result.aliases.is_empty() {
        children.push(TreeNode::leaf(format_key_value(
            "aliases",
            &result.aliases.join(", "),
            options,
        )));
    }
    push_detail(&mut children, "notes", result.notes.clone(), options);
    children.push(TreeNode::leaf(format_summary(
        &[("bindings", result.bindings.len().to_string())],
        options,
    )));
    if !result.bindings.is_empty() {
        children.push(TreeNode::branch(
            format_section_count("bindings", result.bindings.len(), options),
            result
                .bindings
                .iter()
                .map(|binding| concept_binding_node(binding, options))
                .collect(),
        ));
    }

    render_tree(&TreeNode::branch(
        Palette::new(options).section_header(format!("concept {}", result.concept)),
        children,
    ))
}

pub fn render_trace_with_options(result: &TraceResult, options: RenderOptions) -> String {
    let mut flows = PathMergeNode::default();
    for flow in &result.flows {
        insert_trace_flow(&mut flows, flow, options);
    }

    let mut children = vec![
        TreeNode::leaf(format_key_value(
            "requested_symbol",
            &result.requested_symbol,
            options,
        )),
        TreeNode::leaf(format_key_value(
            "traced_roots",
            &result.traced_roots.join(", "),
            options,
        )),
        TreeNode::leaf(format_key_value(
            "fallback_used",
            if result.fallback_used {
                "true"
            } else {
                "false"
            },
            options,
        )),
    ];
    if let Some(hint) = result.hint.as_deref() {
        children.push(TreeNode::leaf(format_key_value("hint", hint, options)));
    }
    children.push(TreeNode::leaf(format_summary(
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
    )));
    children.push(TreeNode::branch(
        format_section_count("flows", result.summary.total_flows, options),
        flows.into_tree_children(),
    ));

    let root = TreeNode::branch(format_symbol_ref(&result.entry_ref, options), children);

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
        DataflowNodeKind::Entry => {
            let mut label = format!(
                "{} {}",
                palette.symbol_name(&node.name),
                palette.tag("[entry]"),
            );
            label.push_str(&format_file_suffix(node.file.as_deref(), options));
            label
        }
        DataflowNodeKind::Symbol => {
            let mut label = format!(
                "{} {}",
                palette.symbol_name(&node.name),
                palette.tag("[symbol]"),
            );
            label.push_str(&format_file_suffix(node.file.as_deref(), options));
            label
        }
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

fn origin_leaf_label(origin: &OriginPath, options: RenderOptions) -> String {
    let palette = Palette::new(options);
    format!(
        "{} {}",
        format_symbol_ref(&origin.api, options),
        palette.tag(format!(
            "[{} {:.2}]",
            origin.terminal_kind, origin.confidence
        ))
    )
}

fn origin_snippet_node(snippet: &OriginSnippet, options: RenderOptions) -> TreeNode {
    let mut children = vec![TreeNode::leaf(format_key_value(
        "reason",
        &snippet.reason,
        options,
    ))];
    push_detail(
        &mut children,
        "snippet",
        Some(snippet.snippet.clone()),
        options,
    );
    let mut symbol_children = symbol_ref_details(&snippet.symbol, options);
    symbol_children.append(&mut children);
    tree_node(format_symbol_ref(&snippet.symbol, options), symbol_children)
}

pub fn render_origin_with_options(result: &OriginResult, options: RenderOptions) -> String {
    let mut children = vec![
        TreeNode::leaf(format_summary(
            &[("origins", result.total_origins.to_string())],
            options,
        )),
        TreeNode::branch(
            format_section_count("origins", result.total_origins, options),
            result
                .origins
                .iter()
                .map(|origin| {
                    let mut item_children = Vec::new();
                    if !origin.path.is_empty() {
                        item_children.push(TreeNode::leaf(format_key_value(
                            "path",
                            &origin.path.join(" <- "),
                            options,
                        )));
                    }
                    if !origin.field_candidates.is_empty() {
                        item_children.push(TreeNode::leaf(format_key_value(
                            "field_candidates",
                            &origin.field_candidates.join(", "),
                            options,
                        )));
                    }
                    if options.fields.snippet && !origin.code_snippets.is_empty() {
                        item_children.push(TreeNode::branch(
                            format_section_count(
                                "code snippets",
                                origin.code_snippets.len(),
                                options,
                            ),
                            origin
                                .code_snippets
                                .iter()
                                .map(|snippet| origin_snippet_node(snippet, options))
                                .collect(),
                        ));
                    }
                    if let Some(endpoint) = &origin.endpoint {
                        item_children.push(TreeNode::leaf(format_key_value(
                            "endpoint", endpoint, options,
                        )));
                    }
                    if let Some(method) = &origin.request_method {
                        item_children.push(TreeNode::leaf(format_key_value(
                            "request_method",
                            method,
                            options,
                        )));
                    }
                    if !origin.request_keys.is_empty() {
                        item_children.push(TreeNode::leaf(format_key_value(
                            "request_keys",
                            &origin.request_keys.join(", "),
                            options,
                        )));
                    }
                    item_children.extend(
                        origin
                            .notes
                            .iter()
                            .map(|note| TreeNode::leaf(format_key_value("note", note, options))),
                    );
                    TreeNode::branch(origin_leaf_label(origin, options), item_children)
                })
                .collect(),
        ),
    ];
    if result.truncated {
        children.push(TreeNode::leaf(format_key_value(
            "hint",
            "origin results were truncated to keep traversal bounded",
            options,
        )));
    }
    if result.total_origins == 0 {
        children.push(TreeNode::leaf(format_key_value(
            "hint",
            "no upstream terminal found from current graph edges",
            options,
        )));
    }

    render_tree(&TreeNode::branch(
        format_symbol_ref(&result.target_ref, options),
        children,
    ))
}

pub fn render_impact_with_options(result: &ImpactResult, options: RenderOptions) -> String {
    let dependents = result
        .tree
        .children
        .iter()
        .map(|node| impact_tree_to_tree_node(node, options))
        .collect();

    let root = symbol_ref_node(
        &result.source_ref,
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
        options,
    );

    render_tree(&root)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use grapha_core::graph::{NodeKind, Visibility};

    use crate::localization::{LocalizationCatalogRecord, LocalizationReference};
    use crate::query::{
        ContextResult, SymbolInfo, SymbolRef, SymbolTreeRef, dataflow::DataflowEdge,
        dataflow::DataflowEdgeKind, dataflow::DataflowNode, dataflow::DataflowNodeKind,
        dataflow::DataflowResult, dataflow::DataflowSummary, entries::EntriesResult,
        impact::ImpactResult, impact::ImpactTreeNode, localize::LocalizationMatch,
        localize::LocalizeResult, localize::UnmatchedLocalizationUsage, origin::OriginPath,
        origin::OriginResult, origin::OriginSnippet, reverse::AffectedEntry,
        reverse::ReverseResult, trace::Flow, trace::TerminalInfo, trace::TraceResult,
        trace::TraceSummary, usages::RecordUsages, usages::UsageQuery, usages::UsageSite,
        usages::UsagesResult,
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
            locator: None,
            name: name.to_string(),
            kind,
            file: file.to_string(),
            span: Some([1, 2]),
            visibility: Some(Visibility::Public),
            role: None,
            signature: None,
            module: None,
            snippet: None,
        }
    }

    fn symbol_info(name: &str, kind: NodeKind, file: &str) -> SymbolInfo {
        SymbolInfo {
            id: format!("{file}::{name}"),
            locator: None,
            name: name.to_string(),
            kind,
            file: file.to_string(),
            span: [1, 2],
            visibility: Some(Visibility::Public),
            role: None,
            signature: None,
            module: None,
            snippet: None,
        }
    }

    #[test]
    fn context_omits_empty_sections() {
        let result = ContextResult {
            symbol: symbol_info("helper", NodeKind::Function, "main.rs"),
            callers: vec![symbol_ref("main", NodeKind::Function, "main.rs")],
            callees: Vec::new(),
            reads: Vec::new(),
            read_by: Vec::new(),
            invalidation_sources: Vec::new(),
            contains: Vec::new(),
            contains_tree: Vec::new(),
            contained_by: Vec::new(),
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let rendered = render_context_with_options(&result, RenderOptions::plain());
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
            reads: vec![symbol_ref(
                "roomMode",
                NodeKind::Property,
                "ContentView.swift",
            )],
            read_by: Vec::new(),
            invalidation_sources: vec![symbol_ref(
                "roomMode",
                NodeKind::Property,
                "ContentView.swift",
            )],
            contains: vec![symbol_ref("VStack", NodeKind::View, "ContentView.swift")],
            contains_tree: vec![SymbolTreeRef {
                id: "ContentView.swift::body::VStack".into(),
                locator: None,
                name: "VStack".into(),
                kind: NodeKind::View,
                file: "ContentView.swift".into(),
                span: Some([1, 2]),
                visibility: Some(Visibility::Public),
                role: None,
                signature: None,
                module: None,
                snippet: None,
                contains: vec![
                    SymbolTreeRef {
                        id: "ContentView.swift::body::Text".into(),
                        locator: None,
                        name: "Text".into(),
                        kind: NodeKind::View,
                        file: "ContentView.swift".into(),
                        span: Some([1, 2]),
                        visibility: Some(Visibility::Public),
                        role: None,
                        signature: None,
                        module: None,
                        snippet: None,
                        contains: Vec::new(),
                    },
                    SymbolTreeRef {
                        id: "ContentView.swift::body::Row".into(),
                        locator: None,
                        name: "Row".into(),
                        kind: NodeKind::View,
                        file: "ContentView.swift".into(),
                        span: Some([1, 2]),
                        visibility: Some(Visibility::Public),
                        role: None,
                        signature: None,
                        module: None,
                        snippet: None,
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

        let rendered = render_context_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("reads (1)"));
        assert!(rendered.contains("roomMode [property] (ContentView.swift)"));
        assert!(rendered.contains("invalidation_sources (1)"));
        assert!(rendered.contains("contains (1)"));
        assert!(rendered.contains("├── contains (1)"));
        assert!(rendered.contains("│   └── VStack [view] (ContentView.swift)"));
        assert!(rendered.contains("│       ├── Text [view] (ContentView.swift)"));
        assert!(rendered.contains("│       └── Row [view] (ContentView.swift)"));
        assert!(rendered.contains("contained_by (1)"));
        assert!(rendered.contains("ContentView [struct] (ContentView.swift)"));
    }

    #[test]
    fn context_renders_requested_fields_in_tree_output() {
        let mut root = symbol_info("body", NodeKind::Property, "ContentView.swift");
        root.module = Some("Room".into());
        root.signature = Some("var body: some View".into());
        root.role = Some(grapha_core::graph::NodeRole::Internal);
        root.snippet = Some("Text(\"Hello\")\n.padding()".into());

        let mut dependency = symbol_ref("roomMode", NodeKind::Property, "ContentView.swift");
        dependency.module = Some("Room".into());
        dependency.signature = Some("@State var roomMode: RoomMode".into());
        dependency.role = Some(grapha_core::graph::NodeRole::Internal);
        dependency.snippet = Some("roomMode".into());

        let result = ContextResult {
            symbol: root,
            callers: Vec::new(),
            callees: Vec::new(),
            reads: vec![dependency],
            read_by: Vec::new(),
            invalidation_sources: Vec::new(),
            contains: Vec::new(),
            contains_tree: Vec::new(),
            contained_by: Vec::new(),
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let rendered = render_context_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::all()),
        );

        assert!(rendered.contains("id: ContentView.swift::body"));
        assert!(rendered.contains("module: Room"));
        assert!(rendered.contains("span: 1..2"));
        assert!(rendered.contains("visibility: public"));
        assert!(rendered.contains("signature: var body: some View"));
        assert!(rendered.contains("role: internal"));
        assert!(rendered.contains("├── snippet"));
        assert!(rendered.contains("│   ├── Text(\"Hello\")"));
        assert!(rendered.contains("│   └── .padding()"));
        assert!(rendered.contains("id: ContentView.swift::roomMode"));
        assert!(rendered.contains("signature: @State var roomMode: RoomMode"));
    }

    #[test]
    fn entries_render_as_tree() {
        let result = EntriesResult {
            entries: vec![
                symbol_ref("boot", NodeKind::Function, "boot.rs"),
                symbol_ref("main", NodeKind::Function, "main.rs"),
            ],
            shown: 2,
            total: 2,
        };

        let rendered = render_entries_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("entry points (2 shown / 2 total)"));
        assert!(rendered.contains("boot [function] (boot.rs)"));
        assert!(rendered.contains("main [function] (main.rs)"));
    }

    #[test]
    fn entries_omit_files_when_file_field_disabled() {
        let result = EntriesResult {
            entries: vec![
                symbol_ref("boot", NodeKind::Function, "boot.rs"),
                symbol_ref("main", NodeKind::Function, "main.rs"),
            ],
            shown: 2,
            total: 2,
        };

        let rendered = render_entries_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("boot [function]"));
        assert!(rendered.contains("main [function]"));
        assert!(!rendered.contains("(boot.rs)"));
        assert!(!rendered.contains("(main.rs)"));
    }

    #[test]
    fn trace_merges_shared_prefixes_and_renders_notes() {
        let result = TraceResult {
            entry: "main.rs::main".to_string(),
            requested_symbol: "main.rs::main".to_string(),
            traced_roots: vec!["main.rs::main".to_string()],
            fallback_used: false,
            hint: None,
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

        let rendered = render_trace_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("main [function] (main.rs)"));
        assert!(rendered.contains("requested_symbol: main.rs::main"));
        assert!(rendered.contains("traced_roots: main.rs::main"));
        assert!(rendered.contains("fallback_used: false"));
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

        let rendered = render_reverse_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("db [function] (target.rs)"));
        assert!(rendered.contains("affected entries (2)"));
        assert!(rendered.contains("service"));
        assert!(rendered.contains("entry1 [entry] [function] (a.rs)"));
        assert!(rendered.contains("entry2 [entry] [function] (b.rs)"));
    }

    #[test]
    fn reverse_omits_files_when_file_field_disabled() {
        let result = ReverseResult {
            symbol: "target.rs::db".to_string(),
            affected_entries: vec![AffectedEntry {
                entry: symbol_ref("entry1", NodeKind::Function, "a.rs"),
                distance: 2,
                path: vec!["entry1".into(), "service".into(), "db".into()],
            }],
            total_entries: 1,
            target_ref: symbol_ref("db", NodeKind::Function, "target.rs"),
        };

        let rendered = render_reverse_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("db [function]"));
        assert!(rendered.contains("entry1 [entry] [function]"));
        assert!(!rendered.contains("(target.rs)"));
        assert!(!rendered.contains("(a.rs)"));
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

        let rendered = render_impact_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("source [function] (core.rs)"));
        assert!(rendered.contains("summary: depth_1=1, depth_2=1, depth_3_plus=0, total=2"));
        assert!(rendered.contains("dependents (2)"));
        assert!(rendered.contains("alpha [function] (a.rs)"));
        assert!(rendered.contains("beta [function] (b.rs)"));
    }

    #[test]
    fn colorized_context_uses_theme_friendly_styles() {
        let result = ContextResult {
            symbol: symbol_info("helper", NodeKind::Function, "main.rs"),
            callers: vec![symbol_ref("main", NodeKind::Function, "main.rs")],
            callees: Vec::new(),
            reads: Vec::new(),
            read_by: Vec::new(),
            invalidation_sources: Vec::new(),
            contains: Vec::new(),
            contains_tree: Vec::new(),
            contained_by: Vec::new(),
            implementors: Vec::new(),
            implements: Vec::new(),
            type_refs: Vec::new(),
        };

        let plain = render_context_with_options(&result, RenderOptions::plain());
        let rendered = render_context_with_options(&result, RenderOptions::color());

        assert!(rendered.contains("\x1b[1mhelper\x1b[0m"));
        assert!(rendered.contains("\x1b[33m[function]\x1b[0m"));
        assert!(rendered.contains("(main.rs)"));
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

    #[test]
    fn dataflow_omits_files_when_file_field_disabled() {
        let result = DataflowResult {
            entry: "main.rs::handler".to_string(),
            nodes: vec![DataflowNode {
                id: "helper.rs::load".to_string(),
                name: "load".to_string(),
                kind: DataflowNodeKind::Symbol,
                file: Some("helper.rs".to_string()),
                effect_kind: None,
                operation: None,
                target: None,
            }],
            edges: vec![DataflowEdge {
                source: "main.rs::handler".to_string(),
                target: "helper.rs::load".to_string(),
                kind: DataflowEdgeKind::Call,
                operation: None,
                conditions: Vec::new(),
                async_boundary: None,
                provenance: vec![],
            }],
            entry_ref: symbol_ref("handler", NodeKind::Function, "main.rs"),
            summary: DataflowSummary {
                symbols: 1,
                effects: 0,
                edges: 1,
                calls: 1,
                reads: 0,
                writes: 0,
                publishes: 0,
                subscribes: 0,
            },
        };

        let rendered = render_dataflow_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("handler [function]"));
        assert!(rendered.contains("load [symbol]"));
        assert!(!rendered.contains("(main.rs)"));
        assert!(!rendered.contains("(helper.rs)"));
    }

    #[test]
    fn origin_omits_files_when_file_field_disabled() {
        let result = OriginResult {
            symbol: "UserAPI.swift::fetchUserInfo".to_string(),
            target_ref: symbol_ref("fetchUserInfo", NodeKind::Function, "UserAPI.swift"),
            origins: vec![OriginPath {
                api: symbol_ref("requestGetUser", NodeKind::Function, "ProfileService.swift"),
                terminal_kind: "network".to_string(),
                path: vec![
                    "fetchUserInfo".into(),
                    "_getUser".into(),
                    "requestGetUser".into(),
                ],
                field_candidates: Vec::new(),
                confidence: 0.8,
                notes: vec!["reached request endpoint user/getUserInfoByUid".into()],
                endpoint: Some("user/getUserInfoByUid/\\(data.id)".into()),
                request_method: None,
                request_keys: vec!["attrs".into()],
                code_snippets: vec![OriginSnippet {
                    symbol: symbol_ref(
                        "requestGetUser",
                        NodeKind::Function,
                        "ProfileService.swift",
                    ),
                    reason: "request_leaf".into(),
                    snippet: "func requestGetUser() {}".into(),
                }],
            }],
            total_origins: 1,
            truncated: false,
        };

        let rendered = render_origin_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("fetchUserInfo [function]"));
        assert!(rendered.contains("requestGetUser [function] [network 0.80]"));
        assert!(!rendered.contains("(UserAPI.swift)"));
        assert!(!rendered.contains("(ProfileService.swift)"));
        assert!(!rendered.contains("code snippets"));
    }

    #[test]
    fn origin_renders_code_snippets_only_when_snippet_field_enabled() {
        let result = OriginResult {
            symbol: "UserAPI.swift::fetchUserInfo".to_string(),
            target_ref: symbol_ref("fetchUserInfo", NodeKind::Function, "UserAPI.swift"),
            origins: vec![OriginPath {
                api: symbol_ref("requestGetUser", NodeKind::Function, "ProfileService.swift"),
                terminal_kind: "network".to_string(),
                path: vec![
                    "fetchUserInfo".into(),
                    "_getUser".into(),
                    "requestGetUser".into(),
                ],
                field_candidates: Vec::new(),
                confidence: 0.8,
                notes: vec!["reached request endpoint user/getUserInfoByUid".into()],
                endpoint: Some("user/getUserInfoByUid/\\(data.id)".into()),
                request_method: None,
                request_keys: vec!["attrs".into()],
                code_snippets: vec![OriginSnippet {
                    symbol: symbol_ref(
                        "requestGetUser",
                        NodeKind::Function,
                        "ProfileService.swift",
                    ),
                    reason: "request_leaf".into(),
                    snippet: "func requestGetUser() {}".into(),
                }],
            }],
            total_origins: 1,
            truncated: false,
        };

        let rendered = render_origin_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::parse("snippet")),
        );
        assert!(rendered.contains("code snippets (1)"));
        assert!(rendered.contains("reason: request_leaf"));
    }

    #[test]
    fn origin_shows_truncation_hint_even_when_no_origins_survive() {
        let result = OriginResult {
            symbol: "UserAPI.swift::fetchUserInfo".to_string(),
            target_ref: symbol_ref("fetchUserInfo", NodeKind::Function, "UserAPI.swift"),
            origins: Vec::new(),
            total_origins: 0,
            truncated: true,
        };

        let rendered = render_origin_with_options(&result, RenderOptions::plain());
        assert!(rendered.contains("hint: origin results were truncated to keep traversal bounded"));
        assert!(rendered.contains("hint: no upstream terminal found from current graph edges"));
    }

    #[test]
    fn localize_omits_catalog_file_when_file_field_disabled() {
        let result = LocalizeResult {
            symbol: symbol_info("body", NodeKind::Property, "ContentView.swift"),
            matches: vec![LocalizationMatch {
                view: symbol_info("Text", NodeKind::View, "ContentView.swift"),
                ui_path: vec!["VStack".into(), "Text".into()],
                reference: LocalizationReference {
                    ref_kind: "wrapper".into(),
                    wrapper_name: Some("welcomeTitle".into()),
                    wrapper_base: None,
                    wrapper_symbol: None,
                    table: Some("Localizable".into()),
                    key: Some("welcome_title".into()),
                    fallback: None,
                    arg_count: None,
                    literal: None,
                },
                record: LocalizationCatalogRecord {
                    table: "Localizable".into(),
                    key: "welcome_title".into(),
                    catalog_file: "Localizable.xcstrings".into(),
                    catalog_dir: ".".into(),
                    source_language: "en".into(),
                    source_value: "Welcome".into(),
                    status: "translated".into(),
                    comment: None,
                    translations: BTreeMap::new(),
                },
                match_kind: "wrapper".into(),
            }],
            unmatched: vec![UnmatchedLocalizationUsage {
                view: symbol_info("Text", NodeKind::View, "ContentView.swift"),
                ui_path: Vec::new(),
                reference: LocalizationReference {
                    ref_kind: "literal".into(),
                    wrapper_name: None,
                    wrapper_base: None,
                    wrapper_symbol: None,
                    table: None,
                    key: None,
                    fallback: None,
                    arg_count: None,
                    literal: Some("Hello".into()),
                },
                reason: "no record".into(),
            }],
        };

        let rendered = render_localize_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("record: Localizable.welcome_title"));
        assert!(!rendered.contains("Localizable.xcstrings"));
    }

    #[test]
    fn usages_omits_catalog_file_when_file_field_disabled() {
        let result = UsagesResult {
            query: UsageQuery {
                key: "welcome_title".into(),
                table: Some("Localizable".into()),
                matched_by: None,
                resolved_key: None,
            },
            records: vec![RecordUsages {
                record: LocalizationCatalogRecord {
                    table: "Localizable".into(),
                    key: "welcome_title".into(),
                    catalog_file: "Localizable.xcstrings".into(),
                    catalog_dir: ".".into(),
                    source_language: "en".into(),
                    source_value: "Welcome".into(),
                    status: "translated".into(),
                    comment: None,
                    translations: BTreeMap::new(),
                },
                usages: vec![UsageSite {
                    owner: symbol_info("body", NodeKind::Property, "ContentView.swift"),
                    view: symbol_info("Text", NodeKind::View, "ContentView.swift"),
                    ui_path: vec!["Text".into()],
                    reference: LocalizationReference {
                        ref_kind: "wrapper".into(),
                        wrapper_name: Some("welcomeTitle".into()),
                        wrapper_base: None,
                        wrapper_symbol: None,
                        table: Some("Localizable".into()),
                        key: Some("welcome_title".into()),
                        fallback: None,
                        arg_count: None,
                        literal: None,
                    },
                }],
            }],
        };

        let rendered = render_usages_with_options(
            &result,
            RenderOptions::plain().with_fields(FieldSet::none()),
        );
        assert!(rendered.contains("Localizable.welcome_title"));
        assert!(!rendered.contains("Localizable.xcstrings"));
    }
}
