use std::collections::BTreeMap;
use std::path::Path;

use grapha_core::graph::EdgeKind;
use grapha_core::{ExtractionResult, LanguageExtractor};
use grapha_swift::{SwiftExtractor, extract_swift, extract_swift_via_fallback_for_tests};

fn fixture() -> &'static [u8] {
    include_bytes!("fixtures/semantic_parity.swift")
}

fn has_edge(
    result: &grapha_core::ExtractionResult,
    source: &str,
    target: &str,
    kind: EdgeKind,
) -> bool {
    result
        .edges
        .iter()
        .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
}

fn extract_treesitter_fallback_direct(source: &[u8], path: &Path) -> ExtractionResult {
    let extractor = SwiftExtractor;
    extractor.extract(source, path).unwrap()
}

fn semantic_summary(result: &ExtractionResult) -> (Vec<String>, Vec<String>, Vec<String>) {
    let imports = result
        .imports
        .iter()
        .map(|import| format!("{}::{:?}", import.path, import.kind))
        .collect::<Vec<_>>();

    let nodes = result
        .nodes
        .iter()
        .filter_map(|node| {
            let metadata = node
                .metadata
                .iter()
                .filter(|(key, _)| {
                    matches!(
                        key.as_str(),
                        "swiftui.dynamic_property.wrapper"
                            | "swiftui.invalidation_source"
                            | "l10n.wrapper.table"
                            | "l10n.wrapper.key"
                            | "l10n.wrapper.fallback"
                            | "l10n.wrapper.arg_count"
                            | "l10n.ref_kind"
                            | "l10n.wrapper_name"
                            | "l10n.arg_count"
                            | "l10n.literal"
                            | "asset.ref_kind"
                            | "asset.name"
                    )
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            if metadata.is_empty() {
                None
            } else {
                Some(format!("{}::{:?}::{:?}", node.id, node.kind, metadata))
            }
        })
        .collect::<Vec<_>>();

    let edges = result
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::Inherits | EdgeKind::Implements))
        .map(|edge| format!("{}->{:?}->{:?}", edge.source, edge.kind, edge.target))
        .collect::<Vec<_>>();

    let mut imports = imports;
    let mut nodes = nodes;
    let mut edges = edges;
    imports.sort();
    nodes.sort();
    edges.sort();
    (imports, nodes, edges)
}

#[test]
fn extract_swift_distinguishes_inherits_from_implements() {
    let result = extract_swift(
        fixture(),
        Path::new("semantic_parity.swift"),
        None,
        None,
        true,
    )
    .unwrap();

    assert!(has_edge(
        &result,
        "semantic_parity.swift::Worker",
        "semantic_parity.swift::Base",
        EdgeKind::Inherits,
    ));
    assert!(has_edge(
        &result,
        "semantic_parity.swift::Worker",
        "semantic_parity.swift::Runnable",
        EdgeKind::Implements,
    ));
}

#[test]
fn extract_swift_marks_dynamic_properties_as_invalidation_sources() {
    let result = extract_swift(
        fixture(),
        Path::new("semantic_parity.swift"),
        None,
        None,
        true,
    )
    .unwrap();

    let count = result
        .nodes
        .iter()
        .find(|node| node.name == "count")
        .expect("missing count property");

    assert_eq!(
        count
            .metadata
            .get("swiftui.dynamic_property.wrapper")
            .map(String::as_str),
        Some("state")
    );
    assert_eq!(
        count
            .metadata
            .get("swiftui.invalidation_source")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn extract_swift_without_index_store_matches_fallback_semantics() {
    let source = br#"
    import SwiftUI

    struct ContentView: View {
        let title: String

        var body: some View {
            Text(title)
        }
    }
    "#;

    let path = Path::new("ContentView.swift");
    let disabled = extract_swift(source, path, None, None, false).unwrap();
    let fallback = extract_swift_via_fallback_for_tests(source, path).unwrap();

    assert_eq!(semantic_summary(&disabled), semantic_summary(&fallback));
}

#[test]
fn extract_swift_classifies_protocol_conformance_independent_of_declaration_order() {
    let source = br#"
    class Worker: Runnable, Resettable {}
    protocol Runnable {}
    protocol Resettable {}

    class Runner: Base, Runnable {}
    class Base {}
    "#;

    let result =
        extract_swift_via_fallback_for_tests(source, Path::new("semantic_order.swift")).unwrap();

    assert!(has_edge(
        &result,
        "semantic_order.swift::Worker",
        "semantic_order.swift::Runnable",
        EdgeKind::Implements,
    ));
    assert!(has_edge(
        &result,
        "semantic_order.swift::Worker",
        "semantic_order.swift::Resettable",
        EdgeKind::Implements,
    ));
    assert!(
        !result.edges.iter().any(|edge| {
            edge.source == "semantic_order.swift::Worker" && edge.kind == EdgeKind::Inherits
        }),
        "protocol-only conformance lists must not produce inheritance edges"
    );
    assert!(has_edge(
        &result,
        "semantic_order.swift::Runner",
        "semantic_order.swift::Base",
        EdgeKind::Inherits,
    ));
    assert!(has_edge(
        &result,
        "semantic_order.swift::Runner",
        "semantic_order.swift::Runnable",
        EdgeKind::Implements,
    ));
}

#[test]
fn extract_swift_fallback_keeps_external_superclass_in_mixed_inheritance_lists() {
    let source = br#"
    protocol Runnable {}

    class Screen: UIViewController, Runnable {}
    "#;

    let result =
        extract_swift_via_fallback_for_tests(source, Path::new("external_superclass.swift"))
            .unwrap();

    assert!(has_edge(
        &result,
        "external_superclass.swift::Screen",
        "external_superclass.swift::UIViewController",
        EdgeKind::Inherits,
    ));
    assert!(has_edge(
        &result,
        "external_superclass.swift::Screen",
        "external_superclass.swift::Runnable",
        EdgeKind::Implements,
    ));
}

#[test]
fn extract_swift_fallback_does_not_infer_known_protocols_as_superclasses() {
    let source = br#"
    protocol Runnable {}

    class Worker: Sendable, Runnable {}
    "#;

    let result =
        extract_swift_via_fallback_for_tests(source, Path::new("known_protocols.swift")).unwrap();

    assert!(has_edge(
        &result,
        "known_protocols.swift::Worker",
        "known_protocols.swift::Sendable",
        EdgeKind::Implements,
    ));
    assert!(has_edge(
        &result,
        "known_protocols.swift::Worker",
        "known_protocols.swift::Runnable",
        EdgeKind::Implements,
    ));
    assert!(
        !has_edge(
            &result,
            "known_protocols.swift::Worker",
            "known_protocols.swift::Sendable",
            EdgeKind::Inherits,
        ),
        "known protocol names must not be upgraded into superclass edges"
    );
}

#[test]
fn extract_swift_fallback_does_not_infer_external_protocols_in_mixed_lists() {
    let source = br#"
    protocol Runnable {}

    class Worker: ObservableObject, Runnable {}
    "#;

    let result = extract_swift_via_fallback_for_tests(
        source,
        Path::new("external_protocol_mixed_list.swift"),
    )
    .unwrap();

    assert!(has_edge(
        &result,
        "external_protocol_mixed_list.swift::Worker",
        "external_protocol_mixed_list.swift::ObservableObject",
        EdgeKind::Implements,
    ));
    assert!(has_edge(
        &result,
        "external_protocol_mixed_list.swift::Worker",
        "external_protocol_mixed_list.swift::Runnable",
        EdgeKind::Implements,
    ));
    assert!(
        !has_edge(
            &result,
            "external_protocol_mixed_list.swift::Worker",
            "external_protocol_mixed_list.swift::ObservableObject",
            EdgeKind::Inherits,
        ),
        "external protocol names must not be upgraded into superclass edges"
    );
}

#[test]
fn extract_swift_fallback_keeps_nsobject_superclass_in_mixed_lists() {
    let source = br#"
    protocol Runnable {}

    class Worker: NSObject, Runnable {}
    "#;

    let result = extract_swift_via_fallback_for_tests(
        source,
        Path::new("external_nsobject_superclass.swift"),
    )
    .unwrap();

    assert!(has_edge(
        &result,
        "external_nsobject_superclass.swift::Worker",
        "external_nsobject_superclass.swift::NSObject",
        EdgeKind::Inherits,
    ));
    assert!(has_edge(
        &result,
        "external_nsobject_superclass.swift::Worker",
        "external_nsobject_superclass.swift::Runnable",
        EdgeKind::Implements,
    ));
}

#[test]
fn extract_swift_fallback_does_not_upgrade_view_protocol_to_superclass() {
    let source = br#"
    protocol Runnable {}

    class Worker: View, Runnable {}
    "#;

    let result = extract_swift_via_fallback_for_tests(
        source,
        Path::new("external_view_protocol_collision.swift"),
    )
    .unwrap();

    assert!(has_edge(
        &result,
        "external_view_protocol_collision.swift::Worker",
        "external_view_protocol_collision.swift::View",
        EdgeKind::Implements,
    ));
    assert!(has_edge(
        &result,
        "external_view_protocol_collision.swift::Worker",
        "external_view_protocol_collision.swift::Runnable",
        EdgeKind::Implements,
    ));
    assert!(
        !has_edge(
            &result,
            "external_view_protocol_collision.swift::Worker",
            "external_view_protocol_collision.swift::View",
            EdgeKind::Inherits,
        ),
        "View must stay a protocol conformance in fallback mode"
    );
}

#[test]
fn extract_swift_matches_fallback_semantics_for_task3_fixture() {
    let path = Path::new("semantic_parity.swift");
    let bridge_result = extract_swift(fixture(), path, None, None, true).unwrap();
    let fallback_result = extract_swift_via_fallback_for_tests(fixture(), path).unwrap();

    assert_eq!(
        semantic_summary(&bridge_result),
        semantic_summary(&fallback_result)
    );
}

#[test]
fn extract_swift_covers_localization_and_asset_enrichment_paths() {
    let path = Path::new("semantic_parity.swift");
    let result = extract_swift(fixture(), path, None, None, true).unwrap();
    let raw_fallback = extract_treesitter_fallback_direct(fixture(), path);

    let wrapper = result
        .nodes
        .iter()
        .find(|node| node.id == "semantic_parity.swift::L10n::greeting")
        .expect("expected localized wrapper symbol to be present");
    assert_eq!(
        wrapper
            .metadata
            .get("l10n.wrapper.table")
            .map(String::as_str),
        Some("Localizable")
    );
    assert_eq!(
        wrapper.metadata.get("l10n.wrapper.key").map(String::as_str),
        Some("greeting")
    );
    assert_eq!(
        wrapper
            .metadata
            .get("l10n.wrapper.fallback")
            .map(String::as_str),
        Some("Hello")
    );
    assert_eq!(
        wrapper
            .metadata
            .get("l10n.wrapper.arg_count")
            .map(String::as_str),
        Some("0")
    );
    assert!(
        result.nodes.iter().any(|node| {
            node.metadata.get("l10n.wrapper_name").map(String::as_str) == Some("greeting")
                && node.metadata.get("l10n.ref_kind").map(String::as_str) == Some("wrapper")
        }),
        "expected a localized Text usage to be enriched"
    );
    assert!(
        result.nodes.iter().any(|node| {
            node.metadata.get("asset.name").map(String::as_str) == Some("feature_badge")
                && node.metadata.get("asset.ref_kind").map(String::as_str) == Some("image")
        }),
        "expected an image asset reference to be enriched"
    );
    assert!(
        !raw_fallback
            .nodes
            .iter()
            .any(|node| node.metadata.contains_key("l10n.wrapper.key")),
        "raw fallback extractor should not have wrapper metadata before gating runs"
    );
    assert!(
        !raw_fallback
            .nodes
            .iter()
            .any(|node| node.metadata.contains_key("l10n.ref_kind")),
        "raw fallback extractor should not have localization metadata before gating runs"
    );
    assert!(
        !raw_fallback
            .nodes
            .iter()
            .any(|node| node.metadata.contains_key("asset.name")),
        "raw fallback extractor should not have enrichment metadata before gating runs"
    );
}
