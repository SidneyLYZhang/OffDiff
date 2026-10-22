//! Canonical document IR.
//!
//! Every projection (JSON artifact, Markdown projection, Diff view) is rendered
//! from this model. Lengths are stored in hundredths of a point so that twips,
//! half-points, EMU and PDF points can be compared without floating point noise.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Schema identifier written into every artifact.
pub const SCHEMA: &str = "offdiff/1";

/// Length in hundredths of a point (1/100 pt).
///
/// Twips are multiplied by 5, half-points by 50, EMU are divided by 127.
pub type Cp = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Docx,
    Xlsx,
    Pptx,
    Pdf,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Xlsx => "xlsx",
            Format::Pptx => "pptx",
            Format::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Properties::is_empty")]
    pub properties: Properties,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<Defaults>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub styles: BTreeMap<String, Style>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub numbering: BTreeMap<String, NumberingLevel>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub media: BTreeMap<String, Media>,
    pub sections: Vec<Section>,
}

impl Document {
    pub fn new(format: Format) -> Self {
        Document {
            schema: SCHEMA.to_string(),
            format,
            properties: Properties::default(),
            defaults: None,
            styles: BTreeMap::new(),
            numbering: BTreeMap::new(),
            media: BTreeMap::new(),
            sections: Vec::new(),
        }
    }

    pub fn style_name<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        self.styles
            .get(id)
            .and_then(|style| style.name.as_deref())
            .or(Some(id))
    }

    /// Effective paragraph properties: document defaults, the style chain
    /// (already resolved), then direct formatting.
    pub fn para_effective(&self, style: Option<&str>, direct: Option<&ParaProps>) -> ParaProps {
        let defaults = self.defaults.as_ref();
        let mut props = defaults
            .and_then(|defaults| defaults.para.clone())
            .unwrap_or_default();
        let id = style.or_else(|| defaults.and_then(|defaults| defaults.paragraph.as_deref()));
        if let Some(style) = id.and_then(|id| self.styles.get(id)) {
            if let Some(para) = &style.para {
                props = props.merged(para);
            }
        }
        if let Some(direct) = direct {
            props = props.merged(direct);
        }
        props
    }

    /// Effective run properties: defaults, paragraph style, character style,
    /// then direct formatting.
    pub fn run_effective(
        &self,
        para_style: Option<&str>,
        run_style: Option<&str>,
        direct: Option<&RunProps>,
    ) -> RunProps {
        let defaults = self.defaults.as_ref();
        let mut props = defaults
            .and_then(|defaults| defaults.run.clone())
            .unwrap_or_default();
        let paragraph_id =
            para_style.or_else(|| defaults.and_then(|defaults| defaults.paragraph.as_deref()));
        if let Some(style) = paragraph_id.and_then(|id| self.styles.get(id)) {
            if let Some(run) = &style.run {
                props = props.merged(run);
            }
        }
        let character_id =
            run_style.or_else(|| defaults.and_then(|defaults| defaults.character.as_deref()));
        if let Some(style) = character_id.and_then(|id| self.styles.get(id)) {
            if let Some(run) = &style.run {
                props = props.merged(run);
            }
        }
        if let Some(direct) = direct {
            props = props.merged(direct);
        }
        props
    }

    /// Heading level (1-based) from `outlineLvl` or a `heading N` style name.
    pub fn heading_level(&self, style: Option<&str>, direct: Option<&ParaProps>) -> Option<u8> {
        let para = self.para_effective(style, direct);
        if let Some(outline) = para.outline {
            return Some(outline.saturating_add(1).min(9));
        }
        let id = style?;
        let name = self.style_name(id).unwrap_or(id).to_ascii_lowercase();
        for prefix in ["heading ", "heading"] {
            if let Some(rest) = name.strip_prefix(prefix) {
                if let Ok(level) = rest.trim().parse::<u8>() {
                    if (1..=9).contains(&level) {
                        return Some(level);
                    }
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Properties {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stable: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub volatile: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unsupported: BTreeSet<String>,
}

impl Properties {
    pub fn is_empty(&self) -> bool {
        self.stable.is_empty() && self.volatile.is_empty() && self.unsupported.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paragraph: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// `w:docDefaults/w:rPrDefault`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunProps>,
    /// `w:docDefaults/w:pPrDefault`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub para: Option<ParaProps>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StyleKind {
    Paragraph,
    Character,
    Table,
    Cell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub kind: StyleKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub based_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunProps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub para: Option<ParaProps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellProps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xf: Option<XfProps>,
}

impl Style {
    pub fn new(kind: StyleKind) -> Self {
        Style {
            kind,
            name: None,
            based_on: None,
            run: None,
            para: None,
            cell: None,
            xf: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// `single`, `double`, `none`, `dotted`, ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    /// `superscript`, `subscript`, `baseline`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vert_align: Option<String>,
    /// Font size, hundredths of a point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Cp>,
    /// `#RRGGBB` or `auto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_east_asian: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<String>,
    /// Letter spacing, hundredths of a point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spacing: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caps: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_caps: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

impl RunProps {
    pub fn is_empty(&self) -> bool {
        *self == RunProps::default()
    }

    /// Merge `other` over `self`, returning the combined properties.
    pub fn merged(mut self, other: &RunProps) -> RunProps {
        macro_rules! pick {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field = other.$field.clone();
                }
            };
        }
        pick!(bold);
        pick!(italic);
        pick!(underline);
        pick!(strike);
        pick!(vert_align);
        pick!(size);
        pick!(color);
        pick!(font);
        pick!(font_east_asian);
        pick!(highlight);
        pick!(spacing);
        pick!(caps);
        pick!(small_caps);
        pick!(hidden);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParaProps {
    /// `left`, `center`, `right`, `justify`, `distribute`, `start`, `end`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent: Option<Indent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spacing: Option<Spacing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num: Option<NumRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_break_before: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_next: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<u8>,
    /// Outline level for PresentationML paragraphs (0-8). Not a heading level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// `#RRGGBB` paragraph shading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shading: Option<String>,
}

impl ParaProps {
    pub fn is_empty(&self) -> bool {
        *self == ParaProps::default()
    }

    pub fn merged(mut self, other: &ParaProps) -> ParaProps {
        if other.align.is_some() {
            self.align = other.align.clone();
        }
        if other.indent.is_some() {
            self.indent = other.indent.clone();
        }
        if other.spacing.is_some() {
            self.spacing = other.spacing.clone();
        }
        if other.num.is_some() {
            self.num = other.num.clone();
        }
        if other.page_break_before.is_some() {
            self.page_break_before = other.page_break_before;
        }
        if other.keep_next.is_some() {
            self.keep_next = other.keep_next;
        }
        if other.outline.is_some() {
            self.outline = other.outline;
        }
        if other.level.is_some() {
            self.level = other.level;
        }
        if other.shading.is_some() {
            self.shading = other.shading.clone();
        }
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Indent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_line: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hanging: Option<Cp>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spacing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<Cp>,
    /// `auto`, `exact`, `at-least`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_rule: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumRef {
    pub id: String,
    pub level: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col_span: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_span: Option<u32>,
    /// `restart` or `continue`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v_merge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v_align: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XfProps {
    /// Number format code, e.g. `0.00`, `yyyy-mm-dd`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_fmt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<XfFont>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<XfBorder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<XfAlign>,
}

impl XfProps {
    pub fn is_empty(&self) -> bool {
        *self == XfProps::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XfFont {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XfBorder {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct XfAlign {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberingLevel {
    /// `bullet`, `decimal`, `lowerLetter`, ...
    pub format: String,
    /// Level text, e.g. `%1.` or a bullet glyph.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hanging: Option<Cp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub mime: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Base64 payload, only populated by `export --media embed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    Body,
    Sheet,
    Slide,
    Page,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSetup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<Cp>,
    /// `portrait` or `landscape`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    /// Clockwise page rotation in degrees (PDF `/Rotate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<Margins>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Margins {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<Cp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRange {
    /// 1-based, as Excel displays it.
    pub r1: u32,
    pub r2: u32,
    pub c1: u32,
    pub c2: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub kind: SectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageSetup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cols: Vec<ColProps>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<MergeRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Block>,
    /// Speaker notes attached to this section (slides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Block>,
}

/// Column range properties for spreadsheets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColProps {
    /// First column, 1-based as Excel displays it.
    pub index: u32,
    /// Number of columns in the range.
    pub span: u32,
    /// Raw OOXML width in character units, kept verbatim so it never depends on
    /// font metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    Row(Row),
    Shape(Shape),
    TextBlock(TextBlock),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct: Option<ParaProps>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Run {
    Text(TextRun),
    Image(ImageRun),
    Tab,
    Break(BreakRun),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct: Option<RunProps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRun {
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<Cp>,
    /// `inline` or `float`.
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<Cp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakRun {
    /// `line`, `page` or `column`.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grid: Vec<Cp>,
    pub rows: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct: Option<RowProps>,
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct: Option<CellProps>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// 1-based row number, as Excel displays it.
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<Cp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// 1-based column number, as Excel displays it.
    pub col: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Scalar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Number(f64),
    Text(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// OOXML shape kind: `sp`, `pic`, `graphicFrame`, `grpSp`, `cxnSp`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraphs: Vec<Paragraph>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageRun>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<Table>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Shape>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: Cp,
    pub y: Cp,
    pub w: Cp,
    pub h: Cp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> Document {
        let mut doc = Document::new(Format::Docx);
        doc.properties
            .stable
            .insert("title".into(), "Sample".into());
        doc.styles.insert(
            "Heading1".into(),
            Style {
                kind: StyleKind::Paragraph,
                name: Some("heading 1".into()),
                based_on: Some("Normal".into()),
                run: Some(RunProps {
                    bold: Some(true),
                    size: Some(3200),
                    ..RunProps::default()
                }),
                ..Style::new(StyleKind::Paragraph)
            },
        );
        doc.sections.push(Section {
            kind: SectionKind::Body,
            name: None,
            hidden: None,
            page: None,
            cols: Vec::new(),
            merges: Vec::new(),
            blocks: vec![
                Block::Paragraph(Paragraph {
                    style: Some("Heading1".into()),
                    direct: None,
                    runs: vec![Run::Text(TextRun {
                        text: "Hello\nworld".into(),
                        ..TextRun::default()
                    })],
                }),
                Block::Table(Table {
                    style: None,
                    grid: vec![5000],
                    rows: vec![TableRow {
                        direct: None,
                        cells: vec![TableCell {
                            direct: None,
                            blocks: vec![Block::Paragraph(Paragraph::default())],
                        }],
                    }],
                }),
            ],
            notes: Vec::new(),
        });
        doc
    }

    #[test]
    fn roundtrips_through_json() {
        let doc = sample_document();
        let json = serde_json::to_string(&doc).expect("serialize");
        let parsed: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, parsed);
    }

    #[test]
    fn block_serializes_to_one_line() {
        let block = Block::Paragraph(Paragraph {
            style: None,
            direct: None,
            runs: vec![Run::Text(TextRun {
                text: "line one\nline two\ttab".into(),
                ..TextRun::default()
            })],
        });
        let line = serde_json::to_string(&block).expect("serialize");
        assert!(!line.contains('\n'), "block must stay on one line: {line}");
        assert!(line.contains("\\n"));
        assert!(line.contains("\\t"));
    }

    #[test]
    fn props_merge_overrides_only_present_fields() {
        let base = RunProps {
            bold: Some(true),
            size: Some(2000),
            ..RunProps::default()
        };
        let over = RunProps {
            bold: Some(false),
            ..RunProps::default()
        };
        let merged = base.merged(&over);
        assert_eq!(merged.bold, Some(false));
        assert_eq!(merged.size, Some(2000));
    }

    #[test]
    fn schema_is_stable() {
        assert_eq!(SCHEMA, "offdiff/1");
        assert_eq!(Format::Docx.as_str(), "docx");
    }
}
