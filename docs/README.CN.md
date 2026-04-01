# Grapha

[English](../README.md)

极速代码智能，为 LLM 智能体和开发者工具而生。

Grapha 将源代码转换为标准化、图结构的表示，具备编译器级别的精度。对于 Swift，它通过二进制 FFI 桥接读取 Xcode 预编译的 Index Store，获取完整类型解析的符号图；在无编译产物时会依次回退到 SwiftSyntax 和 tree-sitter，实现即时解析。生成的图支持持久化、搜索、数据流追踪和影响分析 —— 让智能体和开发者能够结构化地访问大规模代码库。

> **1,991 个 Swift 文件 — 12.3 万节点，76.6 万编译器解析的边 — 6 秒完成索引。**

## 性能

在生产级 iOS 应用上实测（1,991 个 Swift 文件，约 30 万行代码）：

| 阶段 | 耗时 |
|------|------|
| 提取（Index Store + 二进制 FFI） | **1.8 秒** |
| 合并（模块感知的跨文件解析） | 0.15 秒 |
| 分类（入口点 + 终端操作） | 0.97 秒 |
| SQLite 持久化（88.9 万行） | 2.1 秒 |
| 搜索索引（BM25 via tantivy） | 0.8 秒 |
| **合计** | **6.0 秒** |

| 指标 | 数值 |
|------|------|
| 节点数 | 123,323 |
| 边数（编译器解析） | 766,427 |
| 入口点（自动检测） | 2,985 |
| 终端操作 | 10,548 |

**为什么这么快：**
- **Index Store 路径是零解析二进制 FFI** — Index Store 桥接返回紧凑结构体 + 去重字符串表。Rust 端在编译器级路径上通过指针运算直接读取，无需 serde 反序列化。
- **复用 Index Store** — 直接读取 Xcode 已编译的符号数据库，无需重新解析、无需重新做类型解析。
- **回退层仍然很快** — 当没有 Index Store 时，SwiftSyntax 通过同一个桥接运行，tree-sitter 继续作为最后一层回退解析器。
- **延迟索引构建** — SQLite 索引在批量插入完成后创建，而非逐行维护。
- **并行提取** — 基于 rayon 的并发文件处理。

## 功能特性

- **编译器级精度** — 读取 Xcode 预编译的 Index Store，获取 100% 类型解析的调用图（Swift）。无编译产物时会依次回退到 SwiftSyntax 和 tree-sitter，实现即时解析。
- **数据流追踪** — 从入口点正向追踪到终端操作（网络、持久化、缓存），或从任意符号反向追踪到受影响的入口点。
- **语义数据流图** — 通过 `grapha flow graph` 从符号导出去重后的 effect 图，包含 read、write、publish、subscribe 和终端副作用。
- **影响分析** — BFS 爆炸半径："如果我改了这个函数，什么会受影响？"
- **入口点检测** — 自动识别 SwiftUI View、`@Observable` 类、`fn main()`、`#[test]` 函数。
- **终端分类** — 识别网络调用、持久化（GRDB、CoreData）、缓存（Kingfisher）、统计分析等。支持通过 `grapha.toml` 扩展自定义规则。
- **跨模块解析** — 基于 import 的消歧义，带置信度评分。支持多 Package 项目的模块感知合并。
- **Web UI** — 内嵌交互式图可视化（`grapha serve`）。
- **多语言** — 目前支持 Rust 和 Swift。架构可扩展至 Java、Kotlin、C#、TypeScript。

## 安装

```bash
cargo install --path grapha
```

## 快速开始

```bash
# 索引项目
grapha index .

# 搜索符号
grapha symbol search sendMessage

# 获取符号的 360° 上下文
grapha symbol context sendMessage

# 图查询的人类可读树形输出
grapha flow trace handleSendResult --direction reverse --format tree

# 影响分析：改了这个函数，什么会受影响？
grapha symbol impact bootstrapGame --depth 5

# 正向追踪：入口点 → 终端操作
grapha flow trace bootstrapGame

# 反向追踪：哪些入口点会经过这个符号？
grapha flow trace handleSendResult --direction reverse

# 列出自动检测到的入口点
grapha flow entries

# 本地化查询
grapha l10n symbol body
grapha l10n usages account_forget_password --table Localizable

# 仓库变更分析
grapha repo changes

# 交互式 Web UI
grapha serve --port 8765
```

## 命令

### `grapha index` — 构建图

```bash
grapha index .                         # 索引项目（SQLite）
grapha index . --format json           # JSON 输出（调试用）
grapha index . --store-dir /tmp/idx    # 自定义存储位置
```

自动从 DerivedData 发现 Xcode 的 Index Store，获取编译器解析的符号。无 Index Store 时自动回退到 tree-sitter。

### `grapha analyze` — 一次性提取

```bash
grapha analyze src/                    # 分析目录
grapha analyze src/main.rs             # 分析单文件
grapha analyze src/ --compact          # LLM 优化的分组输出
grapha analyze src/ --filter fn,struct # 按符号类型过滤
grapha analyze src/ -o graph.json      # 输出到文件
```

### `grapha symbol context` — 360° 符号视图

```bash
grapha symbol context Config                  # 调用者、被调用者、实现者
grapha symbol context bootstrapGame           # 模糊名称匹配
grapha symbol context bootstrapGame --format tree
```

### `grapha symbol impact` — 影响范围分析

```bash
grapha symbol impact bootstrapGame            # 谁依赖这个符号？
grapha symbol impact bootstrapGame --depth 5  # 更深层遍历
grapha symbol impact bootstrapGame --format tree
```

### `grapha flow trace` — 正向/反向数据流追踪

```bash
grapha flow trace bootstrapGame                          # 入口点 → 服务层 → 终端操作
grapha flow trace sendMessage --depth 10
grapha flow trace handleSendResult --direction reverse  # 哪些入口点会经过这里？
grapha flow trace bootstrapGame --format tree
```

### `grapha flow graph` — 语义 effect 图

```bash
grapha flow graph bootstrapGame
grapha flow graph sendMessage --depth 10
grapha flow graph bootstrapGame --format tree
```

### `grapha flow entries` — 列出入口点

```bash
grapha flow entries                    # 所有检测到的入口点
grapha flow entries --format tree
```

### `grapha symbol search` — 全文搜索

```bash
grapha symbol search "ViewModel"
grapha symbol search "send" --limit 10
```

### `grapha l10n symbol` — 解析本地化记录

```bash
grapha l10n symbol body
grapha l10n symbol AuthView --format tree
```

### `grapha l10n usages` — 查找本地化引用

```bash
grapha l10n usages account_forget_password
grapha l10n usages shared_title --table Localizable --format tree
```

### `grapha repo changes` — Git 变更检测

```bash
grapha repo changes                    # 所有未提交的变更
grapha repo changes staged             # 仅暂存区
grapha repo changes main               # 与某个分支对比
```

### `grapha serve` — Web UI

```bash
grapha serve --port 8765               # 打开 http://localhost:8765
```

基于 vis-network 的交互式图可视化：点击节点、追踪流向、搜索符号、按类型/角色过滤。

## 架构

### 工作空间

```
grapha-core/     共享类型（Node、Edge、Graph、ExtractionResult）
grapha-swift/    Swift 提取：Index Store → SwiftSyntax → tree-sitter 瀑布策略
grapha/          CLI 二进制、Rust 提取器、流水线、查询引擎、Web UI
```

### 提取瀑布策略（Swift）

```
1. Xcode Index Store（通过 Swift 桥接的二进制 FFI）
   → 编译器解析的 USR，置信度 1.0
   → 从 DerivedData 自动发现

2. SwiftSyntax（通过 Swift 桥接的 JSON 字符串 FFI）
   → 精确语法解析，无类型解析，置信度 0.9

3. tree-sitter-swift（内嵌）
   → 快速回退，精度有限，置信度 0.6-0.8
```

Swift 桥接库（`libGraphaSwiftBridge.dylib`）在检测到 Swift 工具链时由 `build.rs` 自动编译。Index Store 路径通过扁平二进制缓冲区（紧凑结构体 + 字符串表）跨越 FFI 边界，而 SwiftSyntax 路径当前返回 JSON 字符串，由 Rust 端解码为共享图模型。纯 Rust 项目无需 Swift 环境。

### 流水线

```
发现 → 提取 → 合并 → 分类 → 压缩 → 存储 → 查询/服务
         ↑       ↑       ↑
    Index Store / 模块感知  入口点
    SwiftSyntax / 解析     + 终端操作
    tree-sitter
```

### 图模型

节点代表符号（函数、类型、属性），边代表关系并附带置信度评分。

**节点类型：** `function`（函数）、`struct`（结构体）、`enum`（枚举）、`trait`（特征）、`protocol`（协议）、`extension`（扩展）、`property`（属性）、`field`（字段）、`variant`（枚举变体）、`constant`（常量）、`type_alias`（类型别名）、`impl`（实现块）、`module`（模块）

**边类型：**

| 类型 | 含义 |
|------|------|
| `calls` | 函数/方法调用 |
| `implements` | 协议遵循 / trait 实现 |
| `inherits` | 超类 / 超 trait |
| `contains` | 结构嵌套 |
| `type_ref` | 类型引用 |
| `uses` | 导入语句 |
| `reads` / `writes` | 数据访问方向 |
| `publishes` / `subscribes` | 事件流 |

**边上的数据流注解：**

| 字段 | 用途 |
|------|------|
| `direction` | `read`、`write`、`read_write`、`pure` |
| `operation` | `fetch`、`save`、`publish`、`navigate` 等 |
| `condition` | 守卫/条件文本（条件调用时） |
| `async_boundary` | 是否跨越 async 边界 |

**节点角色：**
- `entry_point` — SwiftUI View.body、@Observable 方法、fn main、#[test]
- `terminal` — 网络、持久化、缓存、事件、钥匙串、搜索

## 配置

可选的 `grapha.toml`，用于自定义分类器和入口点：

```toml
[[classifiers]]
pattern = "FirebaseFirestore.*setData"
terminal = "persistence"
direction = "write"
operation = "set"

[[entry_points]]
language = "swift"
pattern = ".*Coordinator.start"
```

## 支持的语言

| 语言 | 提取方式 | 类型解析 |
|------|----------|----------|
| **Swift** | tree-sitter + Xcode Index Store | 编译器级（USR） |
| **Rust** | tree-sitter | 基于名称 |

按语言分 crate 的架构（`grapha-swift/`，未来 `grapha-java/` 等）支持以相同模式添加新语言：编译器索引 → 语法解析器 → tree-sitter 回退。

## 开发

```bash
cargo build                    # 构建所有工作空间 crate
cargo test                     # 运行所有测试（213 个测试）
cargo build -p grapha-core     # 仅构建共享类型
cargo build -p grapha-swift    # 构建 Swift 提取器
cargo run -p grapha -- <cmd>   # 运行 CLI
cargo clippy                   # 代码检查
cargo fmt                      # 代码格式化
```

## 许可证

MIT
