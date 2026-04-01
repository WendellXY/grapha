---
name: grapha
description: Use grapha for symbol search, context lookup, and impact analysis before reading full files
---

## When to use

Before exploring an unfamiliar part of the codebase or modifying symbols.

## Workflow

1. **Search first:** Run `grapha search "<query>" --context` to find relevant symbols with snippets
2. **Understand relationships:** Run `grapha context <symbol>` to see callers, callees, and dependencies
3. **Check impact before changes:** Run `grapha impact <symbol>` to understand blast radius
4. **Orient in large projects:** Run `grapha repo map` to see module/directory overview
5. **Read only what you need:** Open specific files and line ranges from search results

## Tips

- Use `--kind function` to narrow search to functions only
- Use `--module ModuleName` to search within a specific module
- Use `--fuzzy` if you're unsure of exact spelling
- After significant code changes, run `grapha index .` to keep the graph fresh
