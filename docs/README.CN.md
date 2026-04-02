# Grapha

[English](../README.md)

**极速**代码智能引擎，让 AI 智能体以编译器级精度理解代码库。

Grapha 从源码构建符号级依赖图——不靠正则猜测，而是直接读取编译器的索引。Swift 通过二进制 FFI 直连 Xcode 预编译的 Index Store，拿到 100% 类型解析的符号图，再用 tree-sitter 补充视图结构、文档和国际化信息。Rust 则用 tree-sitter 结合 Cargo 工作空间发现。最终产出一张可查询的图，附带置信度评分的边、数据流追踪、影响分析和代码味道检测——CLI 和 MCP 双模式，为 AI 智能体集成而生。

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
| **MCP 工具** | 11 个 |
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

# 搜索符号
grapha symbol search "ViewModel" --kind struct --context
grapha symbol search "send" --kind function --module Room --fuzzy

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

# 代码味道检测
grapha repo smells --module Room

# 模块指标——符号数、耦合度
grapha repo modules

# MCP 服务器（带文件变更自动刷新）
grapha serve --mcp --watch
```

## MCP 服务器 — 11 个 AI 智能体工具

```bash
grapha serve --mcp              # JSON-RPC stdio
grapha serve --mcp --watch      # + 文件变更自动刷新
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
| `search_symbols` | BM25 搜索，支持 kind/module/role/fuzzy 过滤 |
| `get_symbol_context` | 360° 视图：调用者、被调用者、读取、实现、包含树 |
| `get_impact` | 可配置深度的 BFS 爆炸半径 |
| `trace` | 正向追踪至终端操作，或反向追踪至入口点 |
| `get_file_symbols` | 按源码位置列出文件中所有声明 |
| `batch_context` | 单次调用获取最多 20 个符号的上下文 |
| `analyze_complexity` | 结构指标 + 严重度评级 |
| `detect_smells` | 全图代码味道扫描（上帝类、扇出、嵌套等） |
| `get_module_summary` | 模块级指标，含跨模块耦合度 |
| `get_file_map` | 按模块和目录组织的文件/符号地图 |
| `reload` | 热重载图数据，无需重启服务器 |

**Recall 消歧记忆：** MCP 服务器在会话内记住符号解析结果。如果 `helper` 第一次有歧义，你用 `File.swift::helper` 消歧后，后续裸 `helper` 查询自动解析到同一个符号。

## 命令

### 符号查询

```bash
grapha symbol search "query" [--kind K] [--module M] [--fuzzy] [--context]
grapha symbol context <symbol> [--format tree]
grapha symbol impact <symbol> [--depth N] [--format tree]
grapha symbol complexity <symbol>          # 属性/方法/依赖计数，严重度
grapha symbol file <path>                  # 列出文件中的声明
```

### 数据流

```bash
grapha flow trace <symbol> [--direction forward|reverse] [--depth N]
grapha flow graph <symbol> [--depth N]     # 语义 effect 图
grapha flow entries                        # 列出入口点
```

### 仓库分析

```bash
grapha repo smells [--module M]            # 代码味道检测
grapha repo modules                        # 模块级指标
grapha repo map [--module M]               # 文件/符号概览
grapha repo changes [scope]                # git diff → 受影响的符号
```

### 索引与服务

```bash
grapha index . [--full-rebuild] [--timing]
grapha analyze <path> [--compact] [--filter fn,struct]
grapha serve [--mcp] [--watch] [--port N]
```

### 国际化与资源

```bash
grapha l10n symbol <symbol>                # 从 SwiftUI 子树解析 l10n 记录
grapha l10n usages <key> [--table T]       # 查找国际化 key 的使用位置
grapha asset list [--unused]               # xcassets 目录中的图片资源
grapha asset usages <name>                 # 查找 Image()/UIImage() 引用
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

[[classifiers]]
pattern = "FirebaseFirestore.*setData"
terminal = "persistence"
direction = "write"
operation = "set"
```

## 架构

```
grapha-core/     共享类型（Node, Edge, Graph, ExtractionResult）
grapha-swift/    Swift：Index Store → SwiftSyntax → tree-sitter 瀑布策略
grapha/          CLI、Rust 提取器、查询引擎、MCP 服务器、Web UI
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

**14 种节点：** function, struct, enum, trait, protocol, extension, property, field, variant, constant, type_alias, impl, module, view, branch

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

按语言分 crate 的架构支持以瀑布模式添加新语言：编译器索引 → 语法解析器 → tree-sitter 兜底。

## 开发

```bash
cargo build                    # 构建所有 crate
cargo test                     # 运行全部测试（约 200 个）
cargo clippy && cargo fmt      # 检查 + 格式化
```

## 许可证

MIT
