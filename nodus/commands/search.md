---
description: Search project symbols with grapha
---
Run `grapha symbol search "$ARGUMENTS" --context` and present the results.
If no results found, retry with `--fuzzy` flag.
Use `--kind`, `--module`, `--file`, and `--role` filters to narrow noisy matches before falling back to manual file reads.
When `--context` is enabled, expect full symbol snippets for indexed symbols; if snippets look stale or truncated, run `grapha index .` first.
