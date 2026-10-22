# OffDiff

> 让 Office 文档和 PDF 在 git 里像文本一样被评审，格式变化也看得见。

OffDiff 把 `.docx` / `.xlsx` / `.pptx` / `.pdf` 提取成一份确定性的结构化表示（Document IR），再投影成三种产物：

- **JSON 产物**：无损，供程序消费与归档；
- **Markdown 投影**：有损，供人阅读；
- **Diff 视图**：给 `git diff` 用的行导向 JSON，剥离易变属性。

同一份 IR 只解析一次源文件；JSON、Markdown 与 diff 视图都是它的投影，不会各自漂移。

## 为什么需要它

git 默认把 Office 文件当二进制：`git diff report.docx` 只会告诉你 `Binary files differ`。把文档转成纯文本可以解决"内容"的 diff，但字号、颜色、对齐、表格结构、图片替换这些变化会全部消失。

OffDiff 的取舍是：

- 用**有效属性**（解析 `docDefaults` → 样式继承链 → 主题 → 编号之后真正生效的值）判断格式是否变化；
- 用**样式表 + 引用**而不是把样式展开到每个段落，改一个样式定义只产生一处 diff；
- **每个语义单元固定一行**，让行级 diff 稳定、hunk 小；
- **易变属性**（修改时间、修订号、生成软件版本）不进入 diff 视图，保存文件本身不产生噪音。

## 支持状态

| 格式 | 状态 | 说明 |
| --- | --- | --- |
| `.docx` | 可用 | 段落、run 格式、表格、图片、超链接、编号、页面设置、元数据 |
| `.xlsx` | 可用 | 工作表、单元格值与公式、样式表、合并单元格、行列尺寸、隐藏、日期格式 |
| `.pptx` | 可用 | 幻灯片、形状与位置、文本与 run 格式、表格、图片、备注、隐藏页 |
| `.pdf` | 可用 | 页、文本 run（字体/字号/颜色）、标题与列表、表格、图片、元数据；基于 `unpdf` |

旧二进制格式 `.doc` / `.xls` / `.ppt` 明确不支持。

## 安装

需要 Rust 工具链：

```bash
cargo build --release
```

首次构建需要能访问 crates.io 拉取 `unpdf` 及其依赖（或这些依赖已经在本机 cargo 缓存里）。产物在 `target/release/offdiff`。

如果依赖已经在本机 cargo 缓存里，也可以离线构建：

```bash
cargo build --release --offline
```

## 用法

### 1. 导出

导出 JSON 与 Markdown 到目录（图片另存为 `<stem>.media/`）：

```bash
offdiff export report.docx --format both --out-dir out --media copy
```

```
out/
├── report.json
├── report.md
└── report.media/
    └── <sha256>.png
```

常用选项：

| 选项 | 含义 |
| --- | --- |
| `--format json\|md\|both` | 选择投影；默认 `json` |
| `--out-dir DIR` | 输出目录；不给且只选一种文本格式时写 stdout |
| `--media none\|copy\|embed` | 不导出图片 / 另存为文件 / base64 内联进 JSON 与 Markdown |
| `--pretty` | JSON 缩进输出，便于人读；默认每个语义单元一行（更适合 diff） |
| `--rich` | Markdown 用内联 HTML 表达颜色、字体、字号、下划线 |

单独输出到 stdout：

```bash
offdiff export report.docx --format md
```

### 2. git 集成（textconv）

在仓库根目录运行：

```bash
offdiff git-init --bin /absolute/path/to/offdiff
```

它会幂等地：

1. 在 `.gitattributes` 追加：

```gitattributes
*.docx diff=offdiff
*.xlsx diff=offdiff
*.pptx diff=offdiff
*.pdf diff=offdiff
```

2. 写入仓库本地 git 配置：

```bash
git config diff.offdiff.textconv "offdiff textconv --tool-version <版本>"
git config diff.offdiff.binary true
git config diff.offdiff.cachetextconv true
```

之后 `git diff`、`git log -p`、`git show` 会直接显示结构化差异。比如把一段文字居中改成右对齐，diff 是：

```diff
-{"type":"paragraph","direct":{"align":"center"},"runs":[{"type":"text","text":"OffDiff 测试文档"}]},
+{"type":"paragraph","direct":{"align":"right"},"runs":[{"type":"text","text":"OffDiff 测试文档"}]},
```

全局安装：

```bash
offdiff git-init --global --bin /absolute/path/to/offdiff
```

它会写全局 git 配置，并把规则追加到 `core.attributesFile`（未设置时使用 `~/.config/git/attributes`）。

**已知边界**（由 git 本身决定）：

- `git format-patch` 永不包含 textconv 输出；
- `git diff --stat` / `--numstat` 不跑转换；
- 三方合并不使用 textconv；
- GitHub / GitLab 的 PR 页面不会执行你本地的 textconv。托管评审需要提交 `offdiff export` 的产物，或在 CI 里生成差异评论。

### 3. 不经过 git 直接看 diff 视图

```bash
offdiff textconv report.docx
```

输出是合法 JSON，但每个段落/行/形状占一行。`--include-volatile` 可以保留时间戳等易变属性。

两个文件直接比较：

```bash
offdiff textconv a.docx > a.json
offdiff textconv b.docx > b.json
diff -u a.json b.json
```

## JSON 产物结构

```json
{
"schema":"offdiff/1",
"format":"docx",
"properties":{"stable":{"author":"Ada"},"volatile":{"modified":"2026-10-21T00:00:00Z"}},
"defaults":{"paragraph":"Normal"},
"styles":{
"Normal":{"kind":"paragraph","name":"Normal"},
"Heading1":{"kind":"paragraph","name":"heading 1","based_on":"Normal","run":{"bold":true,"size":1600},"para":{"outline":0}}
},
"numbering":{},
"media":{"<sha256>":{"mime":"image/png","bytes":125,"width":60,"height":40}},
"sections":[
{"kind":"body","blocks":[
{"type":"paragraph","style":"Heading1","runs":[{"type":"text","text":"Hello"}]},
{"type":"table","grid":[5000,5000],"rows":[{"cells":[{"blocks":[{"type":"paragraph"}]}]}]},
{"type":"paragraph","runs":[{"type":"image","sha256":"<sha256>","anchor":"inline"}]}
]}
]
}
```

约定：

- 长度单位统一为 **1/100 pt**（twips × 5、半磅 × 50、EMU ÷ 127）；
- `.xlsx` 的行号、列号是 **1-based**，与 Excel 显示一致；
- `.xlsx` 的列宽保留 OOXML 的字符单位原始值（如 `"12.5"`），不做像素换算，避免字体度量差异；
- `.xlsx` 的日期单元格按样式的数字格式转成 ISO 字符串（`YYYY-MM-DD` 或 `YYYY-MM-DDTHH:MM:SS`）；
- `.pdf` 的页面尺寸按 PDF point 换算为 1/100 pt，`page.rotation` 记录 `/Rotate`；
- 图片按 **原始字节的 SHA-256** 标识，JSON 只记哈希与元信息，不内联二进制（除非 `--media embed`）；
- `properties.stable` 是内容属性，`properties.volatile` 是易变属性，diff 视图不含后者；
- `properties.unsupported` 列出检测到但尚未建模的特性，便于判断保真损失。

## 保真边界

### docx

已建模：段落样式与直接格式、run 格式（粗体/斜体/下划线/删除线/上下标/字号/颜色/字体/高亮/字距）、表格（行列/合并/单元格宽度与底色）、图片（内联与浮动、位置与尺寸）、超链接、编号列表、页面设置、文档属性。

未建模（会在 `unsupported` 里标记）：页眉页脚、脚注尾注、批注、修订痕迹（显示为接受后状态）、域与目录、公式、图表/SmartArt/OLE 渲染、文本框、多节文档。

### xlsx

已建模：工作表名称/顺序/隐藏、单元格值（数字/文本/布尔/公式缓存值/日期）、单元格样式（字体/填充/边框/对齐/数字格式）、合并单元格、行高与隐藏、列宽与隐藏。

未建模：图表、图片、批注、数据验证、条件格式、透视表、定义名称、超链接。共享公式只记录主公式与缓存值，不做引用平移。

### pptx

已建模：幻灯片顺序/尺寸/隐藏、形状类型与名称/位置/尺寸、文本框段落与 run 格式、组合形状、表格、图片、备注。

未建模：图表、SmartArt、动画、切换、音视频；母版与版式继承只记录 layout 名称（当前未输出）；继承自版式的项目符号不会还原为 Markdown 列表，检测到显式项目符号或缩进级别时会在 `unsupported` 标记 `bullets`。

### pdf

已建模：页（尺寸、旋转）、段落与文本 run（字体、字号、颜色、粗斜体/下划线/上下标）、标题层级、列表、表格（由 `unpdf` 从版面推断）、图片（哈希、位置与尺寸）、文档元数据。

未建模（会在 `unsupported` 里标记）：书签大纲、表单字段、批注、扫描件的 OCR（`suppressed-ocr`）、无法解码的字体或内容流、旋转文本的阅读顺序、矢量图形与水平线。标题与表格是**版面推断**结果，不保证与 PDF 的语义结构一致；扫描件没有文本层时只会得到空页面。

## 确定性契约

Diff 视图与 JSON 产物必须满足：

- 相同输入产生**逐字节相同**的输出；
- 不包含绝对路径、临时文件名、主机名、时间戳（diff 视图）；
- 键按字典序输出（`BTreeMap` / `BTreeSet`）；
- UTF-8 无 BOM、LF 换行；
- 数字不使用浮点噪声表示（长度一律整数）。

Bad file 不会让 `git diff` 失败：`textconv` 永远以 0 退出，并输出一行稳定的错误占位：

```json
{"type":"error","error":"corrupt"}
```

错误类别：`missing`、`unsupported`、`corrupt`、`io_error`、`unavailable`。

## 开发

```bash
cargo test --offline
cargo clippy --offline --all-targets
```

`tests/golden/` 保存真实 docx 的 JSON / Markdown 快照，回归测试会逐字节比对。git textconv 的端到端测试默认跳过，需要 `git` 和能启动 shell 的终端：

```bash
cargo test --test git_textconv -- --ignored --nocapture
```

代码结构：

```
src/
├── model.rs        # Document IR
├── detect.rs       # 按内容嗅探格式（不依赖扩展名）
├── media.rs        # 媒体哈希、MIME、图片尺寸
├── ooxml/          # ZIP 包、XML 助手、单位换算、关系、元数据
├── docx/           # 样式继承 + 文档解析
├── xlsx/           # 样式表 + 工作表解析
├── pptx/           # 幻灯片/形状/表格/备注
├── render/         # json / markdown / diff 三投影
└── lib.rs          # 解析、导出、git-init
```

测试策略：单元测试内联构造最小 OOXML 包与最小 PDF；`examples/test.docx` 作为真实文档回归，真实论文 PDF 用于手工验证；`.xlsx` / `.pptx` 用 `openpyxl` / `python-pptx` 生成后验证。

## 路线图

- [ ] `offdiff diff a b`：语义级变更报告（当前先由 git 行 diff 承担）
- [ ] PDF：OCR、书签、表单字段
- [ ] xlsx：图片、图表、批注、条件格式
- [ ] pptx：图表、动画、母版/版式继承
- [ ] docx：页眉页脚、脚注、批注、修订痕迹
- [ ] 托管平台 CI：把结构化差异贴到 PR

## 许可证

MIT
