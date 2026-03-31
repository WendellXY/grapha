use std::collections::HashMap;
use std::path::PathBuf;

use grapha_core::graph::{Edge, EdgeKind, Node, NodeKind, Span, Visibility};
use grapha_core::ExtractionResult;

const MAGIC: u32 = 0x47524148; // "GRAH"
const VERSION: u8 = 1;
const HEADER_SIZE: usize = 20;
const PACKED_NODE_SIZE: usize = 44;
const PACKED_EDGE_SIZE: usize = 20;
const NO_MODULE: u32 = 0xFFFFFFFF;

/// Parse a binary buffer produced by the Swift bridge into an `ExtractionResult`.
pub fn parse_binary_buffer(buf: &[u8]) -> Option<ExtractionResult> {
    if buf.len() < HEADER_SIZE {
        return None;
    }

    let magic = read_u32(buf, 0);
    if magic != MAGIC {
        return None;
    }

    let version = buf[4];
    if version != VERSION {
        return None;
    }
    // buf[5..8] reserved/padding

    let node_count = read_u32(buf, 8) as usize;
    let edge_count = read_u32(buf, 12) as usize;
    let string_table_offset = read_u32(buf, 16) as usize;

    let expected_offset = HEADER_SIZE + node_count * PACKED_NODE_SIZE + edge_count * PACKED_EDGE_SIZE;
    if string_table_offset != expected_offset {
        return None;
    }

    let total_needed = string_table_offset;
    if buf.len() < total_needed {
        return None;
    }

    let string_table = &buf[string_table_offset..];

    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let offset = HEADER_SIZE + i * PACKED_NODE_SIZE;
        let chunk = buf.get(offset..offset + PACKED_NODE_SIZE)?;
        nodes.push(read_node(chunk, string_table)?);
    }

    let edges_start = HEADER_SIZE + node_count * PACKED_NODE_SIZE;
    let mut edges = Vec::with_capacity(edge_count);
    for i in 0..edge_count {
        let offset = edges_start + i * PACKED_EDGE_SIZE;
        let chunk = buf.get(offset..offset + PACKED_EDGE_SIZE)?;
        edges.push(read_edge(chunk, string_table)?);
    }

    Some(ExtractionResult {
        nodes,
        edges,
        imports: vec![],
    })
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

fn read_str<'a>(string_table: &'a [u8], offset: u32, len: u32) -> Option<&'a str> {
    let start = offset as usize;
    let end = start.checked_add(len as usize)?;
    let bytes = string_table.get(start..end)?;
    std::str::from_utf8(bytes).ok()
}

fn decode_node_kind(v: u8) -> Option<NodeKind> {
    match v {
        0 => Some(NodeKind::Function),
        1 => Some(NodeKind::Struct),
        2 => Some(NodeKind::Enum),
        3 => Some(NodeKind::Protocol),
        4 => Some(NodeKind::Extension),
        5 => Some(NodeKind::TypeAlias),
        6 => Some(NodeKind::Property),
        7 => Some(NodeKind::Field),
        8 => Some(NodeKind::Variant),
        _ => None,
    }
}

fn decode_edge_kind(v: u8) -> Option<EdgeKind> {
    match v {
        0 => Some(EdgeKind::Calls),
        1 => Some(EdgeKind::Contains),
        2 => Some(EdgeKind::Inherits),
        3 => Some(EdgeKind::Implements),
        4 => Some(EdgeKind::TypeRef),
        _ => None,
    }
}

fn decode_visibility(v: u8) -> Option<Visibility> {
    match v {
        0 => Some(Visibility::Public),
        1 => Some(Visibility::Crate),
        2 => Some(Visibility::Private),
        _ => None,
    }
}

/// PackedNode layout (44 bytes):
///   id_off(4) id_len(4) name_off(4) name_len(4) file_off(4) file_len(4)
///   module_off(4) module_len(4) line(4) col(4) kind(1) visibility(1) pad(2)
fn read_node(chunk: &[u8], string_table: &[u8]) -> Option<Node> {
    let id_off = read_u32(chunk, 0);
    let id_len = read_u32(chunk, 4);
    let name_off = read_u32(chunk, 8);
    let name_len = read_u32(chunk, 12);
    let file_off = read_u32(chunk, 16);
    let file_len = read_u32(chunk, 20);
    let module_off = read_u32(chunk, 24);
    let module_len = read_u32(chunk, 28);
    let line = read_u32(chunk, 32) as usize;
    let col = read_u32(chunk, 36) as usize;
    let kind = decode_node_kind(chunk[40])?;
    let visibility = decode_visibility(chunk[41])?;

    let id = read_str(string_table, id_off, id_len)?.to_string();
    let name = read_str(string_table, name_off, name_len)?.to_string();
    let file = PathBuf::from(read_str(string_table, file_off, file_len)?);

    let module = if module_off == NO_MODULE {
        None
    } else {
        Some(read_str(string_table, module_off, module_len)?.to_string())
    };

    Some(Node {
        id,
        kind,
        name,
        file,
        span: Span {
            start: [line, col],
            end: [line, col],
        },
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module,
    })
}

/// PackedEdge layout (20 bytes):
///   source_off(4) source_len(4) target_off(4) target_len(4)
///   kind(1) confidence_pct(1) pad(2)
fn read_edge(chunk: &[u8], string_table: &[u8]) -> Option<Edge> {
    let source_off = read_u32(chunk, 0);
    let source_len = read_u32(chunk, 4);
    let target_off = read_u32(chunk, 8);
    let target_len = read_u32(chunk, 12);
    let kind = decode_edge_kind(chunk[16])?;
    let confidence = chunk[17] as f64 / 100.0;

    let source = read_str(string_table, source_off, source_len)?.to_string();
    let target = read_str(string_table, target_off, target_len)?.to_string();

    Some(Edge {
        source,
        target,
        kind,
        confidence,
        direction: None,
        operation: None,
        condition: None,
        async_boundary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid binary buffer with the given nodes, edges, and string table.
    fn build_buffer(
        node_chunks: &[Vec<u8>],
        edge_chunks: &[Vec<u8>],
        string_table: &[u8],
    ) -> Vec<u8> {
        let node_count = node_chunks.len() as u32;
        let edge_count = edge_chunks.len() as u32;
        let string_table_offset =
            HEADER_SIZE as u32 + node_count * PACKED_NODE_SIZE as u32 + edge_count * PACKED_EDGE_SIZE as u32;

        let mut buf = Vec::new();
        // Header
        buf.extend_from_slice(&MAGIC.to_le_bytes());
        buf.push(VERSION);
        buf.extend_from_slice(&[0u8; 3]); // reserved
        buf.extend_from_slice(&node_count.to_le_bytes());
        buf.extend_from_slice(&edge_count.to_le_bytes());
        buf.extend_from_slice(&string_table_offset.to_le_bytes());

        for chunk in node_chunks {
            buf.extend_from_slice(chunk);
        }
        for chunk in edge_chunks {
            buf.extend_from_slice(chunk);
        }
        buf.extend_from_slice(string_table);
        buf
    }

    fn make_packed_node(
        id_off: u32,
        id_len: u32,
        name_off: u32,
        name_len: u32,
        file_off: u32,
        file_len: u32,
        module_off: u32,
        module_len: u32,
        line: u32,
        col: u32,
        kind: u8,
        visibility: u8,
    ) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(PACKED_NODE_SIZE);
        chunk.extend_from_slice(&id_off.to_le_bytes());
        chunk.extend_from_slice(&id_len.to_le_bytes());
        chunk.extend_from_slice(&name_off.to_le_bytes());
        chunk.extend_from_slice(&name_len.to_le_bytes());
        chunk.extend_from_slice(&file_off.to_le_bytes());
        chunk.extend_from_slice(&file_len.to_le_bytes());
        chunk.extend_from_slice(&module_off.to_le_bytes());
        chunk.extend_from_slice(&module_len.to_le_bytes());
        chunk.extend_from_slice(&line.to_le_bytes());
        chunk.extend_from_slice(&col.to_le_bytes());
        chunk.push(kind);
        chunk.push(visibility);
        chunk.extend_from_slice(&[0u8; 2]); // pad
        chunk
    }

    fn make_packed_edge(
        source_off: u32,
        source_len: u32,
        target_off: u32,
        target_len: u32,
        kind: u8,
        confidence_pct: u8,
    ) -> Vec<u8> {
        let mut chunk = Vec::with_capacity(PACKED_EDGE_SIZE);
        chunk.extend_from_slice(&source_off.to_le_bytes());
        chunk.extend_from_slice(&source_len.to_le_bytes());
        chunk.extend_from_slice(&target_off.to_le_bytes());
        chunk.extend_from_slice(&target_len.to_le_bytes());
        chunk.push(kind);
        chunk.push(confidence_pct);
        chunk.extend_from_slice(&[0u8; 2]); // pad
        chunk
    }

    #[test]
    fn test_empty_buffer_parses() {
        let buf = build_buffer(&[], &[], b"");
        let result = parse_binary_buffer(&buf).unwrap();
        assert!(result.nodes.is_empty());
        assert!(result.edges.is_empty());
        assert!(result.imports.is_empty());
    }

    #[test]
    fn test_bad_magic_returns_none() {
        let mut buf = build_buffer(&[], &[], b"");
        // Corrupt magic
        buf[0] = 0x00;
        assert!(parse_binary_buffer(&buf).is_none());
    }

    #[test]
    fn test_bad_version_returns_none() {
        let mut buf = build_buffer(&[], &[], b"");
        // Set version to 99
        buf[4] = 99;
        assert!(parse_binary_buffer(&buf).is_none());
    }

    #[test]
    fn test_truncated_buffer_returns_none() {
        // Less than header size
        assert!(parse_binary_buffer(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_one_node_one_edge() {
        // String table: "s::MyApp::foo" (0..13), "foo" (13..16), "/src/main.swift" (16..31),
        //               "MyApp" (31..36), "s::MyApp::bar" (36..49)
        let string_table = b"s::MyApp::foofoo/src/main.swiftMyApps::MyApp::bar";

        let node = make_packed_node(
            0, 13,  // id: "s::MyApp::foo"
            13, 3,  // name: "foo"
            16, 15, // file: "/src/main.swift"
            31, 5,  // module: "MyApp"
            42, 10, // line 42, col 10
            0,      // kind: Function
            0,      // visibility: Public
        );

        let edge = make_packed_edge(
            0, 13,  // source: "s::MyApp::foo"
            36, 13, // target: "s::MyApp::bar"
            0,      // kind: Calls
            95,     // confidence: 95%
        );

        let buf = build_buffer(&[node], &[edge], string_table);
        let result = parse_binary_buffer(&buf).unwrap();

        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.edges.len(), 1);

        let n = &result.nodes[0];
        assert_eq!(n.id, "s::MyApp::foo");
        assert_eq!(n.name, "foo");
        assert_eq!(n.file, PathBuf::from("/src/main.swift"));
        assert_eq!(n.module.as_deref(), Some("MyApp"));
        assert_eq!(n.span.start, [42, 10]);
        assert_eq!(n.kind, NodeKind::Function);
        assert_eq!(n.visibility, Visibility::Public);

        let e = &result.edges[0];
        assert_eq!(e.source, "s::MyApp::foo");
        assert_eq!(e.target, "s::MyApp::bar");
        assert_eq!(e.kind, EdgeKind::Calls);
        assert!((e.confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn test_string_out_of_bounds_returns_none() {
        // Node references string at offset 999 which is beyond the string table
        let node = make_packed_node(
            999, 5, // id offset way out of bounds
            0, 3,
            0, 3,
            NO_MODULE, 0,
            1, 0,
            0, 0,
        );

        let buf = build_buffer(&[node], &[], b"abc");
        assert!(parse_binary_buffer(&buf).is_none());
    }
}
