# Grapha

[English](../README.md)

**极速**代码智能引擎，让 AI 智能体以编译器级精度理解代码库。

Grapha 从源码构建符号级依赖图——不靠正则猜测，而是读取当前语言能提供的最强结构信息。Swift 通过二进制 FFI 直连 Xcode 预编译的 Index Store，拿到 100% 类型解析的符号图，再用 tree-sitter 补充视图结构、文档、国际化和资源引用信息。Rust 则用 tree-sitter 结合 Cargo 工作空间发现。其他常见语言走通用的 best-effort tree-sitter 提取，覆盖符号、包含关系、导入和基于名称的调用。最终产出一张可查询的图，附带置信度评分的边、数据流追踪、影响分析、代码味道检测和业务概念查找——CLI 和 MCP 双模式，为 AI 智能体集成而生。

> **1,991 个 Swift 文件 — 13.1 万节点 — 78.4 万边 — 8.7 秒。** 零拷贝二进制 FFI，无锁并行提取，热路径零 serde。

## 为什么选 Grapha

| | Grapha |
|---|---|
| **解析精度** | 编译器 Index Store（置信度 1.0）+ tree-sitter 兜底 |
| **关系类型** | 10 种（calls, reads, writes, publishes, subscribes, inherits, implements, contains, type_ref, uses） |
| **数据流追踪** | 正向（入口 → 终端）+ 反向（符号 → 入口） |
| **代码质量** | 复杂度分析、味道检测、模块耦合度 |
| **置信度评分** | 每条边 0.0–1.0 |
| **终端分类** | 自动识别网络、持久化、缓存、事件、钥匙串、搜索 |
| **MCP 工具** | 17 个 |
| **Watch 模式** | 文件监听 + 防抖增量重索引 |
| **Recall** | 会话内消歧记忆——首次消歧后自动解析 |

## 性能

生产级 iOS 应用实测（1,991 个 Swift 文件，约 30 万行）：

| 阶段 | 耗时 |
|------|------|
| 提取（Index Store + tree-sitter 增强） | **3.5 秒** |
| 合并（模块感知的跨文件解析） | 0.3 秒 |
| 分类（入口点 + 终端操作） | 1.7 秒 |
| SQLite 持久化（延迟索引） | 2.0 秒 |
| 搜索索引（BM25 via tantivy） | 1.0 秒 |
| **合计** | **8.7 秒** |

**图规模：** 131,185 节点 · 783,793 边 · 2,983 入口点 · 11,149 终端操作

**为什么这么快：** Index Store 路径走零拷贝指针运算（不经 serde），rayon 无锁并行提取，单次 tree-sitter 共享解析，基于标记跳过非 SwiftUI 文件的增强，SQLite 延迟建索引，USR 作用域边解析。用 `grapha index --timing` 看逐阶段耗时明细。

## 安装

```bash
cargo install grapha
```

## 快速上手

```bash
# 索引项目（默认增量）
grapha index .

# 检查索引新鲜度
grapha repo status

# 搜索符号
grapha symbol search "ViewModel" --kind struct --context --fields full
grapha symbol search "send" --kind function --module Room --fuzzy --declarations-only

# 360° 上下文——调用者、被调用者、读取、实现
grapha symbol context RoomPage --format tree

# 影响分析——改了这个会影响什么？
grapha symbol impact GiftPanelViewModel --depth 2 --format tree

# 复杂度分析——类型的结构健康度
grapha symbol complexity RoomPage

# 数据流：入口 → 终端操作
grapha flow trace RoomPage --format tree

# 反向：哪些入口会经过这个符号？
grapha flow trace sendGift --direction reverse

# 源头追踪：这个 UI 来自哪个 API/数据源？
grapha flow origin UserProfileView --terminal-kind network --format tree

# 代码味道检测
grapha repo smells --module Room
grapha repo smells --file Modules/Room/Sources/Room/View/RoomPage+Layout.swift
grapha repo smells --symbol RoomPageCenterContentView --no-cache

# 模块指标——符号数、耦合度
grapha repo modules

# 架构守护——按配置检查层级依赖规则
grapha repo arch

# 业务概念查找
grapha concept search "送礼横幅" --format tree
grapha concept bind "送礼横幅" --symbol GiftBannerPage --symbol GiftBannerViewModel

# MCP 服务器（带文件变更自动刷新）
grapha serve --mcp --watch
```

## MCP 服务器 — 17 个 AI 智能体工具

```bash
grapha serve --mcp              # JSON-RPC stdio
grapha serve --mcp --watch      # + 文件变更自动刷新
grapha index . && grapha serve --mcp --watch
```

添加到 `.mcp.json`：

```json
{
  "mcpServers": {
    "grapha": {
      "command": "grapha",
      "args": ["serve", "--mcp", "--watch", "-p", "."]
    }
  }
}
```

| 工具 | 功能 |
|------|------|
| `search_symbols` | BM25 搜索，支持 kind/module/file/role/fuzzy 过滤 |
| `get_index_status` | 索引时间戳、仓库快照元数据和陈旧结果提示 |
| `get_symbol_context` | 360° 视图：调用者、被调用者、读取、实现、包含树 |
| `get_impact` | 可配置深度的 BFS 爆炸半径 |
| `get_file_map` | 按模块和目录组织的文件/符号地图 |
| `trace` | 正向追踪至终端操作，或反向追踪至入口点 |
| `get_file_symbols` | 按源码位置列出文件中所有声明 |
| `batch_context` | 单次调用获取最多 20 个符号的上下文 |
| `analyze_complexity` | 结构指标 + 严重度评级 |
| `detect_smells` | 按仓库、模块、文件或符号扫描代码味道 |
| `get_module_summary` | 模块级指标，含跨模块耦合度 |
| `search_concepts` | 跨绑定、国际化、资源和符号进行默认模糊业务概念查找 |
| `get_concept` | 查看已存概念别名和绑定符号 |
| `bind_concept` | 持久化确认后的概念到符号映射 |
| `add_concept_alias` | 为概念添加别名 |
| `remove_concept` | 从项目概念库删除概念 |
| `reload` | 热重载图数据，无需重启服务器 |

**Recall 消歧记忆：** MCP 服务器在会话内记住符号解析结果。如果 `helper` 第一次有歧义，你用 `File.swift::helper` 消歧后，后续裸 `helper` 查询自动解析到同一个符号。服务器未使用 `--watch` 时，手动运行 `grapha index` 后可调用 `reload` 载入最新图数据。

## 命令

### 符号查询

```bash
grapha symbol search "query" [--limit N] [--kind K] [--module M] [--file GLOB] [--role R]
grapha symbol search "query" [--fuzzy] [--exact-name] [--declarations-only] [--public-only]
grapha symbol search "query" [--context] [--fields file,id,module,snippet]
grapha symbol context <symbol> [--format json|tree] [--fields full]
grapha symbol impact <symbol> [--depth N] [--format json|tree] [--fields file,module]
grapha symbol complexity <symbol>          # 属性/方法/依赖计数，严重度
grapha symbol file <path>                  # 列出文件中的声明
```

### 数据流

```bash
grapha flow trace <symbol> [--direction forward|reverse] [--depth N] [--format json|tree]
grapha flow graph <symbol> [--depth N] [--format json|tree]       # 语义 effect 图
grapha flow origin <symbol> [--terminal-kind network|persistence|cache|event|keychain|search]
grapha flow entries [--module M] [--file PATH] [--limit N] [--format json|tree]
```

### 仓库分析

```bash
grapha repo status                         # 索引新鲜度和快照元数据
grapha repo smells [--module M | --file PATH | --symbol QUERY] [--no-cache]
grapha repo modules                        # 模块级指标
grapha repo map [--module M]               # 文件/符号概览
grapha repo changes [unstaged|staged|all|REF]
grapha repo arch                           # 配置化架构规则违规
```

### 索引与服务

```bash
grapha index <path> [--format sqlite|json] [--store-dir DIR] [--full-rebuild] [--timing]
grapha analyze <path> [--compact] [--filter fn,struct]
grapha serve [-p PATH] [--mcp] [--watch] [--port N]
```

### 国际化与资源

```bash
grapha l10n symbol <symbol> [--format json|tree]
grapha l10n usages <key> [--table T] [--format json|tree]
grapha asset list [--unused]               # xcassets 目录中的图片资源
grapha asset usages <name> [--format json|tree]
```

### 业务概念

```bash
grapha concept search "送礼横幅" [--limit N] [--format json|tree]   # 默认模糊，limit 默认 20
grapha concept show "送礼横幅" [--format json|tree]
grapha concept bind "送礼横幅" --symbol GiftBannerPage --symbol GiftBannerViewModel
grapha concept alias "送礼横幅" --add "礼物 banner" --add "gift banner"
grapha concept remove "送礼横幅"
grapha concept prune                       # 清理指向失效 symbol 的绑定
```

## 配置

项目根目录可选 `grapha.toml`：

```toml
[swift]
index_store = true                         # false → 仅用 tree-sitter

[output]
default_fields = ["file", "module"]

[[external]]
name = "FrameUI"
path = "/path/to/local/frameui"            # 纳入跨仓库分析

[[architecture.layers]]
name = "ui"
patterns = ["AppUI*", "Features/*/View*"]

[[architecture.layers]]
name = "infra"
patterns = ["Networking*", "Persistence*"]

[[architecture.deny]]
from = "infra"
to = "ui"
reason = "Infrastructure must not depend on UI."

[[classifiers]]
pattern = "FirebaseFirestore.*setData"
terminal = "persistence"
direction = "write"
operation = "set"
```

## 架构

```
grapha-core/     共享类型（Node, Edge, Graph, ExtractionResult）
grapha-rust/     Rust 插件和 tree-sitter 提取器
grapha-swift/    Swift：Index Store → SwiftSyntax → tree-sitter 瀑布策略
grapha/          CLI、查询引擎、MCP 服务器、Web UI
nodus/           智能体工具包（skills、rules、commands）
```

### 提取瀑布策略（Swift）

```
Xcode Index Store（二进制 FFI）     → 编译器解析的 USR，置信度 1.0
  ↓ 回退
SwiftSyntax（JSON FFI）            → 精确解析，无类型解析，置信度 0.9
  ↓ 回退
tree-sitter-swift（内嵌）          → 快速兜底，置信度 0.6–0.8
```

Index Store 提取后，tree-sitter 在单次共享解析中增强文档注释、SwiftUI 视图层级和国际化元数据。

### 图模型

**16 种节点：** function, class, struct, enum, trait, impl, module, field, variant, property, constant, type_alias, protocol, extension, view, branch

**10 种边：** calls, implements, inherits, contains, type_ref, uses, reads, writes, publishes, subscribes

**数据流注解：** direction (read/write/pure), operation (fetch/save/publish), condition, async_boundary, provenance（源文件 + 位置）

**节点角色：** entry_point（SwiftUI View, @Observable, fn main, #[test]）· terminal（network, persistence, cache, event, keychain, search）

### Nodus 包

```bash
nodus add wenext/grapha --adapter claude
```

一键安装 skills、rules 和 slash commands（`/index`、`/search`、`/impact`、`/complexity`、`/smells`），开箱即用。

## 支持的语言

| 语言 | 提取方式 | 类型解析 |
|------|----------|----------|
| **Swift** | Index Store + tree-sitter | 编译器级（USR） |
| **Rust** | tree-sitter | 基于名称 |
| TypeScript / TSX / JavaScript | best-effort tree-sitter | 基于名称 |
| Python / Go / Java / C / C++ / C# | best-effort tree-sitter | 基于名称 |
| PHP / Ruby / Kotlin / Dart / Pascal | best-effort tree-sitter | 基于名称 |

Swift 和 Rust 仍是第一等提取器。其他语言共享一条通用 tree-sitter 路径，提供有用的图覆盖，但不伪装成编译器级语义。

## 开发

```bash
cargo build                    # 构建所有 crate
cargo test                     # 运行工作区测试套件
cargo clippy && cargo fmt      # 检查 + 格式化
```

## 许可证

MIT
