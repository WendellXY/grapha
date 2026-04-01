# Image Asset Indexing (xcassets)

**Date:** 2026-04-01
**Status:** Draft

---

## Motivation

Grapha already tracks localization references (`.xcstrings` → `L10n.tr()` calls). Image assets follow the same pattern: `.xcassets` catalogs contain image sets that are referenced in Swift source via `Image()`, `UIImage()`, etc. Indexing these references enables usage queries ("which views use this icon?"), dead asset detection, and impact analysis on asset changes.

---

## Asset Discovery & Snapshot

### Discovery

Walk project for `.xcassets` directories, find all `.imageset/Contents.json` files using the `xcassets` crate (`parse_catalog()`). Extract:

- `name` — the image set name (directory name minus `.imageset`)
- `group_path` — hierarchical folder path within the catalog (e.g., `Room/MicSeat`)
- `catalog` — which `.xcassets` directory it belongs to
- `template_rendering_intent` — from Contents.json properties (template/original/none)

### Snapshot Format

Save to `.grapha/assets.json`:

```json
{
  "version": "1",
  "records": [
    {
      "name": "voiceWave",
      "group_path": "Room/MicSeat",
      "catalog": "Assets.xcassets",
      "catalog_dir": ".",
      "template_intent": "template"
    }
  ]
}
```

### Index

```rust
pub struct AssetCatalogIndex {
    records: Vec<AssetRecord>,
    by_name: HashMap<String, Vec<usize>>,  // "voiceWave" → [idx]
}
```

Lookup by name, with optional group_path disambiguation when multiple assets share the same name.

---

## Source Reference Detection

### Patterns

In tree-sitter enrichment (new function `enrich_asset_references_with_tree`), detect:

| Pattern | Extracted `asset.name` |
|---------|----------------------|
| `Image(.Room.voiceWave)` | `Room/voiceWave` |
| `Image(asset: .Profile.icon)` | `Profile/icon` |
| `Image(.Room.RoomList.redPacket)` | `Room/RoomList/redPacket` |
| `UIImage(.FrameUI.commonDefault)` | `FrameUI/commonDefault` |
| `UIImage(resource: .Game.betBtn)` | `Game/betBtn` |
| `UIImage(named: "icon_gift")` | `icon_gift` |
| `UIImage(named: "icon", in: .ui, ...)` | `icon` |
| `Image("icon_gift")` | `icon_gift` |

### Detection Strategy

Walk the tree-sitter AST for `call_expression` nodes where the callee is `Image` or `UIImage`. Extract the first argument:

1. **String literal** (`"icon_gift"`) → use as-is
2. **Dot-expression** (`.Room.voiceWave`) → join path components with `/`
3. **Named argument** (`asset:`, `resource:`, `named:`) → extract the value using rules 1 or 2

### Metadata

On the graph node containing the reference:

```
asset.ref_kind = "image"
asset.name = "Room/voiceWave"
```

### Skip Conditions

Use the same marker-based skip pattern as SwiftUI/l10n: only run asset enrichment on files containing `Image(` or `UIImage(` (byte-level scan).

---

## Pipeline Integration

In `handle_index`, add an `assets_handle` parallel thread:

```rust
let assets_handle = scope.spawn(|| {
    let t = Instant::now();
    let stats = assets::build_and_save_snapshot(&index_root, &store_path)?;
    Ok::<_, anyhow::Error>((t.elapsed(), stats))
});
```

Output line:
```
  ✓ saved asset catalog snapshot (342 image sets) (45.2ms)
```

---

## CLI Commands

New `asset` subcommand group under `Commands`:

```bash
grapha asset list                      # All discovered image assets
grapha asset list --unused             # Dead assets (no references in graph)
grapha asset usages voiceWave          # Which views reference this image?
grapha asset usages voiceWave --format tree
```

### `asset list`

Lists all discovered image assets from `.xcassets` catalogs. With `--unused`, cross-references against the graph to find assets with no `asset.name` metadata matching them.

### `asset usages`

Given an asset name, finds all graph nodes that have `asset.name` matching (exact or suffix match for namespaced references). Returns the node info with file location.

---

## Resolution

Match source references to catalog records:

1. **Exact name match** — `"icon_gift"` matches record with `name: "icon_gift"`
2. **Path match** — `"Room/voiceWave"` matches record with `name: "voiceWave"` and `group_path` containing `Room`
3. **Suffix match** — `"voiceWave"` matches any record where `name == "voiceWave"`, disambiguated by group_path proximity

---

## New Files

- `grapha/src/assets.rs` — discovery, snapshot build/save/load, index, resolution, CLI handlers
- Dependency: `xcassets` crate added to `grapha/Cargo.toml`

---

## Tree-sitter Enrichment

Add `enrich_asset_references_with_tree` in `grapha-swift/src/treesitter.rs`, called from `extract_swift` when the file contains `Image(` or `UIImage(` markers. Shares the same tree-sitter tree as other enrichment passes.

---

## Explicitly Out of Scope

- Color assets (`Color()`, `UIColor()`) — can be added later with the same pattern
- App icon sets — not referenced in source code
- Data sets — rarely referenced by name
- Asset wrapper resolution (following TypeRef edges like l10n) — future enhancement
