# Changelog

## 0.3.0 - 2026-04-22

### Added

- Added business concept resolution commands so natural-language concepts can be searched, inspected, aliased, and bound to symbols.
- Added cached repository smell queries, plus a `--no-cache` escape hatch for fresh analysis runs.
- Added richer SwiftUI body-structure detection to improve structural understanding during extraction.

### Changed

- Improved CLI resolution for smell, asset, and localization commands.
- Strengthened concept scope recall and restored config-driven semantic overrides in the pipeline.

### Fixed

- Scoped Rust symbol IDs correctly and invalidated stale extraction cache entries to avoid cross-run contamination.
- Aligned the semantic extraction plumbing and cleaned up CI regressions on the release branch.
