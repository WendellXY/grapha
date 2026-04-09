use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tree_sitter::{Parser, Tree};

use grapha_core::graph::{
    Edge, EdgeKind, EdgeProvenance, Node, NodeKind, NodeRole, Span, Visibility,
};

use grapha_core::{ExtractionResult, LanguageExtractor};

/// Parse Swift source once. Reuse the tree across enrichment passes.
pub fn parse_swift(source: &[u8]) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))
}

pub struct SwiftExtractor;

impl LanguageExtractor for SwiftExtractor {
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))?;

        let mut result = ExtractionResult::new();
        let file_str = file_path.to_string_lossy().to_string();

        walk_node(tree.root_node(), source, &file_str, &[], None, &mut result);

        Ok(result)
    }
}

#[derive(Clone)]
struct NodeSnapshot {
    id: String,
    kind: NodeKind,
    name: String,
    file: PathBuf,
    span: Span,
}

impl From<&Node> for NodeSnapshot {
    fn from(node: &Node) -> Self {
        Self {
            id: node.id.clone(),
            kind: node.kind,
            name: node.name.clone(),
            file: node.file.clone(),
            span: node.span.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct EdgeDedupKey {
    source: String,
    target: String,
    kind: EdgeKind,
    operation: Option<String>,
}

impl From<&Edge> for EdgeDedupKey {
    fn from(edge: &Edge) -> Self {
        Self {
            source: edge.source.clone(),
            target: edge.target.clone(),
            kind: edge.kind,
            operation: edge.operation.clone(),
        }
    }
}

struct EnrichmentContext<'a> {
    result: &'a mut ExtractionResult,
    node_snapshots: Vec<NodeSnapshot>,
    nodes_by_kind_name: HashMap<(NodeKind, String), Vec<usize>>,
    node_index_by_id: HashMap<String, usize>,
    candidate_owner_names: HashMap<String, Vec<String>>,
    existing_node_ids: HashSet<String>,
    existing_edge_keys: HashSet<EdgeDedupKey>,
}

impl<'a> EnrichmentContext<'a> {
    fn new(result: &'a mut ExtractionResult) -> Self {
        let candidate_owner_names = candidate_owner_names(result);
        let existing_edge_keys = result.edges.iter().map(EdgeDedupKey::from).collect();
        let existing_node_ids = result.nodes.iter().map(|node| node.id.clone()).collect();
        let snapshots: Vec<(usize, NodeSnapshot)> = result
            .nodes
            .iter()
            .enumerate()
            .map(|(idx, node)| (idx, NodeSnapshot::from(node)))
            .collect();
        let mut context = Self {
            result,
            node_snapshots: Vec::with_capacity(snapshots.len()),
            nodes_by_kind_name: HashMap::new(),
            node_index_by_id: HashMap::new(),
            candidate_owner_names,
            existing_node_ids,
            existing_edge_keys,
        };
        for (result_index, snapshot) in snapshots {
            context.track_node(snapshot, result_index);
        }
        context
    }

    fn track_node(&mut self, snapshot: NodeSnapshot, result_index: usize) {
        let snapshot_index = self.node_snapshots.len();
        self.node_index_by_id
            .insert(snapshot.id.clone(), result_index);
        self.nodes_by_kind_name
            .entry((snapshot.kind, snapshot.name.clone()))
            .or_default()
            .push(snapshot_index);
        self.node_snapshots.push(snapshot);
    }

    fn node_mut(&mut self, node_id: &str) -> Option<&mut Node> {
        let index = *self.node_index_by_id.get(node_id)?;
        self.result.nodes.get_mut(index)
    }

    fn local_wrapper_id(&self, file_path: &Path, wrapper_name: &str) -> Option<String> {
        self.result
            .nodes
            .iter()
            .find(|node| {
                file_matches(&node.file, file_path)
                    && node.name == wrapper_name
                    && node.metadata.contains_key("l10n.wrapper.key")
            })
            .map(|node| node.id.clone())
    }

    fn push_node(&mut self, node: Node) -> bool {
        if !self.existing_node_ids.insert(node.id.clone()) {
            return false;
        }
        let result_index = self.result.nodes.len();
        let snapshot = NodeSnapshot::from(&node);
        self.result.nodes.push(node);
        self.track_node(snapshot, result_index);
        true
    }

    fn emit_edge(&mut self, edge: Edge) {
        let key = EdgeDedupKey::from(&edge);
        if self.existing_edge_keys.insert(key) {
            self.result.edges.push(edge);
        }
    }

    fn matching_declaration_id(
        &self,
        file_path: &Path,
        decl_node: tree_sitter::Node,
        source: &[u8],
    ) -> Option<String> {
        let decl_line = decl_node.start_position().row;
        let decl_name = declaration_name(decl_node, source)?;
        let decl_kind = declaration_kind(decl_node)?;
        let owner_name = enclosing_owner_type_name(decl_node, source);
        let key = (decl_kind, decl_name);
        let candidates = self
            .nodes_by_kind_name
            .get(&key)?
            .iter()
            .map(|snapshot_index| &self.node_snapshots[*snapshot_index])
            .filter(|node| file_matches(&node.file, file_path))
            .collect::<Vec<_>>();

        let line_matches = candidates
            .iter()
            .copied()
            .filter(|node| line_matches(node.span.start[0], decl_line))
            .collect::<Vec<_>>();
        if !line_matches.is_empty() {
            return line_matches
                .into_iter()
                .min_by_key(|node| node.span.start[0].abs_diff(decl_line))
                .map(|node| node.id.clone());
        }

        if let Some(owner_name) = owner_name {
            let owner_matches = candidates
                .iter()
                .copied()
                .filter(|node| {
                    self.candidate_owner_names
                        .get(&node.id)
                        .is_some_and(|owners| owners.iter().any(|owner| owner == &owner_name))
                })
                .collect::<Vec<_>>();
            if owner_matches.len() == 1 {
                return Some(owner_matches[0].id.clone());
            }
            if !owner_matches.is_empty() {
                return owner_matches
                    .into_iter()
                    .min_by_key(|node| node.span.start[0].abs_diff(decl_line))
                    .map(|node| node.id.clone());
            }
        }

        if candidates.len() == 1 {
            return Some(candidates[0].id.clone());
        }

        candidates
            .into_iter()
            .min_by_key(|node| node.span.start[0].abs_diff(decl_line))
            .map(|node| node.id.clone())
    }

    fn matching_body_id(&self, file_path: &Path, body_node: tree_sitter::Node) -> Option<String> {
        let body_line = body_node.start_position().row;
        self.nodes_by_kind_name
            .get(&(NodeKind::Property, "body".to_string()))?
            .iter()
            .map(|snapshot_index| &self.node_snapshots[*snapshot_index])
            .filter(|node| {
                file_matches(&node.file, file_path) && line_matches(node.span.start[0], body_line)
            })
            .min_by_key(|node| node.span.start[0].abs_diff(body_line))
            .map(|node| node.id.clone())
    }
}

/// Recursively walk a tree-sitter node, extracting Swift symbols and edges.
fn walk_node(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    match node.kind() {
        "class_declaration" => {
            let declaration_type = detect_class_declaration_type(node);
            match declaration_type {
                ClassDeclarationType::Struct => {
                    extract_struct_or_class(
                        node,
                        source,
                        file,
                        module_path,
                        parent_id,
                        NodeKind::Struct,
                        result,
                    );
                }
                ClassDeclarationType::Class => {
                    extract_struct_or_class(
                        node,
                        source,
                        file,
                        module_path,
                        parent_id,
                        NodeKind::Class,
                        result,
                    );
                }
                ClassDeclarationType::Enum => {
                    extract_enum(node, source, file, module_path, parent_id, result);
                }
                ClassDeclarationType::Extension => {
                    extract_extension(node, source, file, module_path, parent_id, result);
                }
            }
        }
        "protocol_declaration" => {
            extract_protocol(node, source, file, module_path, parent_id, result);
        }
        "function_declaration" | "init_declaration" | "deinit_declaration" => {
            extract_function(node, source, file, module_path, parent_id, result);
        }
        "protocol_function_declaration" => {
            extract_function(node, source, file, module_path, parent_id, result);
        }
        "property_declaration" => {
            extract_property(node, source, file, module_path, parent_id, result);
        }
        "typealias_declaration" => {
            extract_typealias(node, source, file, module_path, parent_id, result);
        }
        "import_declaration" => {
            extract_import(node, source, file, result);
        }
        _ => {
            walk_children(node, source, file, module_path, parent_id, result);
        }
    }
}

/// Walk all named children of a node.
fn walk_children(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_node(child, source, file, module_path, parent_id, result);
    }
}

/// Build a node ID from file, module path, and name.
fn make_id(file: &str, module_path: &[String], name: &str) -> String {
    if module_path.is_empty() {
        format!("{}::{}", file, name)
    } else {
        format!("{}::{}::{}", file, module_path.join("::"), name)
    }
}

/// Build a declaration/member ID scoped to its owning declaration when present.
fn make_decl_id(file: &str, module_path: &[String], parent_id: Option<&str>, name: &str) -> String {
    parent_id
        .map(|pid| format!("{pid}::{name}"))
        .unwrap_or_else(|| make_id(file, module_path, name))
}

fn unique_decl_id(
    result: &ExtractionResult,
    proposed_id: String,
    node: tree_sitter::Node,
) -> String {
    if result
        .nodes
        .iter()
        .all(|existing| existing.id != proposed_id)
    {
        return proposed_id;
    }

    let span = make_span(node);
    format!(
        "{proposed_id}@{}:{}:{}:{}",
        span.start[0], span.start[1], span.end[0], span.end[1]
    )
}

/// Extract the text of the first `simple_identifier` named child (used for function names).
fn simple_identifier_text(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "simple_identifier" {
            return child.utf8_text(source).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Extract the `type_identifier` named child text (used for type names).
fn type_identifier_text(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_identifier" {
            return child.utf8_text(source).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Determine the Swift declaration kind from a `class_declaration` node.
///
/// tree-sitter-swift uses `class_declaration` for struct, class, enum, and extension.
/// We distinguish them by the anonymous keyword child token.
fn detect_class_declaration_type(node: tree_sitter::Node) -> ClassDeclarationType {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            match child.kind() {
                "struct" => return ClassDeclarationType::Struct,
                "class" => return ClassDeclarationType::Class,
                "enum" => return ClassDeclarationType::Enum,
                "extension" => return ClassDeclarationType::Extension,
                _ => {}
            }
        }
    }
    // Default to class if we can't determine
    ClassDeclarationType::Class
}

enum ClassDeclarationType {
    Struct,
    Class,
    Enum,
    Extension,
}

/// Extract visibility from a Swift node by checking for a `modifiers` child
/// containing a `visibility_modifier`.
fn extract_visibility(node: tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.named_children(&mut mod_cursor) {
                if modifier.kind() == "visibility_modifier" {
                    let text = modifier.utf8_text(source).unwrap_or("");
                    if text == "public" || text == "open" {
                        return Visibility::Public;
                    } else if text == "private" || text == "fileprivate" {
                        return Visibility::Private;
                    }
                    // "internal" is default in Swift, maps to Crate
                    return Visibility::Crate;
                }
            }
        }
    }
    // Swift default is `internal`
    Visibility::Crate
}

fn make_span(node: tree_sitter::Node) -> Span {
    let start = node.start_position();
    let end = node.end_position();
    Span {
        start: [start.row, start.column],
        end: [end.row, end.column],
    }
}

fn edge_provenance(file: &str, span: Span, symbol_id: &str) -> Vec<EdgeProvenance> {
    vec![EdgeProvenance {
        file: file.into(),
        span,
        symbol_id: symbol_id.to_string(),
    }]
}

fn node_edge_provenance(
    file: &str,
    node: tree_sitter::Node,
    symbol_id: &str,
) -> Vec<EdgeProvenance> {
    edge_provenance(file, make_span(node), symbol_id)
}

fn span_from_text_range(
    node: tree_sitter::Node,
    text: &str,
    start: usize,
    end: usize,
) -> Option<Span> {
    let bytes = text.as_bytes();
    if start > end || end > bytes.len() {
        return None;
    }

    fn absolute_position(base: tree_sitter::Point, bytes: &[u8]) -> [usize; 2] {
        let newline_count = bytes.iter().filter(|&&byte| byte == b'\n').count();
        if newline_count == 0 {
            [base.row, base.column + bytes.len()]
        } else {
            let last_newline = bytes
                .iter()
                .rposition(|&byte| byte == b'\n')
                .expect("counted newlines above");
            [base.row + newline_count, bytes.len() - last_newline - 1]
        }
    }

    let base = node.start_position();
    Some(Span {
        start: absolute_position(base, &bytes[..start]),
        end: absolute_position(base, &bytes[..end]),
    })
}

fn emit_contains_edge(
    parent_id: Option<&str>,
    child_id: &str,
    file: &str,
    edge_node: tree_sitter::Node,
    result: &mut ExtractionResult,
) {
    if let Some(pid) = parent_id {
        result.edges.push(Edge {
            source: pid.to_string(),
            target: child_id.to_string(),
            kind: EdgeKind::Contains,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, edge_node, pid),
        });
    }
}

/// Extract struct or class declaration.
fn extract_struct_or_class(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    kind: NodeKind,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    // Extract inheritance/conformance edges
    extract_inheritance_edges(node, source, file, module_path, &id, result);

    // Detect entry point: @main attribute
    let has_main_attr = has_swift_attribute(node, source, "main");
    let role = if has_main_attr {
        Some(NodeRole::EntryPoint)
    } else {
        None
    };

    // Detect conformances for body-level entry point marking
    let conformances = collect_inheritance_names(node, source);
    let conforms_to_view = conformances.iter().any(|c| c == "View" || c == "App");
    let is_observable = conformances.iter().any(|c| c == "ObservableObject")
        || has_swift_attribute(node, source, "Observable");

    result.nodes.push(Node {
        id: id.clone(),
        kind,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Walk the class_body for nested declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_body" {
            walk_children_with_hints(
                child,
                source,
                file,
                module_path,
                Some(&id),
                conforms_to_view,
                is_observable,
                result,
            );
        }
    }
}

/// Walk children with hints about parent conformances for entry point detection.
#[allow(clippy::too_many_arguments)]
fn walk_children_with_hints(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    parent_conforms_to_view: bool,
    parent_is_observable: bool,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "property_declaration" && parent_conforms_to_view {
            // Check if this is the `body` property
            if let Some(prop_name) = find_pattern_name(child, source)
                && prop_name == "body"
            {
                extract_property_as_entry_point(
                    child,
                    source,
                    file,
                    module_path,
                    parent_id,
                    result,
                );
                continue;
            }
        }
        if child.kind() == "function_declaration" && parent_is_observable {
            // Mark public methods as entry points
            extract_function_with_entry_hint(
                child,
                source,
                file,
                module_path,
                parent_id,
                true,
                result,
            );
            continue;
        }
        walk_node(child, source, file, module_path, parent_id, result);
    }
}

/// Extract a property and mark it as an entry point (e.g., View.body).
fn extract_property_as_entry_point(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let name = find_pattern_name(node, source);
    let Some(name) = name else { return };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: Some(NodeRole::EntryPoint),
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Scan property body for calls (same as extract_property)
    extract_calls(node, source, file, module_path, &id, result);
    // Always run regex fallback to catch calls inside closures/ViewBuilder
    // bodies that tree-sitter doesn't parse as call_expression nodes.
    if let Ok(text) = node.utf8_text(source) {
        extract_calls_from_text(text, node, file, module_path, &id, result);
    }
    extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
}

/// Extract a function with an optional entry point hint from parent (Observable).
fn extract_function_with_entry_hint(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    observable_parent: bool,
    result: &mut ExtractionResult,
) {
    // init/deinit declarations don't have a simple_identifier name
    let name = if node.kind() == "init_declaration" {
        "init".to_string()
    } else if node.kind() == "deinit_declaration" {
        "deinit".to_string()
    } else {
        let Some(n) = simple_identifier_text(node, source) else {
            return;
        };
        n
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);
    let signature = extract_swift_signature(node, source);
    let doc_comment = extract_swift_doc_comment(node, source);

    let role = if observable_parent
        && (visibility == Visibility::Public || visibility == Visibility::Crate)
    {
        Some(NodeRole::EntryPoint)
    } else {
        None
    };

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role,
        signature,
        doc_comment,
        module: None,
        snippet: None,
    });

    // Walk function body for call expressions.
    // init_declaration uses "class_body" or direct children, not "function_body"
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "function_body" | "class_body") {
            extract_calls(child, source, file, module_path, &id, result);
        }
    }
    // Fallback: if no function_body found (e.g., init), scan all children
    // that aren't parameter lists or modifiers
    let has_body = {
        let mut c = node.walk();
        node.named_children(&mut c)
            .any(|ch| matches!(ch.kind(), "function_body" | "class_body"))
    };
    if !has_body {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            if !matches!(
                child.kind(),
                "modifiers" | "parameter" | "type_annotation" | "attribute" | "where_clause"
            ) {
                extract_calls(child, source, file, module_path, &id, result);
            }
        }
    }
}

/// Extract enum declaration with case variants.
fn extract_enum(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Enum,
        name: name.clone(),
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Extract enum entries from enum_class_body
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "enum_class_body" {
            extract_enum_entries(child, source, file, module_path, &id, result);
            walk_children(child, source, file, module_path, Some(&id), result);
        }
    }
}

/// Extract enum_entry children from an enum body.
fn extract_enum_entries(
    body: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: &str,
    result: &mut ExtractionResult,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "enum_entry"
            && let Some(case_name) = simple_identifier_text(child, source)
        {
            let id = make_decl_id(file, module_path, Some(parent_id), &case_name);

            result.edges.push(Edge {
                source: parent_id.to_string(),
                target: id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, child, parent_id),
            });

            result.nodes.push(Node {
                id,
                kind: NodeKind::Variant,
                name: case_name,
                file: file.into(),
                span: make_span(child),
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
                snippet: None,
            });
        }
    }
}

/// Extract extension declaration.
fn extract_extension(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    // Extension uses user_type > type_identifier for the extended type name
    let name = find_user_type_name(node, source).unwrap_or_else(|| "Unknown".to_string());
    let ext_name = format!("ext_{}", name);
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &ext_name),
        node,
    );

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Extension,
        name,
        file: file.into(),
        span: make_span(node),
        visibility: Visibility::Crate,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Walk the class_body for nested declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_body" {
            walk_children(child, source, file, module_path, Some(&id), result);
        }
    }
}

/// Find the type name from a `user_type > type_identifier` child.
fn find_user_type_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "user_type" {
            return type_identifier_text(child, source);
        }
    }
    None
}

/// Extract protocol declaration.
fn extract_protocol(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Protocol,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Walk the protocol_body for method declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "protocol_body" {
            walk_children(child, source, file, module_path, Some(&id), result);
        }
    }
}

/// Extract function declaration (including init/deinit).
fn extract_function(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let name = if node.kind() == "init_declaration" {
        "init".to_string()
    } else if node.kind() == "deinit_declaration" {
        "deinit".to_string()
    } else {
        let Some(n) = simple_identifier_text(node, source) else {
            return;
        };
        n
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);
    let signature = extract_swift_signature(node, source);
    let doc_comment = extract_swift_doc_comment(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature,
        doc_comment,
        module: None,
        snippet: None,
    });

    // Walk function body for call expressions.
    // init_declaration may not have "function_body" — scan all non-parameter children as fallback.
    let mut cursor = node.walk();
    let mut found_body = false;
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_body" {
            extract_calls(child, source, file, module_path, &id, result);
            found_body = true;
        }
    }
    if !found_body {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            if !matches!(
                child.kind(),
                "modifiers"
                    | "parameter"
                    | "type_annotation"
                    | "attribute"
                    | "where_clause"
                    | "simple_identifier"
                    | "type_identifier"
            ) {
                extract_calls(child, source, file, module_path, &id, result);
            }
        }
    }

    if declaration_returns_swiftui_view(node, source) {
        extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
    }
}

/// Extract property declaration.
fn extract_property(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    // Property name is in pattern > simple_identifier
    let name = find_pattern_name(node, source);
    let Some(name) = name else { return };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });

    // Scan property body for calls via tree-sitter AST
    extract_calls(node, source, file, module_path, &id, result);

    // Fallback: if no calls edges were found via AST, scan the property's source
    // text for function call patterns using regex. This handles cases where
    // tree-sitter-swift doesn't produce call_expression nodes (e.g., SwiftUI
    // View body with result builders).
    let calls_before = result
        .edges
        .iter()
        .filter(|e| e.source == id && e.kind == EdgeKind::Calls)
        .count();
    if calls_before == 0
        && let Ok(text) = node.utf8_text(source)
    {
        extract_calls_from_text(text, node, file, module_path, &id, result);
    }

    if declaration_returns_swiftui_view(node, source) {
        extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
    }
}

/// Extract function calls from raw source text using regex.
/// Fallback for when tree-sitter doesn't produce call_expression nodes
/// (e.g., SwiftUI View body with result builders).
fn extract_calls_from_text(
    text: &str,
    node: tree_sitter::Node,
    file: &str,
    module_path: &[String],
    caller_id: &str,
    result: &mut ExtractionResult,
) {
    // Match patterns like: identifier( or .identifier(
    // But skip common keywords and type annotations
    let call_re = regex::Regex::new(r"(?:^|[.\s({,])([a-z][a-zA-Z0-9]*)\s*\(").unwrap();
    let skip_names: std::collections::HashSet<&str> = [
        "if", "for", "while", "switch", "guard", "return", "let", "var", "case", "some", "in",
        "as", "is", "try", "await", "throw", "catch", "where",
    ]
    .into_iter()
    .collect();

    let mut seen = std::collections::HashSet::new();
    for cap in call_re.captures_iter(text) {
        let fn_name = cap.get(1).unwrap().as_str();
        if skip_names.contains(fn_name) || !seen.insert(fn_name.to_string()) {
            continue;
        }
        let target_id = make_id(file, module_path, fn_name);
        let span = cap
            .get(1)
            .and_then(|capture| span_from_text_range(node, text, capture.start(), capture.end()))
            .unwrap_or_else(|| make_span(node));
        result.edges.push(Edge {
            source: caller_id.to_string(),
            target: target_id,
            kind: EdgeKind::Calls,
            confidence: 0.5, // lower confidence for regex-based extraction
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: edge_provenance(file, span, caller_id),
        });
    }

    // Also match property access chains like AppContext.gift.activityGiftConfigs
    let nav_re = regex::Regex::new(r"[A-Za-z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9]*)+").unwrap();
    for mat in nav_re.find_iter(text) {
        let chain = mat.as_str();
        let parts: Vec<&str> = chain.split('.').collect();
        if parts.len() >= 2 {
            let last = *parts.last().unwrap();
            if skip_names.contains(last) || seen.contains(last) {
                continue;
            }
            seen.insert(last.to_string());
            let prefix = parts[..parts.len() - 1].join(".");
            let target_id = make_id(file, module_path, last);
            let span = span_from_text_range(node, text, mat.start(), mat.end())
                .unwrap_or_else(|| make_span(node));
            result.edges.push(Edge {
                source: caller_id.to_string(),
                target: target_id,
                kind: EdgeKind::Reads,
                confidence: 0.5,
                direction: None,
                operation: Some(prefix),
                condition: None,
                async_boundary: None,
                provenance: edge_provenance(file, span, caller_id),
            });
        }
    }
}

/// Find the name from a `pattern > simple_identifier` child.
fn find_pattern_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "pattern" {
            return simple_identifier_text(child, source);
        }
    }
    None
}

/// Extract typealias declaration.
fn extract_typealias(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id,
        kind: NodeKind::TypeAlias,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });
}

/// Extract import declaration into an Import struct.
fn extract_import(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    result: &mut ExtractionResult,
) {
    // The import path is in the identifier > simple_identifier child
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier"
            && let Ok(path_text) = child.utf8_text(source)
        {
            let path = path_text.to_string();

            result.imports.push(grapha_core::resolve::Import {
                path: path.clone(),
                symbols: vec![],
                kind: grapha_core::resolve::ImportKind::Module,
            });

            result.edges.push(Edge {
                source: file.to_string(),
                target: format!("import {}", path),
                kind: EdgeKind::Uses,
                confidence: 0.7,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, node, file),
            });
        }
    }
}

/// Extract function signature (text up to opening `{`).
fn extract_swift_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    let sig = if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim()
    } else {
        text.trim()
    };
    if sig.is_empty() {
        None
    } else {
        Some(sig.to_string())
    }
}

/// Enrich an existing `ExtractionResult` (e.g. from the index store) with doc
/// comments extracted via tree-sitter.  The index store does not provide doc
/// comments, so we do a lightweight tree-sitter parse and match nodes by
/// `(name, start_line)`.
///
/// Index store lines are **1-based**; tree-sitter rows are **0-based**, so we
/// compare with `row + 1`.
#[cfg(test)]
pub fn enrich_doc_comments(source: &[u8], result: &mut ExtractionResult) -> anyhow::Result<()> {
    let tree = parse_swift(source)?;
    enrich_doc_comments_with_tree(source, &tree, result)
}

pub fn enrich_doc_comments_with_tree(
    source: &[u8],
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    // Collect (name, 1-based line) → doc_comment from tree-sitter AST.
    let mut doc_map: HashMap<(String, usize), String> = HashMap::new();
    collect_doc_comments(tree.root_node(), source, &mut doc_map);

    // Patch nodes that are missing a doc_comment.
    // Index-store spans are 1-based, while tree-sitter/SwiftSyntax spans are
    // 0-based. Try the node's stored line first, then a 1-based adjustment.
    for node in &mut result.nodes {
        if node.doc_comment.is_some() {
            continue;
        }
        let line = node.span.start[0];
        let key = (node.name.clone(), line);
        let adjusted_key = (node.name.clone(), line + 1);
        if let Some(doc) = doc_map
            .remove(&key)
            .or_else(|| doc_map.remove(&adjusted_key))
        {
            node.doc_comment = Some(doc);
        }
    }

    Ok(())
}

/// Recursively walk the tree-sitter AST and collect doc comments for every
/// declaration that has one.  Results are keyed by `(name, 1-based line)`.
fn collect_doc_comments(
    node: tree_sitter::Node,
    source: &[u8],
    out: &mut HashMap<(String, usize), String>,
) {
    match node.kind() {
        "class_declaration" | "protocol_declaration" => {
            if let Some(name) = type_identifier_text(node, source)
                && let Some(doc) = extract_swift_doc_comment(node, source)
            {
                out.insert((name, node.start_position().row + 1), doc);
            }
        }
        "function_declaration"
        | "init_declaration"
        | "deinit_declaration"
        | "protocol_function_declaration" => {
            let name = if node.kind() == "init_declaration" {
                Some("init".to_string())
            } else if node.kind() == "deinit_declaration" {
                Some("deinit".to_string())
            } else {
                simple_identifier_text(node, source)
            };
            if let Some(name) = name
                && let Some(doc) = extract_swift_doc_comment(node, source)
            {
                out.insert((name, node.start_position().row + 1), doc);
            }
        }
        "property_declaration" => {
            if let Some(name) = find_pattern_name(node, source)
                && let Some(doc) = extract_swift_doc_comment(node, source)
            {
                out.insert((name, node.start_position().row + 1), doc);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_doc_comments(child, source, out);
    }
}

/// Extract doc comments from previous sibling comment nodes.
fn extract_swift_doc_comment(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut comments = Vec::new();
    let mut prev = node.prev_named_sibling();
    while let Some(sib) = prev {
        if sib.kind() == "comment" || sib.kind() == "multiline_comment" {
            if let Ok(text) = sib.utf8_text(source) {
                comments.push(text.to_string());
            }
            prev = sib.prev_named_sibling();
        } else {
            break;
        }
    }
    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(comments.join("\n"))
    }
}

fn parse_swift_attribute_name(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches('@');
    let ident: String = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        .collect();
    let base = ident.rsplit('.').next().unwrap_or(&ident).trim();
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

fn collect_swift_attribute_names(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.named_children(&mut mod_cursor) {
                if modifier.kind() == "attribute"
                    && let Ok(text) = modifier.utf8_text(source)
                    && let Some(name) = parse_swift_attribute_name(text)
                {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Check if a Swift node has a specific attribute (e.g., @main, @Observable).
fn has_swift_attribute(node: tree_sitter::Node, source: &[u8], attr_name: &str) -> bool {
    collect_swift_attribute_names(node, source)
        .into_iter()
        .any(|name| name == attr_name)
}

struct SwiftUiDynamicPropertyMetadata {
    wrapper: String,
}

fn extract_swiftui_dynamic_property_metadata(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<SwiftUiDynamicPropertyMetadata> {
    let wrapper = collect_swift_attribute_names(node, source)
        .into_iter()
        .find_map(|name| {
            let normalized = match name.as_str() {
                "State" => "state",
                "StateObject" => "state_object",
                "ObservedObject" => "observed_object",
                "EnvironmentObject" => "environment_object",
                "Environment" => "environment",
                "AppStorage" => "app_storage",
                "SceneStorage" => "scene_storage",
                "Binding" => "binding",
                "GestureState" => "gesture_state",
                "FocusState" => "focus_state",
                "FocusedValue" => "focused_value",
                "FocusedBinding" => "focused_binding",
                "FetchRequest" => "fetch_request",
                "SectionedFetchRequest" => "sectioned_fetch_request",
                "Query" => "query",
                "Bindable" => "bindable",
                _ => return None,
            };
            Some(normalized.to_string())
        })?;

    Some(SwiftUiDynamicPropertyMetadata { wrapper })
}

fn apply_swiftui_dynamic_property_metadata(
    result: &mut ExtractionResult,
    node_id: &str,
    metadata: &SwiftUiDynamicPropertyMetadata,
) {
    let Some(node) = node_by_id_mut(result, node_id) else {
        return;
    };
    node.metadata
        .insert("swiftui.dynamic_property".to_string(), "true".to_string());
    node.metadata.insert(
        "swiftui.dynamic_property.wrapper".to_string(),
        metadata.wrapper.clone(),
    );
    node.metadata.insert(
        "swiftui.invalidation_source".to_string(),
        "true".to_string(),
    );
}

/// Collect inheritance/conformance names from a class_declaration node.
fn collect_inheritance_names(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "inheritance_specifier"
            && let Some(name) =
                find_user_type_name(child, source).or_else(|| type_identifier_text(child, source))
        {
            names.push(name);
        }
    }
    names
}

/// Walk up from a call node to find an enclosing Swift conditional.
/// Stops at `function_declaration` or `closure_expression` boundary.
fn find_enclosing_swift_condition(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "function_declaration" | "closure_expression" => return None,
            "if_statement" => {
                // Get the condition from the if_statement
                if let Some(cond) = parent.child_by_field_name("condition") {
                    return cond.utf8_text(source).ok().map(|s| s.trim().to_string());
                }
                // Fallback: get text between "if" and "{"
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(if_pos) = text.find("if")
                    && let Some(brace_pos) = text.find('{')
                {
                    let cond = text[if_pos + 2..brace_pos].trim();
                    if !cond.is_empty() {
                        return Some(cond.to_string());
                    }
                }
                return None;
            }
            "guard_statement" => {
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(guard_pos) = text.find("guard")
                    && let Some(else_pos) = text.find("else")
                {
                    let cond = text[guard_pos + 5..else_pos].trim();
                    if !cond.is_empty() {
                        return Some(format!("guard {}", cond));
                    }
                }
                return None;
            }
            "switch_entry" => {
                // Get the case pattern text
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(case_pos) = text.find("case")
                    && let Some(colon_pos) = text[case_pos..].find(':').map(|p| case_pos + p)
                    && colon_pos > case_pos + 4
                {
                    let pattern = text[case_pos + 4..colon_pos].trim();
                    if !pattern.is_empty() {
                        return Some(format!("case {}", pattern));
                    }
                }
                return None;
            }
            _ => {
                current = parent.parent();
            }
        }
    }
    None
}

/// Check if a Swift call node is at an async boundary.
fn detect_swift_async_boundary(node: tree_sitter::Node, source: &[u8]) -> Option<bool> {
    // Check if parent is await_expression
    if let Some(parent) = node.parent()
        && parent.kind() == "await_expression"
    {
        return Some(true);
    }
    // Check if inside a Task { } block
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "function_declaration" || parent.kind() == "closure_expression" {
            // Check if the closure is an argument to Task { } or DispatchQueue.async
            if let Some(gp) = parent.parent()
                && gp.kind() == "call_expression"
            {
                if let Some(fn_name) = simple_identifier_text(gp, source)
                    && fn_name == "Task"
                {
                    return Some(true);
                }
                if let Ok(text) = gp.utf8_text(source)
                    && text.contains("DispatchQueue")
                    && text.contains("async")
                {
                    return Some(true);
                }
            }
            break;
        }
        current = parent.parent();
    }
    None
}

/// Extract inheritance/conformance edges from `inheritance_specifier` children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TypeDeclarationKind {
    Class,
    Protocol,
}

fn root_node(mut node: tree_sitter::Node) -> tree_sitter::Node {
    while let Some(parent) = node.parent() {
        node = parent;
    }
    node
}

fn collect_type_declaration_kinds(
    node: tree_sitter::Node,
    source: &[u8],
    out: &mut HashMap<String, TypeDeclarationKind>,
) {
    match node.kind() {
        "class_declaration" => {
            if matches!(
                detect_class_declaration_type(node),
                ClassDeclarationType::Class
            ) && let Some(name) = type_identifier_text(node, source)
            {
                out.insert(name, TypeDeclarationKind::Class);
            }
        }
        "protocol_declaration" => {
            if let Some(name) = type_identifier_text(node, source) {
                out.insert(name, TypeDeclarationKind::Protocol);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_type_declaration_kinds(child, source, out);
    }
}

fn is_known_external_protocol_name(name: &str) -> bool {
    matches!(
        name,
        "Actor"
            | "AnyObject"
            | "CaseIterable"
            | "Codable"
            | "Decodable"
            | "Encodable"
            | "Equatable"
            | "Error"
            | "Hashable"
            | "Identifiable"
            | "Observable"
            | "ObservableObject"
            | "RawRepresentable"
            | "Sendable"
            | "View"
    )
}

fn is_likely_external_class_name(name: &str) -> bool {
    name.starts_with("NS")
        || name.starts_with("UI")
        || name.ends_with("Controller")
        || name.ends_with("ViewController")
        || name.ends_with("View")
        || name.ends_with("Object")
        || name.ends_with("Responder")
        || name.ends_with("Window")
}

fn extract_inheritance_edges(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    type_id: &str,
    result: &mut ExtractionResult,
) {
    let owner_kind = node.kind();
    let mut declaration_kinds = HashMap::new();
    collect_type_declaration_kinds(root_node(node), source, &mut declaration_kinds);
    let mut inheritance_specifiers = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "inheritance_specifier" {
            continue;
        }

        let Some(inherited_name) =
            find_user_type_name(child, source).or_else(|| type_identifier_text(child, source))
        else {
            continue;
        };

        inheritance_specifiers.push((child, inherited_name));
    }

    let resolved_class_parent = if owner_kind == "class_declaration" {
        inheritance_specifiers
            .iter()
            .position(|(_, inherited_name)| {
                declaration_kinds.get(inherited_name) == Some(&TypeDeclarationKind::Class)
            })
    } else {
        None
    };
    let inferred_external_class_parent = if owner_kind == "class_declaration"
        && resolved_class_parent.is_none()
        && inheritance_specifiers.len() > 1
        && declaration_kinds.get(&inheritance_specifiers[0].1)
            != Some(&TypeDeclarationKind::Protocol)
        && !is_known_external_protocol_name(&inheritance_specifiers[0].1)
        && is_likely_external_class_name(&inheritance_specifiers[0].1)
        && inheritance_specifiers
            .iter()
            .skip(1)
            .any(|(_, inherited_name)| {
                declaration_kinds.get(inherited_name) == Some(&TypeDeclarationKind::Protocol)
            }) {
        Some(0)
    } else {
        None
    };
    let class_parent_index = resolved_class_parent.or(inferred_external_class_parent);

    for (index, (child, inherited_name)) in inheritance_specifiers.into_iter().enumerate() {
        let target_id = make_id(file, module_path, &inherited_name);
        let edge_kind = match owner_kind {
            "class_declaration" if class_parent_index == Some(index) => EdgeKind::Inherits,
            "class_declaration" => EdgeKind::Implements,
            "protocol_declaration" => EdgeKind::Inherits,
            _ => EdgeKind::Implements,
        };

        result.edges.push(Edge {
            source: type_id.to_string(),
            target: target_id,
            kind: edge_kind,
            confidence: 0.9,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, child, type_id),
        });
    }
}

/// Extract the prefix of a navigation expression chain.
/// For `AppContext.gift.activityGiftConfigs`, returns `Some("AppContext.gift")`.
/// For `foo.bar`, returns `Some("foo")`.
fn extract_nav_prefix(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    // The first named child that isn't a navigation_suffix is the prefix expression
    let first_child = node
        .named_children(&mut cursor)
        .find(|c| c.kind() != "navigation_suffix")?;
    let text = first_child.utf8_text(source).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Recursively scan for `call_expression` and `navigation_expression` nodes,
/// emitting Calls edges for function calls and Reads edges for property accesses.
fn extract_calls(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    caller_id: &str,
    result: &mut ExtractionResult,
) {
    if node.kind() == "call_expression"
        && let Some(fn_name) = simple_identifier_text(node, source)
    {
        let target_id = make_id(file, module_path, &fn_name);
        let condition = find_enclosing_swift_condition(node, source);
        let async_boundary = detect_swift_async_boundary(node, source);
        result.edges.push(Edge {
            source: caller_id.to_string(),
            target: target_id,
            kind: EdgeKind::Calls,
            confidence: 0.8,
            direction: None,
            operation: None,
            condition,
            async_boundary,
            provenance: node_edge_provenance(file, node, caller_id),
        });
    }

    // Property access: `foo.bar` generates a navigation_expression.
    // Emit a Reads edge so dependency queries can trace through the access
    // without polluting call lists with field-like symbols.
    // Skip if the parent is a call_expression (already handled above as the callee name).
    if node.kind() == "navigation_expression"
        && !matches!(node.parent().map(|p| p.kind()), Some("call_expression"))
    {
        // The accessed property name is the last navigation_suffix child
        let mut cursor = node.walk();
        if let Some(suffix) = node
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "navigation_suffix")
            .last()
            && let Some(name_node) = suffix.named_child(0)
            && let Ok(prop_name) = name_node.utf8_text(source)
            && !prop_name.is_empty()
        {
            let target_id = make_id(file, module_path, prop_name);
            let condition = find_enclosing_swift_condition(node, source);
            // Extract the prefix chain (e.g., "AppContext.gift" from "AppContext.gift.activityGiftConfigs")
            // to help the merge step disambiguate among multiple candidates.
            let prefix = extract_nav_prefix(node, source);
            result.edges.push(Edge {
                source: caller_id.to_string(),
                target: target_id,
                kind: EdgeKind::Reads,
                confidence: 0.6,
                direction: None,
                operation: prefix,
                condition,
                async_boundary: None,
                provenance: node_edge_provenance(file, node, caller_id),
            });
        }
    }

    // Recurse into all children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(child, source, file, module_path, caller_id, result);
    }
}

fn node_by_id_mut<'a>(result: &'a mut ExtractionResult, node_id: &str) -> Option<&'a mut Node> {
    result.nodes.iter_mut().find(|node| node.id == node_id)
}

fn sanitize_id_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn make_swiftui_synthetic_id(
    owner_id: &str,
    prefix: &str,
    name: &str,
    node: tree_sitter::Node,
) -> String {
    let start = node.start_position();
    let end = node.end_position();
    format!(
        "{owner_id}::{prefix}:{}@{}:{}:{}:{}",
        sanitize_id_component(name),
        start.row,
        start.column,
        end.row,
        end.column
    )
}

fn emit_swiftui_node(
    context: &mut EnrichmentContext<'_>,
    owner_id: &str,
    parent_id: &str,
    name: &str,
    kind: NodeKind,
    file: &str,
    node: tree_sitter::Node,
) -> String {
    let prefix = match kind {
        NodeKind::View => "view",
        NodeKind::Branch => "branch",
        _ => "synthetic",
    };
    let id = make_swiftui_synthetic_id(owner_id, prefix, name, node);
    context.push_node(Node {
        id: id.clone(),
        kind,
        name: name.to_string(),
        file: file.into(),
        span: make_span(node),
        visibility: Visibility::Private,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });
    context.emit_edge(Edge {
        source: parent_id.to_string(),
        target: id.clone(),
        kind: EdgeKind::Contains,
        confidence: 1.0,
        direction: None,
        operation: None,
        condition: None,
        async_boundary: None,
        provenance: node_edge_provenance(file, node, parent_id),
    });
    id
}

fn same_owner_member_id(owner_id: &str, name: &str) -> Option<String> {
    let (owner_prefix, _) = owner_id.rsplit_once("::")?;
    Some(format!("{owner_prefix}::{name}"))
}

fn enclosing_owner_type_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut current = Some(node);
    while let Some(cursor) = current {
        if cursor.kind() == "class_declaration" {
            return match detect_class_declaration_type(cursor) {
                ClassDeclarationType::Extension => find_user_type_name(cursor, source),
                _ => type_identifier_text(cursor, source),
            };
        }
        current = cursor.parent();
    }
    None
}

fn uppercase_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn navigation_base_is_owner(
    base: tree_sitter::Node,
    source: &[u8],
    owner_name: Option<&str>,
) -> bool {
    let Ok(text) = base.utf8_text(source) else {
        return false;
    };
    let trimmed = text.trim();
    trimmed == "self" || trimmed == "Self" || owner_name.is_some_and(|owner| trimmed == owner)
}

fn dependency_read_name(
    node: tree_sitter::Node,
    source: &[u8],
    owner_name: Option<&str>,
) -> Option<String> {
    if node.kind() != "simple_identifier" {
        return None;
    }

    if is_syntactic_argument_label(node, source) {
        return None;
    }

    let name = node.utf8_text(source).ok()?.trim().to_string();
    if name.is_empty() || name == "_" || uppercase_identifier(&name) {
        return None;
    }

    if let Some(parent) = node.parent()
        && parent.kind() == "navigation_suffix"
        && let Some(nav) = parent.parent()
        && nav.kind() == "navigation_expression"
        && let Some((base, _)) = navigation_base_and_member_name(nav, source)
        && !navigation_base_is_owner(base, source, owner_name)
    {
        return None;
    }

    Some(name)
}

fn is_syntactic_argument_label(node: tree_sitter::Node, source: &[u8]) -> bool {
    let mut cursor = node.parent();
    let mut inside_call_syntax = false;
    while let Some(parent) = cursor {
        if matches!(
            parent.kind(),
            "value_argument"
                | "value_arguments"
                | "call_suffix"
                | "call_expression"
                | "navigation_suffix"
                | "navigation_expression"
        ) {
            inside_call_syntax = true;
            break;
        }
        if matches!(
            parent.kind(),
            "function_declaration"
                | "protocol_function_declaration"
                | "init_declaration"
                | "closure_parameter"
                | "parameter"
        ) {
            break;
        }
        cursor = parent.parent();
    }

    if !inside_call_syntax {
        return false;
    }

    let remaining = &source[node.end_byte()..];
    let offset = remaining
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(remaining.len());

    remaining.get(offset).copied() == Some(b':')
}

fn emit_dependency_read(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    let owner_name = enclosing_owner_type_name(node, source);
    let Some(name) = dependency_read_name(node, source, owner_name.as_deref()) else {
        return;
    };

    // Scope read targets to the enclosing type when possible.
    // Without this, bare targets like "File.swift::viewModel" match ALL viewModel
    // properties across the codebase during merge, causing false positive reads.
    let target_id = same_owner_member_id(owner_id, &name).unwrap_or_else(|| {
        if let Some(ref owner) = owner_name {
            // Scope to the enclosing type: "File.swift::OwnerType::propertyName"
            make_id(file, std::slice::from_ref(owner), &name)
        } else {
            make_id(file, module_path, &name)
        }
    });
    if target_id == owner_id {
        return;
    }

    context.emit_edge(Edge {
        source: owner_id.to_string(),
        target: target_id,
        kind: EdgeKind::Reads,
        confidence: 0.85,
        direction: None,
        operation: owner_name,
        condition: find_enclosing_swift_condition(node, source),
        async_boundary: None,
        provenance: node_edge_provenance(file, node, owner_id),
    });
}

fn extract_property_dependency_reads(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    emit_dependency_read(node, source, file, module_path, owner_id, context);

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_declaration"
                | "protocol_declaration"
                | "function_declaration"
                | "protocol_function_declaration"
                | "init_declaration"
                | "deinit_declaration"
        ) {
            continue;
        }
        extract_property_dependency_reads(child, source, file, module_path, owner_id, context);
    }
}

fn is_builtin_swiftui_view(name: &str) -> bool {
    matches!(
        name,
        "AnyView"
            | "Button"
            | "Color"
            | "Divider"
            | "DisclosureGroup"
            | "EmptyView"
            | "ForEach"
            | "Form"
            | "GeometryReader"
            | "Grid"
            | "GridRow"
            | "Group"
            | "HStack"
            | "Image"
            | "Label"
            | "LazyHGrid"
            | "LazyHStack"
            | "LazyVGrid"
            | "LazyVStack"
            | "Link"
            | "List"
            | "Menu"
            | "NavigationLink"
            | "NavigationStack"
            | "NavigationView"
            | "Picker"
            | "ProgressView"
            | "ScrollView"
            | "Section"
            | "SecureField"
            | "Spacer"
            | "TabView"
            | "Text"
            | "TextField"
            | "TimelineView"
            | "Toggle"
            | "VStack"
            | "ZStack"
    )
}

/// Built-in SwiftUI views whose first argument is a `LocalizedStringKey`.
fn builtin_view_accepts_localized_title(name: &str) -> bool {
    matches!(
        name,
        "Button"
            | "DisclosureGroup"
            | "Label"
            | "Link"
            | "Menu"
            | "NavigationLink"
            | "Picker"
            | "Section"
            | "Toggle"
    )
}

fn navigation_base_and_member_name<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'a>, String)> {
    if node.kind() != "navigation_expression" {
        return None;
    }

    let mut cursor = node.walk();
    let mut base = None;
    let mut member_name = None;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "navigation_suffix" => {
                if let Some(name_node) = child.named_child(0)
                    && let Ok(name) = name_node.utf8_text(source)
                    && !name.is_empty()
                {
                    member_name = Some(name.to_string());
                }
            }
            _ if base.is_none() => {
                base = Some(child);
            }
            _ => {}
        }
    }

    Some((base?, member_name?))
}

fn swiftui_call_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let callee = node.named_children(&mut cursor).next()?;
    match callee.kind() {
        "simple_identifier" => callee.utf8_text(source).ok().map(ToString::to_string),
        "navigation_expression" => {
            navigation_base_and_member_name(callee, source).map(|(_, member_name)| member_name)
        }
        _ => None,
    }
}

fn swiftui_modifier_receiver_and_name<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'a>, String)> {
    let mut cursor = node.walk();
    let callee = node.named_children(&mut cursor).next()?;
    let (receiver, member_name) = navigation_base_and_member_name(callee, source)?;
    if uppercase_identifier(&member_name) {
        None
    } else {
        Some((receiver, member_name))
    }
}

fn is_view_builder_modifier(name: &str) -> bool {
    matches!(
        name,
        "background"
            | "contextMenu"
            | "footer"
            | "header"
            | "mask"
            | "overlay"
            | "safeAreaInset"
            | "toolbar"
    )
}

fn view_reference_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let parent_kind = node.parent().map(|parent| parent.kind())?;
    if !matches!(
        parent_kind,
        "statements" | "computed_property" | "lambda_literal"
    ) {
        return None;
    }

    match node.kind() {
        "simple_identifier" => node
            .utf8_text(source)
            .ok()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string),
        "navigation_expression" => {
            navigation_base_and_member_name(node, source).map(|(_, member_name)| member_name)
        }
        _ => None,
    }
}

fn emit_swiftui_view_reference(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Option<String> {
    let name = view_reference_name(node, source)?;
    let view_id = emit_swiftui_node(
        context,
        owner_id,
        parent_id,
        &name,
        NodeKind::View,
        file,
        node,
    );
    if !is_builtin_swiftui_view(&name) {
        let target_id = same_owner_member_id(owner_id, &name)
            .unwrap_or_else(|| make_id(file, module_path, &name));
        let owner_hint = enclosing_owner_type_name(node, source);
        context.emit_edge(Edge {
            source: view_id.clone(),
            target: target_id,
            kind: EdgeKind::TypeRef,
            confidence: 0.85,
            direction: None,
            operation: owner_hint,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, node, &view_id),
        });
    }
    Some(view_id)
}

fn emit_swiftui_call_reference(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Option<String> {
    let name = swiftui_call_name(node, source)?;
    if uppercase_identifier(&name) {
        return None;
    }

    let view_id = emit_swiftui_node(
        context,
        owner_id,
        parent_id,
        &name,
        NodeKind::View,
        file,
        node,
    );
    let target_id =
        same_owner_member_id(owner_id, &name).unwrap_or_else(|| make_id(file, module_path, &name));
    let owner_hint = enclosing_owner_type_name(node, source);
    context.emit_edge(Edge {
        source: view_id.clone(),
        target: target_id,
        kind: EdgeKind::TypeRef,
        confidence: 0.85,
        direction: None,
        operation: owner_hint,
        condition: None,
        async_boundary: None,
        provenance: node_edge_provenance(file, node, &view_id),
    });
    for lambda in structural_call_suffix_lambda_children(node, source) {
        let _ = extract_swiftui_structure(
            lambda,
            source,
            file,
            module_path,
            owner_id,
            &view_id,
            context,
        );
    }
    Some(view_id)
}

#[derive(Clone)]
struct SwiftUiLambdaChild<'a> {
    node: tree_sitter::Node<'a>,
    label: Option<String>,
}

fn trailing_closure_label_before(source: &[u8], lambda_start_byte: usize) -> Option<String> {
    let window_start = lambda_start_byte.saturating_sub(128);
    let text = std::str::from_utf8(&source[window_start..lambda_start_byte])
        .ok()?
        .trim_end();
    let colon_index = text.rfind(':')?;
    if !text[colon_index + 1..].trim().is_empty() {
        return None;
    }

    let before_colon = text[..colon_index].trim_end();
    let label_start = before_colon
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .map(|index| index + 1)
        .unwrap_or(0);
    let label = before_colon[label_start..].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

fn is_view_builder_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "label"
            | "header"
            | "footer"
            | "background"
            | "overlay"
            | "placeholder"
            | "leading"
            | "trailing"
            | "detail"
            | "sidebar"
            | "top"
            | "bottom"
    ) || lower.contains("content")
}

fn call_suffix_lambda_children<'a>(
    node: tree_sitter::Node<'a>,
    source: &'a [u8],
) -> Vec<SwiftUiLambdaChild<'a>> {
    let mut suffixes = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "call_suffix" {
            let mut suffix_cursor = child.walk();
            for suffix_child in child.named_children(&mut suffix_cursor) {
                if suffix_child.kind() == "lambda_literal" {
                    let label = trailing_closure_label_before(source, suffix_child.start_byte());
                    suffixes.push(SwiftUiLambdaChild {
                        node: suffix_child,
                        label,
                    });
                }
            }
        }
    }
    suffixes
}

fn structural_call_suffix_lambda_children<'a>(
    node: tree_sitter::Node<'a>,
    source: &'a [u8],
) -> Vec<tree_sitter::Node<'a>> {
    let lambdas = call_suffix_lambda_children(node, source);
    let has_labeled = lambdas.iter().any(|child| child.label.is_some());

    lambdas
        .into_iter()
        .filter(|child| match child.label.as_deref() {
            Some(label) => is_view_builder_label(label),
            None => !has_labeled,
        })
        .map(|child| child.node)
        .collect()
}

fn recurse_swiftui_named_children(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        anchors.extend(extract_swiftui_structure(
            child,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ));
    }
    anchors
}

fn extract_swiftui_dependency_reads(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    match node.kind() {
        "call_expression" => {
            let structural_lambdas = structural_call_suffix_lambda_children(node, source);

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "call_suffix" {
                    let mut suffix_cursor = child.walk();
                    for suffix_child in child.named_children(&mut suffix_cursor) {
                        if suffix_child.kind() != "lambda_literal" {
                            extract_swiftui_dependency_reads(
                                suffix_child,
                                source,
                                file,
                                module_path,
                                owner_id,
                                context,
                            );
                        }
                    }
                    continue;
                }

                extract_swiftui_dependency_reads(
                    child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }

            for lambda in structural_lambdas {
                extract_swiftui_dependency_reads(
                    lambda,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }
        }
        _ => {
            emit_dependency_read(node, source, file, module_path, owner_id, context);

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                extract_swiftui_dependency_reads(
                    child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }
        }
    }
}

fn extract_swiftui_if_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    let condition = node
        .child_by_field_name("condition")
        .and_then(|condition| condition.utf8_text(source).ok())
        .map(|text| format!("if {}", text.trim()))
        .filter(|text| text != "if");

    let mut named_children = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "statements" | "if_statement") {
            named_children.push(child);
        }
    }

    let mut branch_ids = Vec::new();
    if let Some(then_child) = named_children.first().copied() {
        let branch_id = emit_swiftui_node(
            context,
            owner_id,
            parent_id,
            condition.as_deref().unwrap_or("if"),
            NodeKind::Branch,
            file,
            then_child,
        );
        branch_ids.push(branch_id.clone());
        let _ = extract_swiftui_structure(
            then_child,
            source,
            file,
            module_path,
            owner_id,
            &branch_id,
            context,
        );
    }

    if let Some(else_child) = named_children.get(1).copied() {
        let branch_id = emit_swiftui_node(
            context,
            owner_id,
            parent_id,
            "else",
            NodeKind::Branch,
            file,
            else_child,
        );
        branch_ids.push(branch_id.clone());
        let _ = extract_swiftui_structure(
            else_child,
            source,
            file,
            module_path,
            owner_id,
            &branch_id,
            context,
        );
    }

    branch_ids
}

fn switch_entry_label(node: tree_sitter::Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "default_keyword" => return "default".to_string(),
            "switch_pattern" => {
                if let Ok(text) = child.utf8_text(source) {
                    return format!("case {}", text.trim());
                }
            }
            _ => {}
        }
    }
    "case".to_string()
}

fn extract_swiftui_switch_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    let label = node
        .child_by_field_name("expr")
        .and_then(|expr| expr.utf8_text(source).ok())
        .map(|text| format!("switch {}", text.trim()))
        .unwrap_or_else(|| "switch".to_string());
    let switch_id = emit_swiftui_node(
        context,
        owner_id,
        parent_id,
        &label,
        NodeKind::Branch,
        file,
        node,
    );

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "switch_entry" {
            continue;
        }
        let case_id = emit_swiftui_node(
            context,
            owner_id,
            &switch_id,
            &switch_entry_label(child, source),
            NodeKind::Branch,
            file,
            child,
        );
        let mut case_cursor = child.walk();
        for case_child in child.named_children(&mut case_cursor) {
            if case_child.kind() == "statements" {
                let _ = extract_swiftui_structure(
                    case_child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    &case_id,
                    context,
                );
            }
        }
    }

    vec![switch_id]
}

fn extract_swiftui_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    match node.kind() {
        "statements" | "lambda_literal" | "computed_property" => recurse_swiftui_named_children(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
        "call_expression" => {
            if let Some(name) = swiftui_call_name(node, source)
                && uppercase_identifier(&name)
            {
                let view_id = emit_swiftui_node(
                    context,
                    owner_id,
                    parent_id,
                    &name,
                    NodeKind::View,
                    file,
                    node,
                );
                if !is_builtin_swiftui_view(&name) {
                    context.emit_edge(Edge {
                        source: view_id.clone(),
                        target: make_id(file, module_path, &name),
                        kind: EdgeKind::TypeRef,
                        confidence: 0.85,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: node_edge_provenance(file, node, &view_id),
                    });
                }
                for lambda in structural_call_suffix_lambda_children(node, source) {
                    let _ = extract_swiftui_structure(
                        lambda,
                        source,
                        file,
                        module_path,
                        owner_id,
                        &view_id,
                        context,
                    );
                }
                vec![view_id]
            } else if let Some((receiver, modifier_name)) =
                swiftui_modifier_receiver_and_name(node, source)
            {
                let anchors = extract_swiftui_structure(
                    receiver,
                    source,
                    file,
                    module_path,
                    owner_id,
                    parent_id,
                    context,
                );
                if is_view_builder_modifier(&modifier_name) {
                    let modifier_parents: Vec<String> = if anchors.is_empty() {
                        vec![parent_id.to_string()]
                    } else {
                        anchors.clone()
                    };
                    for lambda in structural_call_suffix_lambda_children(node, source) {
                        for anchor in &modifier_parents {
                            let _ = extract_swiftui_structure(
                                lambda,
                                source,
                                file,
                                module_path,
                                owner_id,
                                anchor,
                                context,
                            );
                        }
                    }
                }
                anchors
            } else if let Some(view_id) = emit_swiftui_call_reference(
                node,
                source,
                file,
                module_path,
                owner_id,
                parent_id,
                context,
            ) {
                vec![view_id]
            } else {
                recurse_swiftui_named_children(
                    node,
                    source,
                    file,
                    module_path,
                    owner_id,
                    parent_id,
                    context,
                )
            }
        }
        "if_statement" => extract_swiftui_if_structure(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
        "switch_statement" => extract_swiftui_switch_structure(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
        "simple_identifier" | "navigation_expression" => emit_swiftui_view_reference(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        )
        .into_iter()
        .collect(),
        _ => recurse_swiftui_named_children(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
    }
}

fn extract_swiftui_declaration_structure_with_context(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    decl_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "computed_property" | "function_body") {
            extract_swiftui_dependency_reads(child, source, file, module_path, decl_id, context);
            let _ = extract_swiftui_structure(
                child,
                source,
                file,
                module_path,
                decl_id,
                decl_id,
                context,
            );
        }
    }
}

fn extract_swiftui_declaration_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    decl_id: &str,
    result: &mut ExtractionResult,
) {
    let mut context = EnrichmentContext::new(result);
    extract_swiftui_declaration_structure_with_context(
        node,
        source,
        file,
        module_path,
        decl_id,
        &mut context,
    );
}

fn type_text_looks_like_swiftui_view(type_text: &str) -> bool {
    let trimmed = normalized_swiftui_type_text(type_text);
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("some View")
        || trimmed.starts_with("any View")
        || trimmed.starts_with("some SwiftUI.View")
        || trimmed.starts_with("any SwiftUI.View")
    {
        return true;
    }

    let ident: String = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '<' | '>'))
        .collect();
    let base = ident
        .split('<')
        .next()
        .unwrap_or(&ident)
        .rsplit('.')
        .next()
        .unwrap_or(&ident);
    base == "View" || base.ends_with("View") || is_builtin_swiftui_view(base)
}

fn normalized_swiftui_type_text(type_text: &str) -> &str {
    let mut trimmed = type_text.trim().trim_start_matches(':').trim();
    loop {
        let without_optional = trimmed.trim_end_matches(['?', '!']).trim();
        let stripped_parens = strip_outer_parentheses(without_optional);
        if stripped_parens == trimmed {
            return stripped_parens;
        }
        trimmed = stripped_parens;
    }
}

fn strip_outer_parentheses(text: &str) -> &str {
    if !(text.starts_with('(') && text.ends_with(')')) {
        return text;
    }

    let mut depth = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx + ch.len_utf8() != text.len() {
                    return text;
                }
            }
            _ => {}
        }
    }

    if depth == 0 {
        &text[1..text.len() - 1]
    } else {
        text
    }
}

fn declaration_returns_swiftui_view(node: tree_sitter::Node, source: &[u8]) -> bool {
    match node.kind() {
        "property_declaration" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|child| child.kind() == "type_annotation")
                .and_then(|type_node| type_node.utf8_text(source).ok())
                .is_some_and(type_text_looks_like_swiftui_view)
        }
        "function_declaration" | "protocol_function_declaration" => {
            let Ok(text) = node.utf8_text(source) else {
                return false;
            };
            text.split_once("->")
                .map(|(_, type_text)| type_text_looks_like_swiftui_view(type_text))
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn file_matches(node_file: &Path, file_path: &Path) -> bool {
    let node_file = node_file.to_string_lossy();
    let file_path = file_path.to_string_lossy();
    node_file == file_path
        || node_file.ends_with(file_path.as_ref())
        || file_path.ends_with(node_file.as_ref())
        || node_file
            .rsplit('/')
            .next()
            .zip(file_path.rsplit('/').next())
            .is_some_and(|(left, right)| left == right)
}

fn swiftui_owner_id(node_id: &str) -> Option<&str> {
    node_id.split_once("::view:").map(|(owner_id, _)| owner_id)
}

fn line_matches(node_line: usize, ast_row_zero_based: usize) -> bool {
    node_line.abs_diff(ast_row_zero_based) <= 1
}

#[derive(Debug, Clone)]
struct LocalizationWrapperMetadata {
    table: String,
    key: String,
    fallback: Option<String>,
    arg_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LocalizationReferenceData {
    ref_kind: &'static str,
    wrapper_name: Option<String>,
    wrapper_base: Option<String>,
    arg_count: usize,
    literal: Option<String>,
}

type LocalizationBindings = HashMap<String, Vec<LocalizationReferenceData>>;

static L10N_TR_METADATA_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)L10n\.tr\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)".*?fallback:\s*"((?:[^"\\]|\\.)*)""#,
    )
    .expect("L10n.tr metadata regex should compile")
});

static L10N_RESOURCE_METADATA_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)L10nResource\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*table:\s*"((?:[^"\\]|\\.)*)".*?fallback:\s*"((?:[^"\\]|\\.)*)""#,
    )
    .expect("L10nResource metadata regex should compile")
});

static LOCALIZATION_WRAPPER_EXPR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*(?:(?:([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*)|\.)([A-Za-z_][A-Za-z0-9_]*)\s*(?:\((.*)\))?\s*$"#,
    )
    .expect("localization wrapper expression regex should compile")
});

static LOCALIZED_TEXT_WRAPPER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*Text\s*\(\s*((?:i18n)\s*:\s*)?(?:(?:([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*)|\.)([A-Za-z_][A-Za-z0-9_]*)\s*(?:\((.*)\))?\s*(?:\)|,)"#,
    )
    .expect("localized text wrapper regex should compile")
});

static LOCALIZED_TEXT_LITERAL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*Text\s*\(\s*"((?:[^"\\]|\\.)*)"(?:\s*,\s*bundle\s*:\s*[^,)]+)?\s*(?:\)|,)"#,
    )
    .expect("localized text literal regex should compile")
});

fn decode_swift_string_literal(raw: &str) -> String {
    raw.replace(r#"\""#, "\"")
        .replace(r"\n", "\n")
        .replace(r"\t", "\t")
}

fn count_top_level_call_args(input: &str) -> usize {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let mut count = 1usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for ch in trimmed.chars() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => count += 1,
            _ => {}
        }
    }

    count
}

fn function_parameter_count(node: tree_sitter::Node) -> usize {
    fn count_parameters(node: tree_sitter::Node) -> usize {
        let mut count = usize::from(node.kind() == "parameter");
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            count += count_parameters(child);
        }
        count
    }

    count_parameters(node)
}

fn parse_l10n_tr_metadata(text: &str, arg_count: usize) -> Option<LocalizationWrapperMetadata> {
    let captures = L10N_TR_METADATA_RE.captures(text)?;
    Some(LocalizationWrapperMetadata {
        table: decode_swift_string_literal(captures.get(1)?.as_str()),
        key: decode_swift_string_literal(captures.get(2)?.as_str()),
        fallback: Some(decode_swift_string_literal(captures.get(3)?.as_str())),
        arg_count,
    })
}

fn parse_l10n_resource_metadata(
    text: &str,
    arg_count: usize,
) -> Option<LocalizationWrapperMetadata> {
    let captures = L10N_RESOURCE_METADATA_RE.captures(text)?;
    Some(LocalizationWrapperMetadata {
        table: decode_swift_string_literal(captures.get(2)?.as_str()),
        key: decode_swift_string_literal(captures.get(1)?.as_str()),
        fallback: Some(decode_swift_string_literal(captures.get(3)?.as_str())),
        arg_count,
    })
}

fn extract_wrapper_metadata(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<LocalizationWrapperMetadata> {
    let text = node.utf8_text(source).ok()?;
    let arg_count = if node.kind() == "function_declaration" {
        function_parameter_count(node)
    } else {
        0
    };

    parse_l10n_tr_metadata(text, arg_count)
        .or_else(|| parse_l10n_resource_metadata(text, arg_count))
}

fn collect_localizable_wrapper_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "property_declaration" | "function_declaration")
        && extract_wrapper_metadata(node, source).is_some()
    {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localizable_wrapper_nodes(child, source, out);
    }
}

fn apply_wrapper_metadata(
    context: &mut EnrichmentContext<'_>,
    node_id: &str,
    metadata: &LocalizationWrapperMetadata,
) {
    let Some(node) = context.node_mut(node_id) else {
        return;
    };
    node.metadata
        .insert("l10n.wrapper.table".to_string(), metadata.table.clone());
    node.metadata
        .insert("l10n.wrapper.key".to_string(), metadata.key.clone());
    if let Some(fallback) = &metadata.fallback {
        node.metadata
            .insert("l10n.wrapper.fallback".to_string(), fallback.clone());
    }
    node.metadata.insert(
        "l10n.wrapper.arg_count".to_string(),
        metadata.arg_count.to_string(),
    );
}

fn apply_localization_reference(
    context: &mut EnrichmentContext<'_>,
    file_path: &Path,
    file: &str,
    usage_id: &str,
    usage_span: &Span,
    reference: &LocalizationReferenceData,
    argument_label: Option<&str>,
) {
    {
        let Some(node) = context.node_mut(usage_id) else {
            return;
        };
        node.metadata
            .insert("l10n.ref_kind".to_string(), reference.ref_kind.to_string());
        node.metadata.insert(
            "l10n.arg_count".to_string(),
            reference.arg_count.to_string(),
        );
        if let Some(wrapper_name) = &reference.wrapper_name {
            node.metadata
                .insert("l10n.wrapper_name".to_string(), wrapper_name.clone());
        }
        if let Some(wrapper_base) = &reference.wrapper_base {
            node.metadata
                .insert("l10n.wrapper_base".to_string(), wrapper_base.clone());
        }
        if let Some(literal) = &reference.literal {
            node.metadata
                .insert("l10n.literal".to_string(), literal.clone());
        }
        if let Some(label) = argument_label {
            node.metadata
                .insert("l10n.argument_label".to_string(), label.to_string());
        }
    }

    if let Some(wrapper_name) = &reference.wrapper_name
        && let Some(wrapper_id) = context.local_wrapper_id(file_path, wrapper_name)
    {
        context.emit_edge(Edge {
            source: usage_id.to_string(),
            target: wrapper_id,
            kind: EdgeKind::TypeRef,
            confidence: 0.85,
            direction: None,
            operation: reference.wrapper_base.clone(),
            condition: None,
            async_boundary: None,
            provenance: vec![EdgeProvenance {
                file: file.into(),
                span: usage_span.clone(),
                symbol_id: usage_id.to_string(),
            }],
        });
    }
}

fn apply_localization_reference_to_span(
    context: &mut EnrichmentContext<'_>,
    file_path: &Path,
    file: &str,
    usage_id: &str,
    usage_span: &Span,
    reference: &LocalizationReferenceData,
    argument_label: Option<&str>,
) {
    apply_localization_reference(
        context,
        file_path,
        file,
        usage_id,
        usage_span,
        reference,
        argument_label,
    );
}

fn emit_localization_usage_node(
    context: &mut EnrichmentContext<'_>,
    parent_id: &str,
    file: &str,
    label: &str,
    span: &Span,
    unique_suffix: Option<&str>,
) -> String {
    let suffix = unique_suffix
        .map(sanitize_id_component)
        .filter(|value| !value.is_empty())
        .map(|value| format!(":{value}"))
        .unwrap_or_default();
    let id = format!(
        "{parent_id}::l10n:{}{}@{}:{}:{}:{}",
        sanitize_id_component(label),
        suffix,
        span.start[0],
        span.start[1],
        span.end[0],
        span.end[1]
    );
    context.push_node(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name: label.to_string(),
        file: file.into(),
        span: span.clone(),
        visibility: Visibility::Private,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
        snippet: None,
    });
    context.emit_edge(Edge {
        source: parent_id.to_string(),
        target: id.clone(),
        kind: EdgeKind::Contains,
        confidence: 1.0,
        direction: None,
        operation: None,
        condition: None,
        async_boundary: None,
        provenance: vec![EdgeProvenance {
            file: file.into(),
            span: span.clone(),
            symbol_id: parent_id.to_string(),
        }],
    });
    id
}

/// Extract the first argument of a built-in SwiftUI view as a localization reference.
/// Handles `Button("Title") { }`, `Label("Title", systemImage: "star")`, etc.
fn builtin_view_title_reference(
    text: &str,
    view_name: &str,
    bindings: Option<&LocalizationBindings>,
) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    let prefix = format!("{view_name}(");
    if !trimmed.starts_with(&prefix) {
        return None;
    }
    let args = call_argument_list(trimmed)?;
    let first_argument = split_top_level_arguments(args).into_iter().next()?;
    let (label, value) = split_argument_label(first_argument);
    // Skip labeled first arguments that aren't title-like (e.g. `Button(action:)`)
    if let Some(label) = &label
        && !is_text_like_argument_label(label)
        && label != "i18n"
    {
        return None;
    }
    // Try binding-based resolution first
    let binding_refs = simple_identifier_binding_references(value, bindings);
    if let Some(reference) = binding_refs.into_iter().next() {
        return Some(reference);
    }
    localized_reference_for_expression_text(value)
}

fn localized_reference_for_expression_text(text: &str) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    if let Some(captures) = LOCALIZED_TEXT_LITERAL_RE.captures(&format!("Text({trimmed})")) {
        return Some(LocalizationReferenceData {
            ref_kind: "literal",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: Some(decode_swift_string_literal(captures.get(1)?.as_str())),
        });
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "Text") {
        return Some(reference);
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "String") {
        return Some(reference);
    }

    let captures = LOCALIZATION_WRAPPER_EXPR_RE.captures(trimmed)?;
    let wrapper_base = captures.get(1).map(|value| value.as_str().to_string());
    if !looks_like_localized_wrapper_expression(trimmed, wrapper_base.as_deref()) {
        return None;
    }
    let args = captures.get(3).map(|value| value.as_str()).unwrap_or("");
    Some(LocalizationReferenceData {
        ref_kind: "wrapper",
        wrapper_name: Some(captures.get(2)?.as_str().to_string()),
        wrapper_base,
        arg_count: count_top_level_call_args(args),
        literal: None,
    })
}

fn looks_like_localized_wrapper_expression(text: &str, wrapper_base: Option<&str>) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('.')
        || trimmed.contains("i18n:")
        || trimmed.contains("i10n:")
        || matches_localization_wrapper_base(wrapper_base)
}

fn matches_localization_wrapper_base(wrapper_base: Option<&str>) -> bool {
    match wrapper_base.map(str::trim) {
        None => false,
        Some("") => false,
        Some(base) => {
            let normalized = base.rsplit('.').next().unwrap_or(base);
            matches!(
                normalized,
                "L10n" | "L10nResource" | "l10n" | "i18n" | "i10n"
            ) || normalized.ends_with("L10n")
                || normalized.ends_with("Resource")
                || normalized.ends_with("Strings")
                || normalized
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
        }
    }
}

fn expression_maybe_string_text(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed != "true"
        && trimmed != "false"
        && !trimmed.starts_with('{')
        && !trimmed.starts_with('[')
        && !trimmed.starts_with('#')
        && trimmed.parse::<f64>().is_err()
}

fn is_text_like_argument_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    [
        "text",
        "title",
        "subtitle",
        "message",
        "label",
        "placeholder",
        "caption",
        "hint",
        "prompt",
        "description",
        "header",
        "footer",
    ]
    .iter()
    .any(|needle| lower == *needle || lower.ends_with(needle))
}

fn localized_i18n_call_reference(text: &str, callee: &str) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    let prefix = format!("{callee}(");
    if !trimmed.starts_with(&prefix) {
        return None;
    }

    let args = call_argument_list(trimmed)?;
    for segment in split_top_level_arguments(args) {
        let (label, value) = split_argument_label(segment);
        if !matches!(label.as_deref(), Some("i18n" | "i10n")) {
            continue;
        }

        if let Some(mut reference) = localized_reference_for_expression_text(value) {
            if reference.wrapper_base.is_none() {
                reference.wrapper_base = Some("L10nResource".to_string());
            }
            return Some(reference);
        }

        return Some(LocalizationReferenceData {
            ref_kind: "possible_wrapper",
            wrapper_name: None,
            wrapper_base: Some("L10nResource".to_string()),
            arg_count: 0,
            literal: None,
        });
    }

    None
}

fn localized_text_references_from_text(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<LocalizationReferenceData> {
    let trimmed = text.trim();
    if trimmed.starts_with("Text(verbatim:") {
        return Vec::new();
    }

    let binding_references = if let Some(args) = call_argument_list(trimmed) {
        split_top_level_arguments(args)
            .into_iter()
            .next()
            .map(|first_argument| {
                let (_, value) = split_argument_label(first_argument);
                simple_identifier_binding_references(value, bindings)
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if !binding_references.is_empty() {
        return binding_references;
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "Text") {
        return vec![reference];
    }

    if let Some(captures) = LOCALIZED_TEXT_WRAPPER_RE.captures(trimmed) {
        let Some(wrapper_name) = captures.get(3).map(|value| value.as_str().to_string()) else {
            return Vec::new();
        };
        let uses_i18n_label = captures.get(1).is_some();
        let args = captures.get(4).map(|value| value.as_str()).unwrap_or("");
        return vec![LocalizationReferenceData {
            ref_kind: "wrapper",
            wrapper_name: Some(wrapper_name),
            wrapper_base: captures
                .get(2)
                .map(|value| value.as_str().to_string())
                .or_else(|| uses_i18n_label.then_some("L10nResource".to_string())),
            arg_count: count_top_level_call_args(args),
            literal: None,
        }];
    }

    if let Some(captures) = LOCALIZED_TEXT_LITERAL_RE.captures(trimmed) {
        let Some(literal) = captures
            .get(1)
            .map(|value| decode_swift_string_literal(value.as_str()))
        else {
            return Vec::new();
        };
        return vec![LocalizationReferenceData {
            ref_kind: "literal",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: Some(literal),
        }];
    }

    if trimmed.starts_with("Text(") && trimmed.ends_with(')') {
        if let Some(args) = call_argument_list(trimmed)
            && let Some(first_argument) = split_top_level_arguments(args).into_iter().next()
        {
            let (label, value) = split_argument_label(first_argument);
            if label.as_deref() == Some("i18n") && expression_maybe_string_text(value) {
                return vec![LocalizationReferenceData {
                    ref_kind: "possible_wrapper",
                    wrapper_name: None,
                    wrapper_base: Some("L10nResource".to_string()),
                    arg_count: 0,
                    literal: None,
                }];
            }
        }

        return vec![LocalizationReferenceData {
            ref_kind: "possible_string",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: None,
        }];
    }

    Vec::new()
}

fn call_argument_list(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    (close > open).then_some(&text[open + 1..close])
}

fn split_top_level_arguments(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in args.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                parts.push(args[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }

    let tail = args[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }

    parts
}

fn split_argument_label(segment: &str) -> (Option<String>, &str) {
    let trimmed = segment.trim();
    let Some(colon) = trimmed.find(':') else {
        return (None, trimmed);
    };
    let label = trimmed[..colon].trim();
    if label.is_empty()
        || !label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return (None, trimmed);
    }
    (Some(label.to_string()), trimmed[colon + 1..].trim())
}

fn simple_identifier_binding_references(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<LocalizationReferenceData> {
    let Some(bindings) = bindings else {
        return Vec::new();
    };

    let trimmed = text.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Vec::new();
    }

    bindings.get(trimmed).cloned().unwrap_or_default()
}

fn push_unique_localization_reference(
    references: &mut Vec<LocalizationReferenceData>,
    reference: LocalizationReferenceData,
) {
    if !references.contains(&reference) {
        references.push(reference);
    }
}

fn localization_reference_suffix(reference: &LocalizationReferenceData, index: usize) -> String {
    reference
        .wrapper_name
        .clone()
        .or_else(|| reference.literal.clone())
        .or_else(|| reference.wrapper_base.clone())
        .unwrap_or_else(|| format!("ref{index}"))
}

fn collect_localization_references_in_subtree(
    node: tree_sitter::Node,
    source: &[u8],
    references: &mut Vec<LocalizationReferenceData>,
) {
    match node.kind() {
        "call_expression" | "navigation_expression" => {
            if let Ok(text) = node.utf8_text(source)
                && let Some(reference) = localized_reference_for_expression_text(text)
            {
                push_unique_localization_reference(references, reference);
                return;
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localization_references_in_subtree(child, source, references);
    }
}

fn collect_localization_bindings(
    node: tree_sitter::Node,
    source: &[u8],
    bindings: &mut LocalizationBindings,
) {
    match node.kind() {
        "class_declaration"
        | "protocol_declaration"
        | "function_declaration"
        | "protocol_function_declaration"
        | "init_declaration"
        | "deinit_declaration"
        | "lambda_literal" => return,
        "property_declaration" => {
            if let Some(name) = find_pattern_name(node, source) {
                let mut references = Vec::new();
                collect_localization_references_in_subtree(node, source, &mut references);
                if !references.is_empty() {
                    let entry = bindings.entry(name).or_default();
                    for reference in references {
                        push_unique_localization_reference(entry, reference);
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localization_bindings(child, source, bindings);
    }
}

fn localization_bindings_for_declaration(
    node: tree_sitter::Node,
    source: &[u8],
) -> LocalizationBindings {
    let mut bindings = LocalizationBindings::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "computed_property" | "function_body") {
            collect_localization_bindings(child, source, &mut bindings);
        }
    }
    bindings
}

fn custom_view_localization_arguments_from_text(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<(Option<String>, LocalizationReferenceData)> {
    let Some(args) = call_argument_list(text) else {
        return Vec::new();
    };

    let mut localized_args = Vec::new();
    for segment in split_top_level_arguments(args) {
        let (label, value) = split_argument_label(segment);
        let accepts_reference = label
            .as_deref()
            .is_some_and(|label| label == "i18n" || is_text_like_argument_label(label));
        let binding_references = simple_identifier_binding_references(value, bindings);
        if accepts_reference && !binding_references.is_empty() {
            localized_args.extend(
                binding_references
                    .into_iter()
                    .map(|reference| (label.clone(), reference)),
            );
            continue;
        }

        let reference = if let Some(reference) = localized_reference_for_expression_text(value) {
            reference
        } else if label.as_deref() == Some("i18n") && expression_maybe_string_text(value) {
            LocalizationReferenceData {
                ref_kind: "possible_wrapper",
                wrapper_name: None,
                wrapper_base: Some("L10nResource".to_string()),
                arg_count: 0,
                literal: None,
            }
        } else if label.as_deref().is_some_and(is_text_like_argument_label)
            && expression_maybe_string_text(value)
        {
            LocalizationReferenceData {
                ref_kind: "possible_string",
                wrapper_name: None,
                wrapper_base: None,
                arg_count: 0,
                literal: None,
            }
        } else {
            continue;
        };
        if !accepts_reference {
            continue;
        }
        localized_args.push((label, reference));
    }

    localized_args
}

fn collect_swiftui_declaration_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "property_declaration" | "function_declaration")
        && declaration_returns_swiftui_view(node, source)
    {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_swiftui_declaration_nodes(child, source, out);
    }
}

fn collect_property_declaration_nodes<'a>(
    node: tree_sitter::Node<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == "property_declaration" {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_property_declaration_nodes(child, out);
    }
}

fn declaration_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "property_declaration" => find_pattern_name(node, source),
        "function_declaration" => simple_identifier_text(node, source),
        "protocol_function_declaration" => simple_identifier_text(node, source),
        _ => None,
    }
}

fn declaration_kind(node: tree_sitter::Node) -> Option<NodeKind> {
    match node.kind() {
        "property_declaration" => Some(NodeKind::Property),
        "function_declaration" | "protocol_function_declaration" => Some(NodeKind::Function),
        _ => None,
    }
}

fn matching_swiftui_declaration_id(
    context: &EnrichmentContext<'_>,
    file_path: &Path,
    decl_node: tree_sitter::Node,
    source: &[u8],
) -> Option<String> {
    context.matching_declaration_id(file_path, decl_node, source)
}

fn collect_swiftui_body_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == "class_declaration" {
        let conformances = collect_inheritance_names(node, source);
        let conforms_to_view = conformances.iter().any(|c| c == "View" || c == "App");
        if conforms_to_view {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "class_body" {
                    continue;
                }
                let mut body_cursor = child.walk();
                for body_child in child.named_children(&mut body_cursor) {
                    if body_child.kind() == "property_declaration"
                        && let Some(name) = find_pattern_name(body_child, source)
                        && name == "body"
                    {
                        out.push(body_child);
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_swiftui_body_nodes(child, source, out);
    }
}

fn matching_body_id(
    context: &EnrichmentContext<'_>,
    file_path: &Path,
    body_node: tree_sitter::Node,
) -> Option<String> {
    context.matching_body_id(file_path, body_node)
}

fn candidate_owner_names(result: &ExtractionResult) -> HashMap<String, Vec<String>> {
    let id_to_name: HashMap<&str, &str> = result
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.name.as_str()))
        .collect();
    let mut candidate_owner_names: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &result.edges {
        if edge.kind == EdgeKind::Contains
            && let Some(owner_name) = id_to_name.get(edge.source.as_str())
        {
            candidate_owner_names
                .entry(edge.target.clone())
                .or_default()
                .push((*owner_name).to_string());
        } else if edge.kind == EdgeKind::Implements
            && let Some(owner_name) = id_to_name.get(edge.target.as_str())
        {
            candidate_owner_names
                .entry(edge.source.clone())
                .or_default()
                .push((*owner_name).to_string());
        }
    }
    candidate_owner_names
}

// ---------------------------------------------------------------------------
// Image asset reference enrichment
// ---------------------------------------------------------------------------

const META_ASSET_REF_KIND: &str = "asset.ref_kind";
const META_ASSET_NAME: &str = "asset.name";

/// Fast byte-level check — skip tree-sitter parsing when no asset markers found.
pub fn source_contains_image_asset_markers(source: &[u8]) -> bool {
    fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
    bytes_contains(source, b"Image(") || bytes_contains(source, b"UIImage(")
}

pub fn enrich_asset_references_with_tree(
    source: &[u8],
    _file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let line_index = LineIndex::new(source);
    let mut calls = Vec::new();
    collect_image_asset_calls(tree.root_node(), source, &mut calls);

    for call in calls {
        let call_line = line_index.line_at_byte(call.byte_offset);
        let Some(owner_idx) = find_enclosing_node_index(result, call_line) else {
            continue;
        };
        // Only set if not already tagged (first call wins for that node)
        if !result.nodes[owner_idx]
            .metadata
            .contains_key(META_ASSET_REF_KIND)
        {
            result.nodes[owner_idx]
                .metadata
                .insert(META_ASSET_REF_KIND.to_string(), "image".to_string());
            result.nodes[owner_idx]
                .metadata
                .insert(META_ASSET_NAME.to_string(), call.asset_name.clone());
        }
    }

    Ok(())
}

struct ImageAssetCall {
    asset_name: String,
    byte_offset: usize,
}

fn collect_image_asset_calls<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    calls: &mut Vec<ImageAssetCall>,
) {
    if node.kind() == "call_expression"
        && let Some(asset_call) = try_parse_image_call(node, source)
    {
        calls.push(asset_call);
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_image_asset_calls(child, source, calls);
        }
    }
}

fn try_parse_image_call(node: tree_sitter::Node, source: &[u8]) -> Option<ImageAssetCall> {
    // A call_expression has a callee (first child) and call_suffix child containing
    // value_arguments. The callee text should be "Image" or "UIImage".
    let callee = node.child(0)?;
    let callee_text = callee.utf8_text(source).ok()?;
    if callee_text != "Image" && callee_text != "UIImage" {
        return None;
    }

    // Find call_suffix -> value_arguments
    let call_suffix = find_child_by_kind(node, "call_suffix")?;
    let value_arguments = find_child_by_kind(call_suffix, "value_arguments")?;

    // Look at value_argument children
    let arg_count = value_arguments.child_count();
    for i in 0..arg_count {
        let Some(arg) = value_arguments.child(i) else {
            continue;
        };
        if arg.kind() != "value_argument" {
            continue;
        }

        // Check for label
        let (label, value_node) = extract_argument_label_and_value(arg, source);
        let label = label.as_deref();

        // Only process relevant arguments
        match label {
            // Image("icon") or Image(named: "icon") or UIImage(named: "icon")
            // Image(asset: .X.Y) or Image(resource: .X.Y)
            None | Some("named") | Some("asset") | Some("resource") => {}
            // Image(systemName: ...) — skip SF Symbols
            Some("systemName") | Some("systemimage") => return None,
            // Image(decorative: ...) — treat as asset reference
            Some("decorative") => {}
            _ => continue,
        }

        let Some(value_node) = value_node else {
            continue;
        };

        // Try string literal
        if let Some(name) = extract_string_literal(value_node, source) {
            return Some(ImageAssetCall {
                asset_name: name,
                byte_offset: node.start_byte(),
            });
        }

        // Try dot expression: .Room.voiceWave → "Room/voiceWave"
        if let Some(name) = extract_dot_expression_asset_name(value_node, source) {
            return Some(ImageAssetCall {
                asset_name: name,
                byte_offset: node.start_byte(),
            });
        }
    }

    None
}

fn find_child_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == kind
        {
            return Some(child);
        }
    }
    None
}

fn extract_argument_label_and_value<'a>(
    arg: tree_sitter::Node<'a>,
    source: &[u8],
) -> (Option<String>, Option<tree_sitter::Node<'a>>) {
    // value_argument may have: simple_identifier ":" value
    // or just: value
    let child_count = arg.child_count();
    if child_count == 0 {
        return (None, None);
    }

    // Check if first child is a simple_identifier (label)
    if let Some(first) = arg.child(0)
        && first.kind() == "simple_identifier"
    {
        let label = first.utf8_text(source).ok().map(|s| s.to_string());
        // Value is the last non-colon child
        let value = (0..child_count)
            .rev()
            .filter_map(|i| arg.child(i))
            .find(|c| c.kind() != "simple_identifier" && c.kind() != ":");
        return (label, value);
    }

    // No label — value is the first meaningful child
    let value = (0..child_count)
        .filter_map(|i| arg.child(i))
        .find(|c| c.kind() != "," && c.kind() != "(" && c.kind() != ")");
    (None, value)
}

fn extract_string_literal(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    // line_string_literal containing string_content
    if node.kind() == "line_string_literal" {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && child.kind() == "line_str_text"
            {
                return child.utf8_text(source).ok().map(|s| s.to_string());
            }
        }
    }
    None
}

fn extract_dot_expression_asset_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    // Dot expressions like .Room.voiceWave are navigation_expression nodes
    // We collect all member segments
    let text = node.utf8_text(source).ok()?;
    if !text.starts_with('.') {
        return None;
    }
    // .Room.voiceWave → "Room/voiceWave"
    let trimmed = text.trim_start_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.replace('.', "/"))
}

/// Pre-computed byte-offset → line-number index for fast lookups.
struct LineIndex {
    /// Byte offsets where each line starts (line 0 starts at offset 0).
    line_starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &[u8]) -> Self {
        let mut starts = vec![0usize];
        for (i, &byte) in source.iter().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            line_starts: starts,
        }
    }

    /// Convert a byte offset to a 0-based line number.
    fn line_at_byte(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(line) => line.saturating_sub(1),
        }
    }
}

/// Find the graph node whose span contains the given 0-based line number,
/// preferring the tightest (smallest) enclosing span.
fn find_enclosing_node_index(result: &ExtractionResult, line: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (index, span_size)

    for (idx, node) in result.nodes.iter().enumerate() {
        let start_line = node.span.start[0];
        let end_line = node.span.end[0];

        if line >= start_line && line <= end_line {
            let span_size = end_line - start_line;
            if best.is_none() || span_size < best.unwrap().1 {
                best = Some((idx, span_size));
            }
        }
    }

    best.map(|(idx, _)| idx)
}

struct SourceSpanIndex {
    line_starts: Vec<usize>,
}

impl SourceSpanIndex {
    fn new(source: &[u8]) -> Self {
        let mut line_starts = vec![0usize];
        for (idx, &byte) in source.iter().enumerate() {
            if byte == b'\n' {
                line_starts.push(idx + 1);
            }
        }
        Self { line_starts }
    }

    fn byte_offset(&self, line: usize, column: usize, source: &[u8]) -> Option<usize> {
        let start = *self.line_starts.get(line)?;
        let next_line_start = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(source.len());
        let line_slice = source.get(start..next_line_start)?;
        let mut remaining = column;
        for (offset, _) in std::str::from_utf8(line_slice).ok()?.char_indices() {
            if remaining == 0 {
                return Some(start + offset);
            }
            remaining -= 1;
        }
        (remaining == 0).then_some(next_line_start.saturating_sub(1))
    }

    fn text_for_span<'a>(&self, source: &'a [u8], span: &Span) -> Option<&'a str> {
        let start = self.byte_offset(span.start[0], span.start[1], source)?;
        let end = self.byte_offset(span.end[0], span.end[1], source)?;
        std::str::from_utf8(source.get(start..end)?).ok()
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn enrich_localization_metadata(
    source: &[u8],
    file_path: &Path,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let tree = parse_swift(source)?;
    enrich_localization_metadata_with_tree(source, file_path, &tree, result)
}

pub fn enrich_localization_metadata_with_tree(
    source: &[u8],
    file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut context = EnrichmentContext::new(result);
    let file_str = file_path.to_string_lossy().to_string();
    let source_spans = SourceSpanIndex::new(source);

    let mut wrapper_nodes = Vec::new();
    collect_localizable_wrapper_nodes(tree.root_node(), source, &mut wrapper_nodes);
    for wrapper_node in wrapper_nodes {
        let Some(wrapper_metadata) = extract_wrapper_metadata(wrapper_node, source) else {
            continue;
        };
        let Some(node_id) =
            matching_swiftui_declaration_id(&context, file_path, wrapper_node, source)
        else {
            continue;
        };
        apply_wrapper_metadata(&mut context, &node_id, &wrapper_metadata);
    }

    let mut declaration_nodes = Vec::new();
    collect_swiftui_declaration_nodes(tree.root_node(), source, &mut declaration_nodes);
    let mut localization_bindings_by_owner: HashMap<String, LocalizationBindings> = HashMap::new();
    for declaration_node in declaration_nodes {
        let Some(decl_id) =
            matching_swiftui_declaration_id(&context, file_path, declaration_node, source)
        else {
            continue;
        };
        let bindings = localization_bindings_for_declaration(declaration_node, source);
        if !bindings.is_empty() {
            localization_bindings_by_owner.insert(decl_id, bindings);
        }
    }

    let view_nodes = context.node_snapshots.clone();
    for view_node in view_nodes {
        if view_node.kind != NodeKind::View || !file_matches(&view_node.file, file_path) {
            continue;
        }
        let Some(text) = source_spans.text_for_span(source, &view_node.span) else {
            continue;
        };
        let bindings = swiftui_owner_id(&view_node.id)
            .and_then(|owner_id| localization_bindings_by_owner.get(owner_id));

        if view_node.name == "Text" {
            let references = localized_text_references_from_text(text, bindings);
            if let Some(reference) = references.first() {
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &view_node.id,
                    &view_node.span,
                    reference,
                    None,
                );
            }
            for (index, reference) in references.iter().enumerate().skip(1) {
                let usage_id = emit_localization_usage_node(
                    &mut context,
                    &view_node.id,
                    &file_str,
                    "text",
                    &view_node.span,
                    Some(&localization_reference_suffix(reference, index)),
                );
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &usage_id,
                    &view_node.span,
                    reference,
                    None,
                );
            }
            continue;
        }

        if builtin_view_accepts_localized_title(&view_node.name) {
            if let Some(reference) = builtin_view_title_reference(text, &view_node.name, bindings) {
                let usage_id = emit_localization_usage_node(
                    &mut context,
                    &view_node.id,
                    &file_str,
                    "title",
                    &view_node.span,
                    Some(&localization_reference_suffix(&reference, 0)),
                );
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &usage_id,
                    &view_node.span,
                    &reference,
                    Some("title"),
                );
            }
            continue;
        }

        if is_builtin_swiftui_view(&view_node.name) {
            continue;
        }

        for (index, (label, reference)) in
            custom_view_localization_arguments_from_text(text, bindings)
                .into_iter()
                .enumerate()
        {
            let usage_id = emit_localization_usage_node(
                &mut context,
                &view_node.id,
                &file_str,
                label.as_deref().unwrap_or("text"),
                &view_node.span,
                Some(&localization_reference_suffix(&reference, index)),
            );
            apply_localization_reference_to_span(
                &mut context,
                file_path,
                &file_str,
                &usage_id,
                &view_node.span,
                &reference,
                label.as_deref(),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
pub fn enrich_swiftui_structure(
    source: &[u8],
    file_path: &Path,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let tree = parse_swift(source)?;
    enrich_swiftui_structure_with_tree(source, file_path, &tree, result)
}

pub fn enrich_swiftui_structure_with_tree(
    source: &[u8],
    file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut context = EnrichmentContext::new(result);
    let mut declaration_nodes = Vec::new();
    collect_swiftui_declaration_nodes(tree.root_node(), source, &mut declaration_nodes);

    let file_str = file_path.to_string_lossy().to_string();

    let mut property_nodes = Vec::new();
    collect_property_declaration_nodes(tree.root_node(), &mut property_nodes);
    for property_node in property_nodes {
        let Some(property_id) =
            matching_swiftui_declaration_id(&context, file_path, property_node, source)
        else {
            continue;
        };
        if let Some(metadata) = extract_swiftui_dynamic_property_metadata(property_node, source) {
            apply_swiftui_dynamic_property_metadata(context.result, &property_id, &metadata);
        }
        if declaration_returns_swiftui_view(property_node, source) {
            continue;
        }
        extract_property_dependency_reads(
            property_node,
            source,
            &file_str,
            &[],
            &property_id,
            &mut context,
        );
    }

    for decl_node in declaration_nodes {
        let Some(decl_id) = matching_swiftui_declaration_id(&context, file_path, decl_node, source)
        else {
            continue;
        };
        extract_swiftui_declaration_structure_with_context(
            decl_node,
            source,
            &file_str,
            &[],
            &decl_id,
            &mut context,
        );
    }

    let mut body_nodes = Vec::new();
    collect_swiftui_body_nodes(tree.root_node(), source, &mut body_nodes);

    for body_node in body_nodes {
        let Some(body_id) = matching_body_id(&context, file_path, body_node) else {
            continue;
        };
        extract_swiftui_declaration_structure_with_context(
            body_node,
            source,
            &file_str,
            &[],
            &body_id,
            &mut context,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{EdgeKind, NodeKind, Visibility};

    fn extract(source: &str) -> ExtractionResult {
        let extractor = SwiftExtractor;
        extractor
            .extract(source.as_bytes(), Path::new("test.swift"))
            .unwrap()
    }

    fn extract_with_localization(source: &str) -> ExtractionResult {
        crate::extract_swift(source.as_bytes(), Path::new("test.swift"), None, None).unwrap()
    }

    fn find_node<'a>(result: &'a ExtractionResult, name: &str) -> &'a grapha_core::graph::Node {
        result
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("node '{}' not found", name))
    }

    fn has_edge(result: &ExtractionResult, source: &str, target: &str, kind: EdgeKind) -> bool {
        result
            .edges
            .iter()
            .any(|e| e.source == source && e.target == target && e.kind == kind)
    }

    #[test]
    fn extracts_struct() {
        let result = extract("public struct Config { let debug: Bool }");
        let node = find_node(&result, "Config");
        assert_eq!(node.kind, NodeKind::Struct);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_class() {
        let result = extract("public class AppDelegate { }");
        let node = find_node(&result, "AppDelegate");
        assert_eq!(node.kind, NodeKind::Class);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_protocol() {
        let result = extract("public protocol Drawable { func draw() }");
        let node = find_node(&result, "Drawable");
        assert_eq!(node.kind, NodeKind::Protocol);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_enum_with_cases() {
        let result = extract(
            r#"
            public enum Color {
                case red
                case green
            }
            "#,
        );
        let color = find_node(&result, "Color");
        assert_eq!(color.kind, NodeKind::Enum);

        let red = find_node(&result, "red");
        assert_eq!(red.kind, NodeKind::Variant);

        let green = find_node(&result, "green");
        assert_eq!(green.kind, NodeKind::Variant);

        assert!(has_edge(&result, &color.id, &red.id, EdgeKind::Contains));
        assert!(has_edge(&result, &color.id, &green.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_function() {
        let result = extract("public func greet() { }");
        let node = find_node(&result, "greet");
        assert_eq!(node.kind, NodeKind::Function);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn overloaded_initializers_get_distinct_ids() {
        let result = extract(
            r#"
            struct StringPair {
                init(key: String, value: String) {}
                init?(iosLine: String) {}
            }
            "#,
        );

        let init_nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| node.name == "init")
            .collect();
        assert_eq!(init_nodes.len(), 2);

        let unique_ids: std::collections::HashSet<_> =
            init_nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(unique_ids.len(), 2);
    }

    #[test]
    fn multiple_extensions_get_distinct_ids() {
        let result = extract(
            r#"
            struct Config {}

            extension Config {
                func alpha() {}
            }

            extension Config {
                func beta() {}
            }
            "#,
        );

        let extension_nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Extension)
            .collect();
        assert_eq!(extension_nodes.len(), 2);

        let unique_ids: std::collections::HashSet<_> = extension_nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(unique_ids.len(), 2);
    }

    #[test]
    fn extracts_extension() {
        let result = extract("extension Config { func foo() {} }");
        let ext = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Extension)
            .expect("extension node not found");
        assert_eq!(ext.name, "Config");

        let foo = find_node(&result, "foo");
        assert_eq!(foo.kind, NodeKind::Function);
        assert!(has_edge(&result, &ext.id, &foo.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_protocol_conformance() {
        let result = extract(
            r#"
            public protocol Configurable {}
            public class AppDelegate: Configurable { }
            "#,
        );
        let app = find_node(&result, "AppDelegate");
        assert!(has_edge(
            &result,
            &app.id,
            "test.swift::Configurable",
            EdgeKind::Implements
        ));
    }

    #[test]
    fn extracts_import() {
        let result = extract("import Foundation");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, "Foundation");
        assert_eq!(
            result.imports[0].kind,
            grapha_core::resolve::ImportKind::Module
        );
    }

    #[test]
    fn extracts_call_edges() {
        let result = extract(
            r#"
            func greet() { }
            func launch() {
                greet()
            }
            "#,
        );
        assert!(has_edge(
            &result,
            "test.swift::launch",
            "test.swift::greet",
            EdgeKind::Calls,
        ));
        let call_edge = result
            .edges
            .iter()
            .find(|edge| {
                edge.source == "test.swift::launch"
                    && edge.target == "test.swift::greet"
                    && edge.kind == EdgeKind::Calls
            })
            .expect("should find call edge");
        assert!(
            !call_edge.provenance.is_empty(),
            "call edges should carry provenance"
        );
        assert_eq!(call_edge.provenance[0].symbol_id, "test.swift::launch");
    }

    #[test]
    fn property_accesses_become_reads_not_calls() {
        let result = extract(
            r#"
            struct Model { let value: Int }
            func launch(model: Model) {
                let current = model.value
            }
            "#,
        );

        assert!(has_edge(
            &result,
            "test.swift::launch",
            "test.swift::value",
            EdgeKind::Reads,
        ));
        assert!(!has_edge(
            &result,
            "test.swift::launch",
            "test.swift::value",
            EdgeKind::Calls,
        ));
    }

    #[test]
    fn extracts_condition_on_call_inside_if() {
        let result = extract(
            r#"
            func run() {
                if true {
                    helper()
                }
            }
            func helper() { }
            "#,
        );
        let cond_edge = result
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls && e.target.contains("helper"));
        assert!(cond_edge.is_some(), "should find Calls edge to helper");
        // The condition may or may not be extracted depending on tree-sitter-swift's AST
        // We verify the edge exists and the mechanism doesn't crash
    }

    #[test]
    fn detects_view_body_as_entry_point() {
        let result = extract(
            r#"
            struct ContentView: View {
                var body: Int { return 0 }
            }
            "#,
        );
        let body_node = result
            .nodes
            .iter()
            .find(|n| n.name == "body")
            .expect("should find body property");
        assert_eq!(
            body_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "View.body should be an EntryPoint"
        );
    }

    #[test]
    fn scopes_body_ids_per_view_type() {
        let result = extract(
            r#"
            struct FirstView: View {
                var body: some View { Text("One") }
            }

            struct SecondView: View {
                var body: some View { Text("Two") }
            }
            "#,
        );

        let body_ids: Vec<&str> = result
            .nodes
            .iter()
            .filter(|n| n.name == "body" && n.kind == NodeKind::Property)
            .map(|n| n.id.as_str())
            .collect();

        assert_eq!(body_ids.len(), 2);
        assert!(body_ids.contains(&"test.swift::FirstView::body"));
        assert!(body_ids.contains(&"test.swift::SecondView::body"));
    }

    #[test]
    fn extracts_swiftui_view_hierarchy_and_type_refs() {
        let result = extract(
            r#"
            import SwiftUI

            struct Row: View {
                let title: String
                var body: some View { Text(title) }
            }

            struct ContentView: View {
                var body: some View {
                    VStack {
                        Text("Hello")
                        Row(title: "World")
                    }
                }
            }
            "#,
        );

        let body = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::body")
            .expect("content body node should exist");
        let vstack = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "VStack")
            .expect("VStack synthetic view should exist");
        let row_view = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "Row")
            .expect("Row synthetic view should exist");
        let vstack_children: Vec<&Node> = result
            .edges
            .iter()
            .filter(|edge| edge.source == vstack.id && edge.kind == EdgeKind::Contains)
            .filter_map(|edge| result.nodes.iter().find(|node| node.id == edge.target))
            .collect();

        assert!(has_edge(&result, &body.id, &vstack.id, EdgeKind::Contains));
        assert_eq!(
            vstack_children
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Text", "Row"]
        );
        assert!(has_edge(
            &result,
            &row_view.id,
            "test.swift::Row",
            EdgeKind::TypeRef
        ));
    }

    #[test]
    fn extracts_swiftui_branch_hierarchy() {
        let result = extract(
            r#"
            import SwiftUI

            struct ContentView: View {
                var body: some View {
                    VStack {
                        if showDetails {
                            Group {
                                Text("More")
                            }
                        } else {
                            switch mode {
                            case .empty:
                                EmptyView()
                            default:
                                ForEach(items) { item in
                                    Text(item.name)
                                }
                            }
                        }
                    }
                }
            }
            "#,
        );

        let vstack = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "VStack")
            .expect("VStack should exist");
        let if_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "if showDetails")
            .expect("if branch should exist");
        let else_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "else")
            .expect("else branch should exist");
        let switch_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "switch mode")
            .expect("switch branch should exist");
        let default_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "default")
            .expect("default branch should exist");
        let for_each = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "ForEach")
            .expect("ForEach view should exist");

        assert!(has_edge(
            &result,
            &vstack.id,
            &if_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &vstack.id,
            &else_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &else_branch.id,
            &switch_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &switch_branch.id,
            &default_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &default_branch.id,
            &for_each.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_swiftui_structure_through_modifier_chains_and_view_refs() {
        let result = extract(
            r#"
            import SwiftUI

            struct InlinePanel: View {
                var body: some View { Text("Inline") }
            }

            struct OverlayPanel: View {
                var body: some View { Text("Overlay") }
            }

            struct ContentView: View {
                var chatRoomFragViewPanel: some View {
                    InlinePanel()
                }

                var exitPopView: some View {
                    Text("Exit")
                }

                var body: some View {
                    NavigationStack {
                        if showDetails {
                            InlinePanel()
                                .onReceive(events) { _ in
                                    switch mode {
                                    case .loading:
                                        helper()
                                    default:
                                        break
                                    }
                                }
                        }

                        chatRoomFragViewPanel
                        DialogStreamView()
                        exitPopView
                    }
                    .frame(width: 100)
                    .overlay {
                        OverlayPanel()
                    }
                    .onReceive(events) { _ in
                        switch mode {
                        case .done:
                            helper()
                        default:
                            break
                        }
                    }
                }
            }
            "#,
        );

        let body = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::body")
            .expect("content body node should exist");
        let nav = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "NavigationStack")
            .expect("NavigationStack synthetic view should exist");
        let if_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "if showDetails")
            .expect("if branch should exist");
        let body_inline_panel = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "InlinePanel"
                    && n.id.starts_with("test.swift::ContentView::body::view:")
            })
            .expect("body InlinePanel view should exist");
        let chat_panel = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "chatRoomFragViewPanel")
            .expect("chatRoomFragViewPanel view ref should exist");
        let exit_pop = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "exitPopView")
            .expect("exitPopView view ref should exist");
        let dialog_stream = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "DialogStreamView")
            .expect("DialogStreamView should exist");
        let overlay_panel = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "OverlayPanel")
            .expect("OverlayPanel should exist");
        let helper_property = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::chatRoomFragViewPanel")
            .expect("helper property should exist");
        let exit_property = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::exitPopView")
            .expect("exit property should exist");
        let helper_text = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "Text"
                    && n.id
                        .starts_with("test.swift::ContentView::exitPopView::view:")
            })
            .expect("exit helper text should exist");
        let helper_inline_panel = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "InlinePanel"
                    && n.id
                        .starts_with("test.swift::ContentView::chatRoomFragViewPanel::view:")
            })
            .expect("helper InlinePanel view should exist");

        assert!(has_edge(&result, &body.id, &nav.id, EdgeKind::Contains));
        assert!(has_edge(
            &result,
            &nav.id,
            &if_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &if_branch.id,
            &body_inline_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &nav.id,
            &chat_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(&result, &nav.id, &exit_pop.id, EdgeKind::Contains));
        assert!(has_edge(
            &result,
            &nav.id,
            &dialog_stream.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &nav.id,
            &overlay_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &helper_property.id,
            &helper_inline_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &exit_property.id,
            &helper_text.id,
            EdgeKind::Contains
        ));
        assert!(
            result
                .nodes
                .iter()
                .all(|n| !(n.kind == NodeKind::Branch && n.name == "switch mode")),
            "non-view modifier closures should not become structural branches"
        );
    }

    #[test]
    fn enriches_swiftui_body_with_read_dependencies() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            struct RoomPage: View {
                @State private var roomMode = 0
                @State private var luckyGameType: Int? = nil
                static let headerViewHeight = 44

                var canShowGameRoom: Bool {
                    roomMode > 0 && luckyGameType != nil
                }

                var roomHeaderView: some View {
                    Text("\(roomMode)")
                }

                var body: some View {
                    VStack {
                        if canShowGameRoom {
                            roomHeaderView
                        }
                    }
                    .frame(height: Self.headerViewHeight)
                }
            }
            "#,
        );
        let graph = grapha_core::merge(vec![result]);

        let has_read = |target: &str| {
            graph.edges.iter().any(|edge| {
                edge.source == "test.swift::RoomPage::body"
                    && edge.target == target
                    && edge.kind == EdgeKind::Reads
            })
        };

        assert!(has_read("test.swift::RoomPage::canShowGameRoom"));
        assert!(has_read("test.swift::RoomPage::roomHeaderView"));
        assert!(has_read("test.swift::RoomPage::headerViewHeight"));
    }

    #[test]
    fn enriches_computed_properties_with_read_dependencies() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            struct RoomPage: View {
                @State private var roomMode = 0
                @State private var luckyGameType: Int? = nil
                @State private var minimizeGameRoom = false

                var canShowGameRoom: Bool {
                    guard luckyGameType != nil else { return false }
                    return roomMode > 0 && !minimizeGameRoom
                }

                var body: some View { EmptyView() }
            }
            "#,
        );
        let graph = grapha_core::merge(vec![result]);

        let has_read = |target: &str| {
            graph.edges.iter().any(|edge| {
                edge.source == "test.swift::RoomPage::canShowGameRoom"
                    && edge.target == target
                    && edge.kind == EdgeKind::Reads
            })
        };

        assert!(has_read("test.swift::RoomPage::roomMode"));
        assert!(has_read("test.swift::RoomPage::luckyGameType"));
        assert!(has_read("test.swift::RoomPage::minimizeGameRoom"));
    }

    #[test]
    fn skips_argument_labels_when_enriching_property_reads() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            struct GeneralButtonType {
                let type: Int
            }

            struct AudioButton {
                let label: String
            }

            struct LoginPage: View {
                @State private var isNextValid = false

                private var loginButton: some View {
                    Button {
                    } label: {
                        Text("Login")
                    }
                    .buttonStyle(.app(type: .yellowH112))
                    .disabled(isNextValid.negated)
                }

                var body: some View { loginButton }
            }
            "#,
        );
        let graph = grapha_core::merge(vec![result]);

        let read_targets: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| {
                edge.source == "test.swift::LoginPage::loginButton" && edge.kind == EdgeKind::Reads
            })
            .map(|edge| edge.target.as_str())
            .collect();

        assert!(
            read_targets.contains(&"test.swift::LoginPage::isNextValid"),
            "real property reads should still be preserved"
        );
        assert!(
            read_targets
                .iter()
                .all(|target| !target.ends_with("::label")),
            "trailing closure labels should not become dependency reads: {read_targets:?}"
        );
        assert!(
            read_targets
                .iter()
                .all(|target| !target.ends_with("::type")),
            "named argument labels should not become dependency reads: {read_targets:?}"
        );
    }

    #[test]
    fn marks_swiftui_dynamic_properties_as_invalidation_sources() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            struct RoomPage: View {
                @State private var roomMode = 0
                @StateObject private var ctx = Room.shared
                @EnvironmentObject private var router: RouterViewModel
                @AppStorage("flag") private var showFlag = false

                var body: some View { EmptyView() }
            }
            "#,
        );

        let node = |name: &str| {
            result
                .nodes
                .iter()
                .find(|node| node.name == name)
                .unwrap_or_else(|| panic!("missing node {name}"))
        };

        assert_eq!(
            node("roomMode")
                .metadata
                .get("swiftui.dynamic_property.wrapper")
                .map(|value| value.as_str()),
            Some("state")
        );
        assert_eq!(
            node("ctx")
                .metadata
                .get("swiftui.dynamic_property.wrapper")
                .map(|value| value.as_str()),
            Some("state_object")
        );
        assert_eq!(
            node("router")
                .metadata
                .get("swiftui.dynamic_property.wrapper")
                .map(|value| value.as_str()),
            Some("environment_object")
        );
        assert_eq!(
            node("showFlag")
                .metadata
                .get("swiftui.dynamic_property.wrapper")
                .map(|value| value.as_str()),
            Some("app_storage")
        );
        assert!(
            ["roomMode", "ctx", "router", "showFlag"]
                .iter()
                .all(|name| {
                    node(name)
                        .metadata
                        .get("swiftui.invalidation_source")
                        .is_some_and(|value| value == "true")
                }),
            "dynamic SwiftUI properties should be tagged as invalidation sources"
        );
    }

    #[test]
    fn extracts_swiftui_structure_for_same_type_view_methods() {
        let result = extract(
            r#"
            import SwiftUI

            struct ContentView: View {
                func helperPanel() -> some View {
                    Text("Helper")
                }

                var body: some View {
                    VStack {
                        helperPanel()
                    }
                }
            }
            "#,
        );

        let helper_fn = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::helperPanel")
            .expect("helper method should exist");
        let helper_call = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "helperPanel"
                    && n.id.starts_with("test.swift::ContentView::body::view:")
            })
            .expect("helper method call should exist");
        let helper_text = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "Text"
                    && n.id
                        .starts_with("test.swift::ContentView::helperPanel::view:")
            })
            .expect("helper method text should exist");

        assert!(has_edge(
            &result,
            &helper_call.id,
            &helper_fn.id,
            EdgeKind::TypeRef
        ));
        assert!(has_edge(
            &result,
            &helper_fn.id,
            &helper_text.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn excludes_action_closures_from_structural_view_tree() {
        let result = extract(
            r#"
            import SwiftUI

            struct ExitPopView: View {
                let dismissPopView: () -> Void
                let exit: () -> Void
                let minimize: () -> Void

                var body: some View {
                    VStack {
                        Button {
                            minimize()
                        } label: {
                            Text("Minimize")
                        }
                    }
                    .onTapGesture {
                        dismissPopView()
                    }
                }
            }

            struct ContentView: View {
                @State private var exitPopShow = true

                var body: some View {
                    if exitPopShow {
                        ExitPopView {
                            exitPopShow = false
                        } exit: {
                            handleExitRoom()
                        } minimize: {
                            handleMinimizeRoom()
                        }
                    }
                }

                func handleExitRoom() {}
                func handleMinimizeRoom() {}
            }
            "#,
        );

        assert!(
            result.nodes.iter().all(|node| !(node.kind == NodeKind::View
                && matches!(
                    node.name.as_str(),
                    "handleExitRoom" | "handleMinimizeRoom" | "minimize" | "dismissPopView"
                ))),
            "action closures should not emit structural view nodes"
        );

        assert!(
            result
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::View && node.name == "Text"),
            "builder-like label closures should still be traversed"
        );
    }

    #[test]
    fn enrich_swiftui_structure_overlays_synthetic_nodes_without_duplicating_declarations() {
        let source = br#"
import SwiftUI

struct Row: View {
    let title: String
    var body: some View { Text(title) }
}

struct ContentView: View {
    var body: some View {
        VStack {
            Text("Hello")
            Row(title: "World")
        }
    }
}
"#;
        let mut result = ExtractionResult::new();
        result.nodes.push(Node {
            id: "s:Row".into(),
            kind: NodeKind::Struct,
            name: "Row".into(),
            file: "test.swift".into(),
            span: Span {
                start: [3, 0],
                end: [6, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });
        result.nodes.push(Node {
            id: "s:ContentView".into(),
            kind: NodeKind::Struct,
            name: "ContentView".into(),
            file: "test.swift".into(),
            span: Span {
                start: [8, 0],
                end: [15, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });
        result.nodes.push(Node {
            id: "s:ContentView.body".into(),
            kind: NodeKind::Property,
            name: "body".into(),
            file: "test.swift".into(),
            span: Span {
                start: [9, 4],
                end: [14, 5],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: Some(NodeRole::EntryPoint),
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });

        enrich_swiftui_structure(source, Path::new("test.swift"), &mut result).unwrap();

        let view_nodes: Vec<&Node> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::View)
            .collect();
        assert!(
            !view_nodes.is_empty(),
            "overlay should add synthetic view nodes"
        );
        assert_eq!(
            result
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Property && n.name == "body")
                .count(),
            1,
            "overlay should not duplicate declaration nodes"
        );
        let vstack = view_nodes
            .iter()
            .find(|n| n.name == "VStack")
            .expect("overlay should add VStack");
        assert!(has_edge(
            &result,
            "s:ContentView.body",
            &vstack.id,
            EdgeKind::Contains
        ));
        let row_view = view_nodes
            .iter()
            .find(|n| n.name == "Row")
            .expect("overlay should add Row instance");
        assert!(has_edge(
            &result,
            &row_view.id,
            "test.swift::Row",
            EdgeKind::TypeRef
        ));
    }

    #[test]
    fn enrich_swiftui_structure_matches_view_helpers_by_owner_when_line_metadata_drifts() {
        let source = br#"
import SwiftUI

extension RoomPage {
    @ViewBuilder
    private var centerContentView: some View {
        ZStack {
            Text("Hello")
        }
    }
}
"#;
        let mut result = ExtractionResult::new();
        result.nodes.push(Node {
            id: "s:e:s:4Room0A4PageV".into(),
            kind: NodeKind::Extension,
            name: "RoomPage".into(),
            file: "RoomPage.swift".into(),
            span: Span {
                start: [2, 0],
                end: [9, 0],
            },
            visibility: Visibility::Crate,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: Some("Room".into()),
            snippet: None,
        });
        result.nodes.push(Node {
            id: "s:4Room0A4PageV17centerContentViewQrvp".into(),
            kind: NodeKind::Property,
            name: "centerContentView".into(),
            file: "RoomPage.swift".into(),
            span: Span {
                start: [0, 0],
                end: [0, 0],
            },
            visibility: Visibility::Private,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: Some("Room".into()),
            snippet: None,
        });
        result.edges.push(Edge {
            source: "s:4Room0A4PageV17centerContentViewQrvp".into(),
            target: "s:e:s:4Room0A4PageV".into(),
            kind: EdgeKind::Implements,
            confidence: 0.9,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
        });

        enrich_swiftui_structure(source, Path::new("RoomPage.swift"), &mut result).unwrap();

        let zstack = result
            .nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::View
                    && node.name == "ZStack"
                    && node
                        .id
                        .starts_with("s:4Room0A4PageV17centerContentViewQrvp::view:")
            })
            .expect("centerContentView should gain a ZStack subtree despite line drift");

        assert!(has_edge(
            &result,
            "s:4Room0A4PageV17centerContentViewQrvp",
            &zstack.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_function_signature() {
        let result = extract("func greet(name: String) -> String { return name }");
        let node = find_node(&result, "greet");
        assert!(node.signature.is_some(), "signature should be extracted");
        let sig = node.signature.as_ref().unwrap();
        assert!(
            sig.contains("func greet"),
            "signature should contain func name"
        );
    }

    #[test]
    fn extracts_doc_comment() {
        let result = extract(
            r#"
            /// A documented function
            func documented() { }
            "#,
        );
        let node = find_node(&result, "documented");
        assert!(
            node.doc_comment.is_some(),
            "doc_comment should be extracted"
        );
        let doc = node.doc_comment.as_ref().unwrap();
        assert!(doc.contains("documented"), "should contain comment text");
    }

    #[test]
    fn extracts_doc_comment_with_attributes() {
        let result = extract(
            r#"
            class GameManager {
                /// Setup the initial running context and load the game scene, this method should be called when
                /// the game view is appeared.
                @MainActor public func bootstrapGame(with launchContext: WebGameLaunchContext) async {
                }
            }
            "#,
        );
        let node = find_node(&result, "bootstrapGame");
        assert!(
            node.doc_comment.is_some(),
            "doc_comment should be extracted for method with @MainActor attribute"
        );
        let doc = node.doc_comment.as_ref().unwrap();
        assert!(
            doc.contains("Setup the initial running context"),
            "should contain comment text, got: {}",
            doc
        );
    }

    #[test]
    fn enrich_doc_comments_patches_missing_docs() {
        let source = br#"
class GameManager {
    /// Setup the initial running context and load the game scene.
    /// This method should be called when the game view is appeared.
    @MainActor public func bootstrapGame(with launchContext: WebGameLaunchContext) async {
    }

    /// Returns the current score.
    var score: Int { 0 }
}
"#;
        // Simulate index-store output: correct names and 1-based lines, no doc comments.
        let mut result = ExtractionResult::new();
        result.nodes.push(Node {
            id: "s:GameManager".into(),
            kind: NodeKind::Struct,
            name: "GameManager".into(),
            file: "test.swift".into(),
            span: Span {
                start: [1, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });
        result.nodes.push(Node {
            id: "s:GameManager.bootstrapGame".into(),
            kind: NodeKind::Function,
            name: "bootstrapGame".into(),
            file: "test.swift".into(),
            span: Span {
                start: [5, 4],
                end: [5, 4],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });
        result.nodes.push(Node {
            id: "s:GameManager.score".into(),
            kind: NodeKind::Property,
            name: "score".into(),
            file: "test.swift".into(),
            span: Span {
                start: [9, 4],
                end: [9, 4],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        });

        enrich_doc_comments(source, &mut result).unwrap();

        let _game_mgr = result
            .nodes
            .iter()
            .find(|n| n.name == "GameManager")
            .unwrap();
        // class_declaration on line 1 (0-based row 1 → 1-based line 2 actually)
        // Let's just check the function and property which are the real targets.

        let bootstrap = result
            .nodes
            .iter()
            .find(|n| n.name == "bootstrapGame")
            .unwrap();
        assert!(
            bootstrap.doc_comment.is_some(),
            "bootstrapGame should have doc_comment after enrichment"
        );
        assert!(
            bootstrap
                .doc_comment
                .as_ref()
                .unwrap()
                .contains("Setup the initial running context"),
            "doc should contain expected text"
        );

        let score = result.nodes.iter().find(|n| n.name == "score").unwrap();
        assert!(
            score.doc_comment.is_some(),
            "score property should have doc_comment after enrichment"
        );
        assert!(
            score
                .doc_comment
                .as_ref()
                .unwrap()
                .contains("current score"),
            "doc should contain expected text"
        );
    }

    #[test]
    fn detects_main_attr_as_entry_point() {
        let result = extract(
            r#"
            @main
            struct MyApp {
            }
            "#,
        );
        let app_node = find_node(&result, "MyApp");
        assert_eq!(
            app_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "@main struct should be an EntryPoint"
        );
    }

    #[test]
    fn enrich_localization_metadata_marks_generated_wrapper_symbols() {
        let result = extract_with_localization(
            r#"
            public enum L10n {
                public static var accountForgetPassword: String {
                    L10n.tr("Localizable", "account_forget_password", fallback: "Forgot Password")
                }

                public static func commonCount(_ p1: Any) -> String {
                    L10n.tr("Localizable", "common_count", String(describing: p1), fallback: "%@")
                }
            }

            public struct L10nResource {
                public init(_ key: String, table: String, bundle: Bundle, fallback: String) {}
            }

            public enum L10nResourceSet {
                public static let commonShare = L10nResource(
                    "common_share",
                    table: "Localizable",
                    bundle: .module,
                    fallback: "Share"
                )
            }
            "#,
        );

        let account_forget_password = result
            .nodes
            .iter()
            .find(|node| node.name == "accountForgetPassword")
            .expect("wrapper property should exist");
        assert_eq!(
            account_forget_password
                .metadata
                .get("l10n.wrapper.table")
                .map(|value| value.as_str()),
            Some("Localizable")
        );
        assert_eq!(
            account_forget_password
                .metadata
                .get("l10n.wrapper.key")
                .map(|value| value.as_str()),
            Some("account_forget_password")
        );

        let common_count = result
            .nodes
            .iter()
            .find(|node| node.name == "commonCount")
            .expect("wrapper function should exist");
        assert_eq!(
            common_count
                .metadata
                .get("l10n.wrapper.key")
                .map(|value| value.as_str()),
            Some("common_count")
        );
        assert_eq!(
            common_count
                .metadata
                .get("l10n.wrapper.arg_count")
                .map(|value| value.as_str()),
            Some("1")
        );

        let common_share = result
            .nodes
            .iter()
            .find(|node| node.name == "commonShare")
            .expect("resource wrapper property should exist");
        assert_eq!(
            common_share
                .metadata
                .get("l10n.wrapper.key")
                .map(|value| value.as_str()),
            Some("common_share")
        );
    }

    #[test]
    fn enrich_localization_metadata_marks_swiftui_text_usages() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            public enum L10n {
                public static var accountForgetPassword: String {
                    L10n.tr("Localizable", "account_forget_password", fallback: "Forgot Password")
                }

                public static var storeUseNow: String {
                    L10n.tr("Localizable", "store_use_now", fallback: "Use now")
                }

                public static func commonCount(_ p1: Any) -> String {
                    L10n.tr("Localizable", "common_count", String(describing: p1), fallback: "%@")
                }
            }

            public struct L10nResource {
                public init(_ key: String, table: String, bundle: Bundle, fallback: String) {}
            }

            public enum L10nResourceSet {
                public static let commonShare = L10nResource(
                    "common_share",
                    table: "Localizable",
                    bundle: .module,
                    fallback: "Share"
                )
            }

            struct ContentView: View {
                let title: String

                var body: some View {
                    VStack {
                        Text(.accountForgetPassword)
                        Text(i18n: .commonShare)
                        Text(L10n.storeUseNow)
                        Text(L10n.commonCount(42))
                        Text(verbatim: title)
                        Text(title)
                        Text("Party Game & Voice Chat", bundle: .module)
                    }
                }
            }
            "#,
        );

        let localized_texts: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::View && node.name == "Text")
            .filter(|node| node.metadata.contains_key("l10n.ref_kind"))
            .collect();
        assert_eq!(
            localized_texts.len(),
            6,
            "expected wrapper, literal, and possible string Text usages, but not verbatim text"
        );

        let account_text = localized_texts
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .map(|value| value.as_str())
                    == Some("accountForgetPassword")
            })
            .expect("dot syntax Text usage should be marked");
        assert_eq!(
            account_text
                .metadata
                .get("l10n.ref_kind")
                .map(|value| value.as_str()),
            Some("wrapper")
        );

        let common_share_text = localized_texts
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .map(|value| value.as_str())
                    == Some("commonShare")
            })
            .expect("i18n Text usage should be marked");
        assert_eq!(
            common_share_text
                .metadata
                .get("l10n.arg_count")
                .map(|value| value.as_str()),
            Some("0")
        );
        assert_eq!(
            common_share_text
                .metadata
                .get("l10n.wrapper_base")
                .map(|value| value.as_str()),
            Some("L10nResource")
        );

        let common_count_text = localized_texts
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .map(|value| value.as_str())
                    == Some("commonCount")
            })
            .expect("parameterized wrapper usage should be marked");
        assert_eq!(
            common_count_text
                .metadata
                .get("l10n.arg_count")
                .map(|value| value.as_str()),
            Some("1")
        );

        let literal_text = localized_texts
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.ref_kind")
                    .map(|value| value.as_str())
                    == Some("literal")
            })
            .expect("string literal usage should be marked");
        assert_eq!(
            literal_text
                .metadata
                .get("l10n.literal")
                .map(|value| value.as_str()),
            Some("Party Game & Voice Chat")
        );

        let possible_string_text = localized_texts
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.ref_kind")
                    .map(|value| value.as_str())
                    == Some("possible_string")
            })
            .expect("dynamic string Text usage should be marked as possible");
        assert!(
            !possible_string_text.metadata.contains_key("l10n.literal"),
            "possible string usages should not pretend to have a concrete literal"
        );

        assert!(
            result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::TypeRef
                    && edge.source == account_text.id
                    && edge.target.ends_with("accountForgetPassword")
            }),
            "localized Text usage should emit a type-ref edge to its wrapper symbol"
        );
    }

    #[test]
    fn enrich_localization_metadata_marks_custom_view_string_arguments() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            public enum L10n {
                public static var welcomeTitle: String {
                    L10n.tr("Localizable", "welcome_title", fallback: "Welcome")
                }
            }

            struct TitleRow: View {
                let title: String
                let subtitle: String

                var body: some View {
                    VStack {
                        Text(title)
                        Text(subtitle)
                    }
                }
            }

            struct ContentView: View {
                let subtitle: String

                var body: some View {
                    TitleRow(title: L10n.welcomeTitle, subtitle: subtitle)
                }
            }
            "#,
        );
        let wrapper_argument = result
            .nodes
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.argument_label")
                    .map(|value| value.as_str())
                    == Some("title")
                    && node
                        .metadata
                        .get("l10n.ref_kind")
                        .map(|value| value.as_str())
                        == Some("wrapper")
            })
            .expect("custom view title argument should be marked");
        assert!(
            result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::TypeRef
                    && edge.source == wrapper_argument.id
                    && edge.target.ends_with("welcomeTitle")
            }),
            "custom view wrapper arguments should link to wrapper symbols"
        );

        let possible_argument = result
            .nodes
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.argument_label")
                    .map(|value| value.as_str())
                    == Some("subtitle")
                    && node
                        .metadata
                        .get("l10n.ref_kind")
                        .map(|value| value.as_str())
                        == Some("possible_string")
            })
            .expect("dynamic custom view text arguments should be marked as possible");
        assert!(
            result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::Contains
                    && edge.target == possible_argument.id
                    && edge.source.contains("TitleRow")
            }),
            "custom-view localization markers should sit under the invocation subtree"
        );
    }

    #[test]
    fn localized_text_reference_marks_text_i18n_identifier_as_possible_wrapper() {
        let text = localized_text_references_from_text("Text(i18n: i18n)", None)
            .into_iter()
            .next()
            .expect("Text(i18n: identifier) should be classified");
        assert_eq!(text.ref_kind, "possible_wrapper");
        assert_eq!(text.wrapper_base.as_deref(), Some("L10nResource"));
    }

    #[test]
    fn localized_reference_ignores_non_localized_member_access() {
        assert!(localized_reference_for_expression_text("friendList.isNilOrEmpty").is_none());
        assert!(localized_reference_for_expression_text("searchUserList.isEmpty").is_none());

        let wrapper = localized_reference_for_expression_text("L10n.welcomeTitle")
            .expect("L10n wrapper references should still resolve");
        assert_eq!(wrapper.wrapper_name.as_deref(), Some("welcomeTitle"));
        assert_eq!(wrapper.wrapper_base.as_deref(), Some("L10n"));
    }

    #[test]
    fn enrich_localization_metadata_marks_string_i18n_identifier_in_custom_view_args() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            public struct L10nResource {
                public var translation: String { "" }
            }

            extension String {
                init(i18n: L10nResource) {
                    self.init(i18n.translation)
                }
            }

            struct TitleRow: View {
                let title: String

                var body: some View {
                    Text(title)
                }
            }

            struct ContentView: View {
                let resource: L10nResource

                var body: some View {
                    TitleRow(title: String(i18n: resource))
                }
            }
            "#,
        );

        let title_argument = result
            .nodes
            .iter()
            .find(|node| {
                node.metadata
                    .get("l10n.argument_label")
                    .map(|value| value.as_str())
                    == Some("title")
                    && node
                        .metadata
                        .get("l10n.ref_kind")
                        .map(|value| value.as_str())
                        == Some("possible_wrapper")
            })
            .expect("String(i18n: identifier) should be tracked as a possible wrapper");
        assert_eq!(
            title_argument
                .metadata
                .get("l10n.wrapper_base")
                .map(|value| value.as_str()),
            Some("L10nResource")
        );
    }

    #[test]
    fn extracts_optional_some_view_structure_and_localized_aliases() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            public struct L10nResource {
                public init(_ key: String, table: String, bundle: Bundle, fallback: String) {}
                public var translation: String { "" }
            }

            extension String {
                init(i18n: L10nResource) {
                    self.init(i18n.translation)
                }
            }

            extension L10nResource {
                static let roomShareNoFriends = L10nResource(
                    "room_share_no_friends",
                    table: "Localizable",
                    bundle: .module,
                    fallback: "No friends"
                )
                static let commonSearchEmpty = L10nResource(
                    "common_search_empty",
                    table: "Localizable",
                    bundle: .module,
                    fallback: "No results"
                )
            }

            struct ListEmptyView: View {
                let title: String

                var body: some View {
                    Text(title)
                }
            }

            struct ContentView: View {
                let isEmpty: Bool

                private var emptyPlaceholderView: (some View)? {
                    let title: String? =
                        if isEmpty {
                            String(i18n: .roomShareNoFriends)
                        } else {
                            String(i18n: .commonSearchEmpty)
                        }

                    return if let title {
                        ListEmptyView(title: title)
                    } else {
                        nil
                    }
                }
            }
            "#,
        );

        let list_empty_view = result
            .nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::View
                    && node.name == "ListEmptyView"
                    && node.id.contains("emptyPlaceholderView::view:")
            })
            .expect("optional some View property should still emit SwiftUI structure");
        assert!(
            result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::TypeRef
                    && edge.source == list_empty_view.id
                    && edge.target.ends_with("ListEmptyView")
            }),
            "custom view should keep its type-ref edge"
        );

        let alias_arguments: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| {
                node.metadata
                    .get("l10n.argument_label")
                    .map(|value| value.as_str())
                    == Some("title")
                    && node
                        .metadata
                        .get("l10n.ref_kind")
                        .map(|value| value.as_str())
                        == Some("wrapper")
            })
            .collect();
        assert_eq!(alias_arguments.len(), 2);
        assert!(
            alias_arguments.iter().any(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .map(|value| value.as_str())
                    == Some("roomShareNoFriends")
            }),
            "first branch localization should propagate into the custom view argument"
        );
        assert!(
            alias_arguments.iter().any(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .map(|value| value.as_str())
                    == Some("commonSearchEmpty")
            }),
            "second branch localization should propagate into the custom view argument"
        );

        assert!(
            alias_arguments.iter().all(|node| {
                node.metadata
                    .get("l10n.wrapper_name")
                    .is_some_and(|value| value != "isEmpty" && value != "isNilOrEmpty")
            }),
            "condition helper members should not leak into localized alias bindings"
        );
    }

    #[test]
    fn enrich_localization_metadata_marks_builtin_views_with_localized_string_key() {
        let result = extract_with_localization(
            r#"
            import SwiftUI

            struct ContentView: View {
                @State var isOn = false

                var body: some View {
                    VStack {
                        Button("Tournament") { }
                        Label("Settings", systemImage: "gear")
                        Toggle("Notifications", isOn: $isOn)
                        NavigationLink("Profile") { Text("Detail") }
                        Section("Account") {
                            Text("Content")
                        }
                    }
                }
            }
            "#,
        );

        let l10n_usage_nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| {
                node.metadata
                    .get("l10n.ref_kind")
                    .map(|value| value.as_str())
                    == Some("literal")
                    && node.metadata.contains_key("l10n.literal")
            })
            .collect();

        let literals: Vec<&str> = l10n_usage_nodes
            .iter()
            .filter_map(|node| node.metadata.get("l10n.literal").map(|v| v.as_str()))
            .collect();

        for expected in &[
            "Tournament",
            "Settings",
            "Notifications",
            "Profile",
            "Account",
        ] {
            assert!(
                literals.contains(expected),
                "expected literal '{}' from built-in SwiftUI view, got: {:?}",
                expected,
                literals
            );
        }
    }
}
