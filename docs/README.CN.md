# Grapha

极速代码智能，为 LLM 智能体而生。

Grapha 将源代码转换为标准化的图结构表示，专为 LLM 消费优化。通过 [tree-sitter](https://tree-sitter.github.io/) 解析语法，提取符号和关系，并提供持久化、搜索和影响分析——让智能体能够结构化地访问大规模代码库。

> **1,991 个 Swift 文件，30.6 万行代码 — 2.5 秒完成索引，峰值内存 146 MB。**

## 安装

```bash
cargo install --path .
```

## 快速开始

```bash
# 索引项目（持久化到 .grapha/）
grapha index .

# 搜索符号
grapha search Config

# 获取符号的 360° 上下文
grapha context Config

# 图查询的人类可读树形输出
grapha reverse Config --format tree

# 分析变更的影响范围
grapha impact Config --depth 3

# 检测 git 变更及受影响的符号
grapha changes
```

## 命令

### `grapha analyze` — 提取并输出图

```bash
grapha analyze src/              # 分析目录（遵守 .gitignore）
grapha analyze src/main.rs       # 分析单个文件
grapha analyze src/ -o graph.json   # 输出到文件
grapha analyze src/ --filter fn,struct,trait  # 按符号类型过滤
grapha analyze src/ --compact    # LLM 优化的分组输出
```

### `grapha index` — 持久化图到存储

```bash
grapha index .                         # 索引项目（SQLite，默认）
grapha index . --format json           # 以 JSON 格式索引（用于调试）
grapha index . --store-dir /tmp/idx    # 自定义存储位置
```

### `grapha context` — 360° 符号视图

```bash
grapha context Config           # 调用者、被调用者、实现者
grapha context Config -p /path/to/project
grapha context Config --format tree
```

### `grapha impact` — 影响范围分析

```bash
grapha impact Config            # 如果 Config 变更，谁会受影响？
grapha impact Config --depth 5  # 更深层遍历
grapha impact Config --format tree
```

### `grapha search` — BM25 全文搜索

```bash
grapha search "Config"          # 按名称搜索
grapha search "main.rs" --limit 5
```

### `grapha changes` — 基于 Git 的变更检测

```bash
grapha changes              # 所有未提交的变更
grapha changes staged       # 仅暂存区变更
grapha changes main         # 与某个分支对比
```

## 输出格式

### 标准格式（JSON 图）

```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "graph.rs::Config",
      "kind": "struct",
      "name": "Config",
      "file": "graph.rs",
      "span": { "start": [10, 0], "end": [15, 1] },
      "visibility": "public",
      "metadata": {}
    }
  ],
  "edges": [
    {
      "source": "main.rs::run",
      "target": "graph.rs::Config",
      "kind": "type_ref",
      "confidence": 0.85
    }
  ]
}
```

### 紧凑格式（`--compact`）— LLM 优化

```json
{
  "version": "0.1.0",
  "files": {
    "graph.rs": {
      "symbols": [
        {
          "name": "Config",
          "kind": "struct",
          "span": [10, 15],
          "type_refs": ["Node"]
        }
      ]
    }
  }
}
```

### 节点类型

`function`（函数）、`struct`（结构体）、`enum`（枚举）、`trait`（特征）、`impl`（实现块）、`module`（模块）、`field`（字段）、`variant`（枚举变体）、`property`（属性）、`constant`（常量）、`type_alias`（类型别名）、`protocol`（协议）、`extension`（扩展）

### 边类型

| 类型 | 含义 | 置信度 |
|------|------|--------|
| `calls` | 函数调用另一个函数 | 0.8 |
| `uses` | `use`/`import` 导入语句 | 0.7 |
| `implements` | `impl Trait for Type` / 协议遵循 | 0.9 |
| `contains` | 结构嵌套（模块 > 结构体 > 字段） | 1.0 |
| `type_ref` | 签名或字段中的类型引用 | 0.85 |
| `inherits` | 超特征约束（`trait Child: Base`） | 0.9 |

## 支持的语言

- **Rust**（通过 `tree-sitter-rust`）
- **Swift**（通过 `tree-sitter-swift`）

核心设计与语言无关。添加新语言只需实现 `LanguageExtractor` trait 并提供对应的 tree-sitter 语法。

## 设计原则

- **结构化，而非语义化** — tree-sitter 解析语法而非类型。函数调用解析基于名称匹配并附带置信度评分，不进行类型推断，也不支持跨 crate 解析。
- **为 LLM 优化** — 最小化 token 数量，确定性 ID，扁平 JSON 结构。`--compact` 模式按文件分组，便于智能体遍历。
- **优雅降级** — 部分解析时尽量提取可用内容。解析失败的文件会输出警告并跳过。跨文件引用通过名称匹配解析，并降低置信度。
- **持久化 + 查询** — 索引一次，多次查询。生产环境使用 SQLite，调试时使用 JSON。

## 开发

```bash
cargo build                    # 构建
cargo test                     # 运行所有测试（79 个测试）
cargo clippy                   # 代码检查
cargo fmt                      # 代码格式化
cargo run -- analyze src/      # 对自身源码运行
cargo run -- index .           # 索引本项目
```

## 许可证

MIT
