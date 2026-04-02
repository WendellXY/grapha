---
name: grapha-workflow
description: Use grapha for symbol search, context lookup, complexity analysis, and impact assessment before reading full files or modifying code
---

# Grapha Workflow

Use grapha's code intelligence to navigate, understand, and assess codebases before making changes.

## When to use

- Before exploring an unfamiliar part of the codebase
- Before modifying public APIs or shared types
- When assessing code quality or refactoring candidates
- When orienting in a large project

## Core workflow

1. **Search first:** `grapha symbol search "<query>" --context` to find relevant symbols with full indexed snippets
2. **Understand relationships:** `grapha symbol context <symbol>` to see callers, callees, and dependencies
3. **Check impact before changes:** `grapha symbol impact <symbol>` to understand blast radius
4. **Assess complexity:** `grapha symbol complexity <type>` to check structural health of a type
5. **Orient in large projects:** `grapha repo modules` for per-module metrics, `grapha repo map` for file layout

## Quality assessment

- `grapha repo smells` — scan the full graph for code smells (god types, deep nesting, wide invalidation, excessive fan-out)
- `grapha repo smells --module Room` — scope to a single module
- `grapha symbol complexity <type>` — detailed metrics for a specific type (properties, dependencies, init params, invalidation sources)

## Dataflow tracing

- `grapha flow trace <symbol>` — follow data forward from a symbol to terminals (network, persistence, etc.)
- `grapha flow trace <symbol> --direction reverse` — find which entry points reach a symbol
- `grapha flow entries` — list auto-detected entry points

## Tips

- Use `--kind function` to narrow search to functions only
- Use `--module ModuleName` to search within a specific module
- Use `--file RoomPage.swift` to restrict results to a file or `--file "Sources/*/RoomPage.swift"` for a glob
- Use `--role entry_point`, `--role terminal`, or `--role internal` when symbol names are common
- Use `--fuzzy` if unsure of exact spelling
- Use `file.swift::symbol` to disambiguate when multiple symbols share a name
- After significant code changes, run `grapha index .` to keep the graph fresh and refresh stored snippets
