# Grapha Workflow

- When exploring an unfamiliar part of the codebase, prefer `grapha symbol search` and `grapha symbol context` over reading entire files
- Before modifying any public API, run `grapha symbol impact` to estimate change scope
- Before refactoring a type, run `grapha symbol complexity` to assess structural health
- Use `grapha repo smells` to find code quality issues across the project
- Use `grapha repo modules` to compare module size and coupling before architectural decisions
- After significant code changes, run `grapha index .` to keep the graph fresh and refresh indexed snippets
- Use `grapha repo map` to orient in unfamiliar modules before diving into files
- When searching for a symbol, start with `grapha symbol search` — it's faster and more precise than grep for symbol-level queries
- Use `grapha symbol search --file ...` and `--role ...` before broadening to fuzzy search when a symbol name is too common
- Use `grapha symbol annotate` for durable symbol notes that should survive future sessions
- Use `grapha annotation serve`, `grapha annotation list`, and `grapha annotation sync --server http://HOST:8080` when sharing annotation knowledge across local machines
- Prefer setting `[repo].name` in `grapha.toml` before syncing non-Git project copies that should share the same annotation identity
