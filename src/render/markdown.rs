//! Markdown projection.
//!
//! Markdown is deliberately lossy: it carries the structure and the formatting
//! that Markdown can express, and drops the rest unless `rich` is enabled, in
//! which case unsupported formatting is emitted as inline HTML.

use crate::model::{
    Block, Document, ImageRun, NumRef, Paragraph, Run, RunProps, Scalar, Section, SectionKind,
    Table, TableCell, TextBlock,
};

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Emit inline HTML for colour, font, size and underline.
    pub rich: bool,
    /// Link prefix for extracted media, e.g. `report.media/`.
    pub media_prefix: Option<String>,
}

pub fn render(document: &Document, options: &Options) -> String {
    let mut out = String::new();
    for (index, section) in document.sections.iter().enumerate() {
        out.push_str(&section_heading(section, index));
        match section.kind {
            SectionKind::Sheet => out.push_str(&render_sheet(document, section)),
            _ => {
                for block in &section.blocks {
                    out.push_str(&render_block(document, block, options));
                }
                if !section.notes.is_empty() {
                    out.push_str("**Notes**\n\n");
                    for block in &section.notes {
                        out.push_str(&render_block(document, block, options));
                    }
                }
            }
        }
    }
    out
}

fn section_heading(section: &Section, index: usize) -> String {
    let fallback = match section.kind {
        SectionKind::Sheet => format!("Sheet {}", index + 1),
        SectionKind::Slide => format!("Slide {}", index + 1),
        SectionKind::Page => format!("Page {}", index + 1),
        SectionKind::Body => return String::new(),
    };
    let name = section.name.as_deref().unwrap_or(&fallback);
    format!("## {name}\n\n")
}

fn render_block(document: &Document, block: &Block, options: &Options) -> String {
    match block {
        Block::Paragraph(paragraph) => render_paragraph(document, paragraph, options),
        Block::Table(table) => render_table(document, table, options),
        Block::Shape(shape) => {
            let mut out = String::new();
            for paragraph in &shape.paragraphs {
                out.push_str(&render_paragraph(document, paragraph, options));
            }
            if let Some(image) = &shape.image {
                out.push_str(&render_image(document, image, options));
                out.push_str("\n\n");
            }
            if let Some(table) = &shape.table {
                out.push_str(&render_table(document, table, options));
            }
            for child in &shape.children {
                out.push_str(&render_block(
                    document,
                    &Block::Shape(child.clone()),
                    options,
                ));
            }
            out
        }
        Block::TextBlock(text_block) => render_text_block(document, text_block, options),
        Block::Row(_) => String::new(),
    }
}

fn render_paragraph(document: &Document, paragraph: &Paragraph, options: &Options) -> String {
    let heading = document.heading_level(paragraph.style.as_deref(), paragraph.direct.as_ref());
    let text = render_runs(document, paragraph, options, heading.is_some());
    if text.trim().is_empty() {
        return String::new();
    }

    if let Some(level) = heading {
        return format!("{} {text}\n\n", "#".repeat(level as usize));
    }

    let properties = document.para_effective(paragraph.style.as_deref(), paragraph.direct.as_ref());
    if let Some(num) = &properties.num {
        let indent = "  ".repeat(num.level as usize);
        return format!("{indent}{}{text}\n\n", list_marker(document, num));
    }

    format!("{text}\n\n")
}

fn render_runs(
    document: &Document,
    paragraph: &Paragraph,
    options: &Options,
    in_heading: bool,
) -> String {
    let mut out = String::new();
    for run in &paragraph.runs {
        match run {
            Run::Text(text) => {
                if text.text.is_empty() {
                    continue;
                }
                let properties = document.run_effective(
                    paragraph.style.as_deref(),
                    text.style.as_deref(),
                    text.direct.as_ref(),
                );
                let mut rendered = format_text(&text.text, &properties, options, in_heading);
                if let Some(link) = &text.link {
                    rendered = format!("[{rendered}](<{link}>)");
                }
                out.push_str(&rendered);
            }
            Run::Image(image) => out.push_str(&render_image(document, image, options)),
            Run::Tab => out.push('\t'),
            Run::Break(break_run) => {
                if break_run.kind == "page" {
                    out.push_str("\n\n");
                } else {
                    out.push_str("  \n");
                }
            }
        }
    }
    out
}

fn render_text_block(document: &Document, block: &TextBlock, options: &Options) -> String {
    let mut out = String::new();
    for run in &block.runs {
        if let Run::Text(text) = run {
            let properties =
                document.run_effective(None, text.style.as_deref(), text.direct.as_ref());
            out.push_str(&format_text(&text.text, &properties, options, false));
        }
    }
    if out.trim().is_empty() {
        String::new()
    } else {
        format!("{out}\n\n")
    }
}

fn list_marker(document: &Document, num: &NumRef) -> String {
    let key = format!("{}:{}", num.id, num.level);
    match document.numbering.get(&key) {
        Some(level) if level.format == "bullet" => "- ".to_string(),
        Some(_) => "1. ".to_string(),
        None => "- ".to_string(),
    }
}

fn format_text(text: &str, properties: &RunProps, options: &Options, in_heading: bool) -> String {
    let mut value = escape(text);
    if properties.bold == Some(true) && !in_heading {
        value = format!("**{value}**");
    }
    if properties.italic == Some(true) {
        value = format!("*{value}*");
    }
    if properties.strike == Some(true) {
        value = format!("~~{value}~~");
    }

    if options.rich {
        if let Some(underline) = &properties.underline {
            if underline != "none" {
                value = format!("<u>{value}</u>");
            }
        }
        match properties.vert_align.as_deref() {
            Some("superscript") => value = format!("<sup>{value}</sup>"),
            Some("subscript") => value = format!("<sub>{value}</sub>"),
            _ => {}
        }
        let mut css: Vec<String> = Vec::new();
        if let Some(color) = &properties.color {
            if color != "auto" {
                css.push(format!("color:{color}"));
            }
        }
        if let Some(size) = properties.size {
            css.push(format!("font-size:{}pt", points(size)));
        }
        if let Some(font) = &properties.font {
            css.push(format!("font-family:'{font}'"));
        }
        if let Some(highlight) = &properties.highlight {
            if highlight.starts_with('#') {
                css.push(format!("background-color:{highlight}"));
            }
        }
        if !css.is_empty() {
            value = format!("<span style=\"{}\">{value}</span>", css.join(";"));
        }
    }

    value
}

fn render_image(document: &Document, image: &ImageRun, options: &Options) -> String {
    let alt = escape(image.alt.as_deref().unwrap_or("image"));
    let media = document.media.get(&image.sha256);

    if let Some(data) = media.and_then(|media| media.data.as_deref()) {
        let mime = media
            .map(|media| media.mime.as_str())
            .unwrap_or("application/octet-stream");
        return format!("![{alt}](<data:{mime};base64,{data}>)");
    }

    match options.media_prefix.as_deref() {
        Some(prefix) => {
            let extension = media
                .map(|media| extension_for_mime(&media.mime))
                .unwrap_or("bin");
            let url = format!(
                "{}/{}.{}",
                prefix.trim_end_matches('/'),
                image.sha256,
                extension
            );
            format!("![{alt}](<{url}>)")
        }
        None => format!("[image: {alt}]"),
    }
}

fn render_table(document: &Document, table: &Table, options: &Options) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in &table.rows {
        let mut cells = Vec::new();
        for cell in &row.cells {
            let continuation = cell
                .direct
                .as_ref()
                .and_then(|direct| direct.v_merge.as_deref())
                == Some("continue");
            let text = if continuation {
                String::new()
            } else {
                render_cell(document, cell, options)
            };
            cells.push(text);
            let span = cell
                .direct
                .as_ref()
                .and_then(|direct| direct.col_span)
                .unwrap_or(1)
                .max(1);
            for _ in 1..span {
                cells.push(String::new());
            }
        }
        rows.push(cells);
    }
    table_markdown(rows)
}

fn render_cell(document: &Document, cell: &TableCell, options: &Options) -> String {
    let mut parts = Vec::new();
    for block in &cell.blocks {
        if let Block::Paragraph(paragraph) = block {
            let text = render_runs(document, paragraph, options, false);
            if !text.trim().is_empty() {
                parts.push(text.replace("  \n", "<br>").replace('\n', "<br>"));
            }
        }
    }
    parts.join("<br>")
}

fn render_sheet(_document: &Document, section: &Section) -> String {
    let rows: Vec<&crate::model::Row> = section
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Row(row) => Some(row),
            _ => None,
        })
        .collect();
    if rows.is_empty() {
        return String::new();
    }

    let columns = rows
        .iter()
        .flat_map(|row| row.cells.iter().map(|cell| cell.col))
        .max()
        .map(|max| max as usize)
        .unwrap_or(0)
        .max(1);

    let mut table = Vec::new();
    for row in rows {
        let mut cells = vec![String::new(); columns];
        for cell in &row.cells {
            if let Some(slot) = cells.get_mut((cell.col as usize).saturating_sub(1)) {
                *slot = render_scalar(cell);
            }
        }
        table.push(cells);
    }
    table_markdown(table)
}

fn render_scalar(cell: &crate::model::Cell) -> String {
    match &cell.value {
        Some(Scalar::Number(value)) => format_number(*value),
        Some(Scalar::Text(text)) => escape(text),
        Some(Scalar::Bool(value)) => {
            if *value {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        None => match &cell.formula {
            Some(formula) => format!("`={formula}`"),
            None => String::new(),
        },
    }
}

fn table_markdown(mut rows: Vec<Vec<String>>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    for row in &mut rows {
        row.resize(columns, String::new());
    }

    let mut out = String::new();
    out.push_str(&format!("| {} |\n", rows[0].join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        (0..columns).map(|_| "---").collect::<Vec<_>>().join(" | ")
    ));
    for row in &rows[1..] {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
    out
}

fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else if value.is_finite() {
        format!("{value}")
    } else {
        String::new()
    }
}

fn points(centipoints: i64) -> String {
    let value = centipoints as f64 / 100.0;
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

pub fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        "image/emf" => "emf",
        "image/wmf" => "wmf",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        _ => "bin",
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' => {
                out.push('\\');
                out.push(character);
            }
            '\n' | '\r' | '\t' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn sample() -> Document {
        let mut document = Document::new(Format::Docx);
        document.styles.insert(
            "Heading1".into(),
            Style {
                kind: StyleKind::Paragraph,
                name: Some("heading 1".into()),
                run: Some(RunProps {
                    bold: Some(true),
                    size: Some(1600),
                    ..RunProps::default()
                }),
                para: Some(ParaProps {
                    outline: Some(0),
                    ..ParaProps::default()
                }),
                ..Style::new(StyleKind::Paragraph)
            },
        );
        document.numbering.insert(
            "1:0".into(),
            NumberingLevel {
                format: "bullet".into(),
                text: "•".into(),
                start: Some(1),
                indent: None,
                hanging: None,
            },
        );
        document.media.insert(
            "abc123".into(),
            Media {
                mime: "image/png".into(),
                bytes: 10,
                width: Some(10),
                height: Some(20),
                data: None,
            },
        );

        document.sections.push(Section {
            kind: SectionKind::Body,
            name: None,
            hidden: None,
            page: None,
            cols: Vec::new(),
            merges: Vec::new(),
            blocks: vec![
                Block::Paragraph(Paragraph {
                    style: Some("Heading1".into()),
                    runs: vec![Run::Text(TextRun {
                        text: "Title".into(),
                        ..TextRun::default()
                    })],
                    ..Paragraph::default()
                }),
                Block::Paragraph(Paragraph {
                    runs: vec![
                        Run::Text(TextRun {
                            text: "bold".into(),
                            direct: Some(RunProps {
                                bold: Some(true),
                                ..RunProps::default()
                            }),
                            ..TextRun::default()
                        }),
                        Run::Text(TextRun {
                            text: " plain".into(),
                            ..TextRun::default()
                        }),
                    ],
                    ..Paragraph::default()
                }),
                Block::Paragraph(Paragraph {
                    direct: Some(ParaProps {
                        num: Some(NumRef {
                            id: "1".into(),
                            level: 0,
                        }),
                        ..ParaProps::default()
                    }),
                    runs: vec![Run::Text(TextRun {
                        text: "item".into(),
                        ..TextRun::default()
                    })],
                    ..Paragraph::default()
                }),
                Block::Paragraph(Paragraph {
                    runs: vec![Run::Image(ImageRun {
                        sha256: "abc123".into(),
                        alt: Some("logo".into()),
                        name: None,
                        width: None,
                        height: None,
                        anchor: "inline".into(),
                        x: None,
                        y: None,
                    })],
                    ..Paragraph::default()
                }),
                Block::Table(Table {
                    style: None,
                    grid: Vec::new(),
                    rows: vec![
                        TableRow {
                            direct: None,
                            cells: vec![
                                TableCell {
                                    direct: None,
                                    blocks: vec![Block::Paragraph(Paragraph {
                                        runs: vec![Run::Text(TextRun {
                                            text: "a".into(),
                                            ..TextRun::default()
                                        })],
                                        ..Paragraph::default()
                                    })],
                                },
                                TableCell {
                                    direct: None,
                                    blocks: vec![Block::Paragraph(Paragraph {
                                        runs: vec![Run::Text(TextRun {
                                            text: "b | c".into(),
                                            ..TextRun::default()
                                        })],
                                        ..Paragraph::default()
                                    })],
                                },
                            ],
                        },
                        TableRow {
                            direct: None,
                            cells: vec![
                                TableCell {
                                    direct: None,
                                    blocks: vec![Block::Paragraph(Paragraph {
                                        runs: vec![Run::Text(TextRun {
                                            text: "d".into(),
                                            ..TextRun::default()
                                        })],
                                        ..Paragraph::default()
                                    })],
                                },
                                TableCell {
                                    direct: None,
                                    blocks: vec![Block::Paragraph(Paragraph {
                                        runs: vec![Run::Text(TextRun {
                                            text: "e".into(),
                                            ..TextRun::default()
                                        })],
                                        ..Paragraph::default()
                                    })],
                                },
                            ],
                        },
                    ],
                }),
            ],
            notes: Vec::new(),
        });
        document
    }

    #[test]
    fn renders_headings_emphasis_lists_images_and_tables() {
        let options = Options {
            rich: false,
            media_prefix: Some("media/".into()),
        };
        let text = render(&sample(), &options);
        assert!(text.contains("# Title"), "{text}");
        assert!(text.contains("**bold** plain"), "{text}");
        assert!(text.contains("- item"), "{text}");
        assert!(text.contains("![logo](<media/abc123.png>)"), "{text}");
        assert!(text.contains("| a | b \\| c |"), "{text}");
        assert!(text.contains("| --- | --- |"), "{text}");
        assert!(text.contains("| d | e |"), "{text}");
    }

    #[test]
    fn rich_mode_emits_inline_html_for_unexpressible_formatting() {
        let mut document = sample();
        document.sections[0].blocks = vec![Block::Paragraph(Paragraph {
            runs: vec![Run::Text(TextRun {
                text: "colored".into(),
                direct: Some(RunProps {
                    color: Some("#FF0000".into()),
                    size: Some(1600),
                    underline: Some("single".into()),
                    ..RunProps::default()
                }),
                ..TextRun::default()
            })],
            ..Paragraph::default()
        })];

        let plain = render(&document, &Options::default());
        assert!(!plain.contains("<span"), "{plain}");

        let rich = render(
            &document,
            &Options {
                rich: true,
                media_prefix: None,
            },
        );
        assert!(rich.contains("<u>"), "{rich}");
        assert!(rich.contains("color:#FF0000"), "{rich}");
        assert!(rich.contains("font-size:16pt"), "{rich}");
    }

    #[test]
    fn renders_sheets_as_tables() {
        let mut document = Document::new(Format::Xlsx);
        document.sections.push(Section {
            kind: SectionKind::Sheet,
            name: Some("Data".into()),
            hidden: None,
            page: None,
            cols: Vec::new(),
            merges: Vec::new(),
            blocks: vec![
                Block::Row(crate::model::Row {
                    index: 1,
                    height: None,
                    hidden: None,
                    cells: vec![
                        crate::model::Cell {
                            col: 1,
                            value: Some(Scalar::Text("name".into())),
                            style: None,
                            formula: None,
                        },
                        crate::model::Cell {
                            col: 2,
                            value: Some(Scalar::Text("value".into())),
                            style: None,
                            formula: None,
                        },
                    ],
                }),
                Block::Row(crate::model::Row {
                    index: 2,
                    height: None,
                    hidden: None,
                    cells: vec![
                        crate::model::Cell {
                            col: 1,
                            value: Some(Scalar::Text("a".into())),
                            style: None,
                            formula: None,
                        },
                        crate::model::Cell {
                            col: 2,
                            value: Some(Scalar::Number(42.0)),
                            style: None,
                            formula: None,
                        },
                    ],
                }),
            ],
            notes: Vec::new(),
        });

        let text = render(&document, &Options::default());
        assert!(text.contains("## Data"), "{text}");
        assert!(text.contains("| name | value |"), "{text}");
        assert!(text.contains("| a | 42 |"), "{text}");
    }
}
