# Grapha Workflow

- When exploring an unfamiliar part of the codebase, prefer `grapha search` and `grapha context` over reading entire files
- Before modifying any public API, run `grapha impact` to estimate change scope
- After significant code changes, run `grapha index .` to keep the graph fresh
- Use `grapha repo map` to orient in unfamiliar modules before diving into files
- When searching for a symbol, start with `grapha search` — it's faster and more precise than grep for symbol-level queries
