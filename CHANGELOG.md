# Changelog

本项目的所有重要变更都记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

<!-- 下一个版本的变更写在这里。 -->

## [0.4.0] - 2026-10-22

> 0.4.0 是本次首个公开发布版本；0.1.0–0.3.0 没有对应的 tag 或发布记录，因此不单独列出。
> 需要 Rust 1.89 或更高版本。

### Added

- **统一的文档 IR**：`docx`、`xlsx`、`pptx`、`pdf` 解析为同一份结构化表示，再投影为 JSON、Markdown 和 git diff 视图；JSON schema 为 `offdiff/1`，每个语义单元固定一行。
- **DOCX**：解析 `docDefaults` → 样式继承链 → 主题 → 编号之后的**有效属性**；支持段落与 run 格式、表格（合并/底色/列宽）、图片（内联与浮动，按 SHA-256 标识）、超链接、页面设置和文档属性；自动裁剪未使用样式，未建模特性写入 `unsupported`。
- **XLSX**：支持工作表名称/顺序/隐藏、单元格值与公式（含缓存值）、单元格样式（字体/填充/边框/对齐/数字格式）、合并单元格、行高列宽与隐藏、日期按数字格式转换为 ISO 字符串。
- **PPTX**：支持幻灯片顺序/尺寸/隐藏、形状类型与名称/位置/尺寸、文本 run 格式、组合形状、表格、图片和备注。
- **PDF**：基于 `unpdf` 提取页尺寸/旋转、文本 run（字体/字号/颜色/粗斜体/上下标）、标题层级、列表、表格、图片和元数据；标题与表格为版面推断结果。
- **CLI**：新增 `export`（`--format json|md|both`、`--media none|copy|embed`、`--pretty`、`--rich`）、`textconv` 和 `git-init` 三个子命令。
- **Git 集成**：`git-init` 幂等写入 `.gitattributes` 与 `diff.offdiff.textconv` / `binary` / `cachetextconv`；`textconv` 输出剥离易变属性，坏文件输出稳定错误占位，**永不阻断 `git diff`**。
- **媒体处理**：图片按原始字节的 SHA-256 内容寻址，支持按需导出到 `<stem>.media/`，或使用 `--media embed` 内联为 base64。
- **确定性输出**：键按字典序、UTF-8 无 BOM、LF 换行、无绝对路径与时间戳；同一输入产生逐字节相同的 Diff 视图。
- **发布自动化**：GitHub Actions 在推送 `v*` tag 时验证代码、构建 Windows/Linux/macOS 产物、发布 GitHub Release，并发布到 crates.io。

[Unreleased]: https://github.com/SidneyLYZhang/OffDiff/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/SidneyLYZhang/OffDiff/releases/tag/v0.4.0
