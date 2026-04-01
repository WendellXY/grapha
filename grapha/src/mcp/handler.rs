use std::path::PathBuf;

use grapha_core::graph::Graph;
use serde_json::{Value, json};
use tantivy::Index;

use crate::mcp::types::ToolDefinition;
use crate::query;
use crate::search;

pub struct McpState {
    pub graph: Graph,
    pub search_index: Index,
    #[allow(dead_code)] // Will be used by index_project tool
    pub store_path: PathBuf,
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_symbols".to_string(),
            description: "Search for symbols by name, kind, module, file, or role. Returns matching symbols with relevance scores.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (symbol name or keyword)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 20)",
                        "default": 20
                    },
                    "kind": {
                        "type": "string",
                        "description": "Filter by symbol kind (function, struct, enum, trait, etc.)"
                    },
                    "module": {
                        "type": "string",
                        "description": "Filter by module name"
                    },
                    "file": {
                        "type": "string",
                        "description": "Filter by file path glob"
                    },
                    "role": {
                        "type": "string",
                        "description": "Filter by role (entry_point, terminal, internal)"
                    },
                    "fuzzy": {
                        "type": "boolean",
                        "description": "Enable fuzzy matching (default: false)",
                        "default": false
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_symbol_context".to_string(),
            description: "Get 360-degree context for a symbol: callers, callees, implementors, containment, and type references.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name or ID"
                    }
                },
                "required": ["symbol"]
            }),
        },
        ToolDefinition {
            name: "get_impact".to_string(),
            description: "Analyze the blast radius of changing a symbol using BFS traversal.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name or ID"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Maximum traversal depth (default: 3)",
                        "default": 3
                    }
                },
                "required": ["symbol"]
            }),
        },
        ToolDefinition {
            name: "get_file_map".to_string(),
            description: "Get a map of files and symbols organized by module and directory.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "module": {
                        "type": "string",
                        "description": "Filter by module name (optional)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "trace".to_string(),
            description: "Trace dataflow forward from a symbol to terminals, or reverse from a symbol back to entry points.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name or ID"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["forward", "reverse"],
                        "description": "Trace direction (default: forward)",
                        "default": "forward"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Maximum traversal depth (default: 10 for forward, unlimited for reverse)"
                    }
                },
                "required": ["symbol"]
            }),
        },
        ToolDefinition {
            name: "index_project".to_string(),
            description: "Re-index the project to update the graph and search index. (Not yet implemented via MCP.)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project directory to index"
                    }
                }
            }),
        },
    ]
}

fn text_content(text: String) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": message
        }],
        "isError": true
    })
}

fn format_query_error(err: &query::QueryResolveError) -> String {
    match err {
        query::QueryResolveError::NotFound { query } => {
            format!("symbol not found: {query}")
        }
        query::QueryResolveError::Ambiguous { query, candidates } => {
            let mut msg = format!("ambiguous query: {query}\n");
            for c in candidates {
                msg.push_str(&format!("  - {} [{:?}] in {}\n", c.name, c.kind, c.file));
            }
            msg.push_str(&format!("hint: {}", query::ambiguity_hint()));
            msg
        }
        query::QueryResolveError::NotFunction { hint } => hint.clone(),
    }
}

pub fn handle_tool_call(state: &McpState, tool_name: &str, arguments: &Value) -> Value {
    match tool_name {
        "search_symbols" => handle_search_symbols(state, arguments),
        "get_symbol_context" => handle_get_symbol_context(state, arguments),
        "get_impact" => handle_get_impact(state, arguments),
        "get_file_map" => handle_get_file_map(state, arguments),
        "trace" => handle_trace(state, arguments),
        "index_project" => handle_index_project(),
        _ => tool_error(format!("unknown tool: {tool_name}")),
    }
}

fn handle_search_symbols(state: &McpState, arguments: &Value) -> Value {
    let query_str = match arguments.get("query").and_then(|v| v.as_str()) {
        Some(q) => q,
        None => return tool_error("missing required parameter: query".to_string()),
    };
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;
    let options = search::SearchOptions {
        kind: arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .map(String::from),
        module: arguments
            .get("module")
            .and_then(|v| v.as_str())
            .map(String::from),
        file_glob: arguments
            .get("file")
            .and_then(|v| v.as_str())
            .map(String::from),
        role: arguments
            .get("role")
            .and_then(|v| v.as_str())
            .map(String::from),
        fuzzy: arguments
            .get("fuzzy")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    match search::search_filtered(&state.search_index, query_str, limit, &options) {
        Ok(results) => match serde_json::to_string_pretty(&results) {
            Ok(json) => text_content(json),
            Err(e) => tool_error(format!("failed to serialize results: {e}")),
        },
        Err(e) => tool_error(format!("search failed: {e}")),
    }
}

fn handle_get_symbol_context(state: &McpState, arguments: &Value) -> Value {
    let symbol = match arguments.get("symbol").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return tool_error("missing required parameter: symbol".to_string()),
    };

    match query::context::query_context(&state.graph, symbol) {
        Ok(result) => match serde_json::to_string_pretty(&result) {
            Ok(json) => text_content(json),
            Err(e) => tool_error(format!("failed to serialize result: {e}")),
        },
        Err(e) => tool_error(format_query_error(&e)),
    }
}

fn handle_get_impact(state: &McpState, arguments: &Value) -> Value {
    let symbol = match arguments.get("symbol").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return tool_error("missing required parameter: symbol".to_string()),
    };
    let depth = arguments.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

    match query::impact::query_impact(&state.graph, symbol, depth) {
        Ok(result) => match serde_json::to_string_pretty(&result) {
            Ok(json) => text_content(json),
            Err(e) => tool_error(format!("failed to serialize result: {e}")),
        },
        Err(e) => tool_error(format_query_error(&e)),
    }
}

fn handle_get_file_map(state: &McpState, arguments: &Value) -> Value {
    let module = arguments.get("module").and_then(|v| v.as_str());

    let result = query::map::file_map(&state.graph, module);
    match serde_json::to_string_pretty(&result) {
        Ok(json) => text_content(json),
        Err(e) => tool_error(format!("failed to serialize result: {e}")),
    }
}

fn handle_trace(state: &McpState, arguments: &Value) -> Value {
    let symbol = match arguments.get("symbol").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return tool_error("missing required parameter: symbol".to_string()),
    };
    let direction = arguments
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("forward");
    let depth = arguments.get("depth").and_then(|v| v.as_u64());

    match direction {
        "forward" => {
            let max_depth = depth.unwrap_or(10) as usize;
            match query::trace::query_trace(&state.graph, symbol, max_depth) {
                Ok(result) => match serde_json::to_string_pretty(&result) {
                    Ok(json) => text_content(json),
                    Err(e) => tool_error(format!("failed to serialize result: {e}")),
                },
                Err(e) => tool_error(format_query_error(&e)),
            }
        }
        "reverse" => {
            let max_depth = depth.map(|d| d as usize);
            match query::reverse::query_reverse(&state.graph, symbol, max_depth) {
                Ok(result) => match serde_json::to_string_pretty(&result) {
                    Ok(json) => text_content(json),
                    Err(e) => tool_error(format!("failed to serialize result: {e}")),
                },
                Err(e) => tool_error(format_query_error(&e)),
            }
        }
        other => tool_error(format!(
            "invalid direction: {other} (expected \"forward\" or \"reverse\")"
        )),
    }
}

fn handle_index_project() -> Value {
    tool_error(
        "index_project is not yet available via MCP. Please run `grapha index <path>` from the CLI."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_returns_six_tools() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 6);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_symbols"));
        assert!(names.contains(&"get_symbol_context"));
        assert!(names.contains(&"get_impact"));
        assert!(names.contains(&"get_file_map"));
        assert!(names.contains(&"trace"));
        assert!(names.contains(&"index_project"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let state = make_test_state();
        let result = handle_tool_call(&state, "nonexistent", &json!({}));
        assert!(
            result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        );
    }

    #[test]
    fn search_symbols_missing_query_returns_error() {
        let state = make_test_state();
        let result = handle_tool_call(&state, "search_symbols", &json!({}));
        assert!(
            result
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        );
    }

    #[test]
    fn index_project_returns_placeholder() {
        let state = make_test_state();
        let result = handle_tool_call(&state, "index_project", &json!({}));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not yet available"));
    }

    fn make_test_state() -> McpState {
        let graph = Graph {
            version: String::new(),
            nodes: vec![],
            edges: vec![],
        };
        let schema = tantivy::schema::Schema::builder().build();
        let index = Index::create_in_ram(schema);
        McpState {
            graph,
            search_index: index,
            store_path: PathBuf::from("/tmp/test"),
        }
    }
}
