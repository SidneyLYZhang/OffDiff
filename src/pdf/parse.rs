//! PDF parsing on top of `unpdf`.
//!
//! `unpdf` already reconstructs paragraphs, lists, tables and text styles, so
//! this module maps its model onto the OffDiff IR instead of re-implementing
//! content-stream parsing.

use std::collections::{BTreeMap, BTreeSet};

use unpdf::model::{
    Alignment, Block as PdfBlock, InlineContent, ListStyle, Paragraph as PdfParagraph,
    Resource as PdfResource, Table as PdfTable, TextStyle as PdfTextStyle,
};
use unpdf::ParseOptions;

use crate::error::{OffDiffError, Result};
use crate::media::MediaStore;
use crate::model::{
    Block, BreakRun, CellProps, Document, Format, ImageRun, Indent, NumRef, NumberingLevel,
    PageSetup, ParaProps, Paragraph, RowProps, Run, RunProps, Section, SectionKind, Spacing, Table,
    TableCell, TableRow, TextRun,
};
use crate::ooxml::{normalize_hex, units};

pub fn parse(bytes: &[u8]) -> Result<(Document, BTreeMap<String, Vec<u8>>)> {
    let options = ParseOptions::new().with_resources(true).sequential();
    let parsed = unpdf::parse_bytes_with_options(bytes, options)
        .map_err(|error| OffDiffError::Corrupt(format!("PDF parsing failed: {error}")))?;

    let mut media = MediaStore::new();
    let mut unsupported = BTreeSet::new();
    let mut list_levels: BTreeSet<(String, u8)> = BTreeSet::new();
    let mut sections = Vec::new();

    for page in &parsed.pages {
        let resources: BTreeMap<&str, &PdfResource> = page
            .images
            .iter()
            .map(|(id, resource)| (id.as_str(), resource))
            .collect();

        let mut blocks = Vec::new();
        for element in &page.elements {
            match element {
                PdfBlock::Paragraph(paragraph) => {
                    blocks.push(Block::Paragraph(convert_paragraph(
                        paragraph,
                        &resources,
                        &mut media,
                        &mut list_levels,
                    )?));
                }
                PdfBlock::Table(table) => {
                    if let Some(caption) = &table.caption {
                        if !caption.trim().is_empty() {
                            blocks.push(Block::Paragraph(text_paragraph(caption)));
                        }
                    }
                    blocks.push(Block::Table(convert_table(
                        table,
                        &resources,
                        &mut media,
                        &mut list_levels,
                    )?));
                }
                PdfBlock::Image {
                    resource_id,
                    alt_text,
                    width,
                    height,
                    x,
                    y,
                } => {
                    if let Some(image) = image_run(
                        resource_id,
                        alt_text.as_deref(),
                        ImageGeometry {
                            width: *width,
                            height: *height,
                            x: *x,
                            y: *y,
                        },
                        &resources,
                        &mut media,
                    )? {
                        blocks.push(Block::Paragraph(Paragraph {
                            runs: vec![Run::Image(image)],
                            ..Paragraph::default()
                        }));
                    }
                }
                PdfBlock::HorizontalRule => {
                    unsupported.insert("horizontal-rules".to_string());
                }
                PdfBlock::PageBreak | PdfBlock::SectionBreak => {}
                PdfBlock::Raw { .. } => {
                    unsupported.insert("raw-content".to_string());
                }
            }
        }

        if page.suppressed_text_runs > 0 {
            unsupported.insert("unreadable-fonts".to_string());
        }
        if page.ocr_text_suppressed {
            unsupported.insert("suppressed-ocr".to_string());
        }
        if page.undecodable_content_streams > 0 {
            unsupported.insert("undecodable-streams".to_string());
        }
        if page.rotated_text_runs > 0 {
            unsupported.insert("rotated-text".to_string());
        }
        if page.unsupported_image_count > 0 {
            unsupported.insert("unsupported-images".to_string());
        }

        sections.push(Section {
            kind: SectionKind::Page,
            name: None,
            hidden: None,
            page: Some(PageSetup {
                width: Some(units::points(page.width as f64)),
                height: Some(units::points(page.height as f64)),
                orientation: None,
                rotation: (page.rotation != 0).then_some(page.rotation),
                margin: None,
            }),
            cols: Vec::new(),
            merges: Vec::new(),
            blocks,
            notes: Vec::new(),
        });
    }

    let metadata = &parsed.metadata;
    let mut stable = BTreeMap::new();
    let mut volatile = BTreeMap::new();
    insert_text(&mut stable, "title", metadata.title.as_deref());
    insert_text(&mut stable, "author", metadata.author.as_deref());
    insert_text(&mut stable, "subject", metadata.subject.as_deref());
    insert_text(&mut stable, "keywords", metadata.keywords.as_deref());
    insert_text(&mut stable, "creator", metadata.creator.as_deref());
    insert_text(
        &mut stable,
        "pdf_version",
        Some(metadata.pdf_version.as_str()),
    );
    insert_text(
        &mut stable,
        "tagged",
        Some(if metadata.tagged { "true" } else { "false" }),
    );
    if let Some(created) = metadata.created {
        stable.insert("created".to_string(), created.to_rfc3339());
    }
    if let Some(modified) = metadata.modified {
        volatile.insert("modified".to_string(), modified.to_rfc3339());
    }
    insert_text(&mut volatile, "producer", metadata.producer.as_deref());

    if metadata.encrypted {
        unsupported.insert("encrypted".to_string());
    }
    if !parsed.form_fields.is_empty() {
        unsupported.insert("form-fields".to_string());
    }
    if parsed.outline.is_some() {
        unsupported.insert("outline".to_string());
    }

    let mut numbering = BTreeMap::new();
    for (id, level) in list_levels {
        numbering.insert(
            format!("{id}:{level}"),
            NumberingLevel {
                format: if id == "pdf-bullet" {
                    "bullet"
                } else {
                    "decimal"
                }
                .to_string(),
                text: String::new(),
                start: Some(1),
                indent: None,
                hanging: None,
            },
        );
    }

    let mut document = Document::new(Format::Pdf);
    document.properties.stable = stable;
    document.properties.volatile = volatile;
    document.properties.unsupported = unsupported;
    document.numbering = numbering;
    document.sections = sections;

    let (media_meta, media_bytes) = media.into_parts();
    document.media = media_meta;
    Ok((document, media_bytes))
}

fn convert_paragraph(
    paragraph: &PdfParagraph,
    resources: &BTreeMap<&str, &PdfResource>,
    media: &mut MediaStore,
    list_levels: &mut BTreeSet<(String, u8)>,
) -> Result<Paragraph> {
    let mut direct = ParaProps {
        align: match paragraph.style.alignment {
            Alignment::Center => Some("center".to_string()),
            Alignment::Right => Some("right".to_string()),
            Alignment::Justify => Some("justify".to_string()),
            Alignment::Left => None,
        },
        outline: paragraph
            .style
            .heading_level
            .map(|level| level.saturating_sub(1).min(8)),
        ..ParaProps::default()
    };

    if let Some(list) = &paragraph.style.list_info {
        let id = match &list.style {
            ListStyle::Unordered { .. } => "pdf-bullet",
            ListStyle::Ordered { .. } => "pdf-number",
        };
        let level = list.level.min(8);
        list_levels.insert((id.to_string(), level));
        direct.num = Some(NumRef {
            id: id.to_string(),
            level,
        });
        direct.level = Some(level);
    } else if paragraph.style.indent_level > 0 {
        direct.level = Some(paragraph.style.indent_level.min(8));
    }

    let mut indent = Indent::default();
    if let Some(value) = paragraph.style.first_line_indent {
        indent.first_line = Some(units::points(value as f64));
    }
    if indent != Indent::default() {
        direct.indent = Some(indent);
    }

    let mut spacing = Spacing::default();
    if let Some(value) = paragraph.style.space_before {
        spacing.before = Some(units::points(value as f64));
    }
    if let Some(value) = paragraph.style.space_after {
        spacing.after = Some(units::points(value as f64));
    }
    if spacing != Spacing::default() {
        direct.spacing = Some(spacing);
    }

    let mut runs = Vec::new();
    for content in &paragraph.content {
        match content {
            InlineContent::Text(run) => {
                if run.text.is_empty() {
                    continue;
                }
                runs.push(Run::Text(TextRun {
                    text: run.text.clone(),
                    direct: text_run_props(&run.style),
                    ..TextRun::default()
                }));
            }
            InlineContent::Link { text, url, .. } => {
                if text.is_empty() {
                    continue;
                }
                runs.push(Run::Text(TextRun {
                    text: text.clone(),
                    link: Some(url.clone()),
                    ..TextRun::default()
                }));
            }
            InlineContent::LineBreak => runs.push(Run::Break(BreakRun {
                kind: "line".to_string(),
            })),
            InlineContent::Image {
                resource_id,
                alt_text,
            } => {
                if let Some(image) = image_run(
                    resource_id,
                    alt_text.as_deref(),
                    ImageGeometry::default(),
                    resources,
                    media,
                )? {
                    runs.push(Run::Image(image));
                }
            }
        }
    }

    Ok(Paragraph {
        style: None,
        direct: (!direct.is_empty()).then_some(direct),
        runs,
    })
}

fn convert_table(
    table: &PdfTable,
    resources: &BTreeMap<&str, &PdfResource>,
    media: &mut MediaStore,
    list_levels: &mut BTreeSet<(String, u8)>,
) -> Result<Table> {
    let grid = table
        .column_widths
        .as_ref()
        .map(|widths| {
            widths
                .iter()
                .map(|width| units::points(*width as f64))
                .collect()
        })
        .unwrap_or_default();

    let mut rows = Vec::new();
    for (index, row) in table.rows.iter().enumerate() {
        let header = row.is_header || index < table.header_rows as usize;
        let direct = header.then_some(RowProps {
            height: None,
            header: Some(true),
        });

        let mut cells = Vec::new();
        for cell in &row.cells {
            let mut properties = CellProps::default();
            if cell.colspan > 1 {
                properties.col_span = Some(cell.colspan as u32);
            }
            if cell.rowspan > 1 {
                properties.row_span = Some(cell.rowspan as u32);
            }
            properties.v_align = None;
            let direct = (properties != CellProps::default()).then_some(properties);

            let mut blocks = Vec::new();
            for paragraph in &cell.content {
                blocks.push(Block::Paragraph(convert_paragraph(
                    paragraph,
                    resources,
                    media,
                    list_levels,
                )?));
            }
            cells.push(TableCell { direct, blocks });
        }
        rows.push(TableRow { direct, cells });
    }

    Ok(Table {
        style: None,
        grid,
        rows,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct ImageGeometry {
    width: Option<f32>,
    height: Option<f32>,
    x: Option<f32>,
    y: Option<f32>,
}

fn image_run(
    resource_id: &str,
    alt_text: Option<&str>,
    geometry: ImageGeometry,
    resources: &BTreeMap<&str, &PdfResource>,
    media: &mut MediaStore,
) -> Result<Option<ImageRun>> {
    let Some(resource) = resources.get(resource_id) else {
        return Ok(None);
    };
    if !resource.is_image() {
        return Ok(None);
    }
    let sha256 = media.add_bytes(
        resource.data.clone(),
        resource.mime_type.clone(),
        resource.width,
        resource.height,
    )?;
    Ok(Some(ImageRun {
        sha256,
        alt: alt_text
            .map(str::to_string)
            .or_else(|| resource.filename.clone()),
        name: resource.filename.clone(),
        width: geometry.width.map(|value| units::points(value as f64)),
        height: geometry.height.map(|value| units::points(value as f64)),
        anchor: if geometry.x.is_some() || geometry.y.is_some() {
            "float".to_string()
        } else {
            "inline".to_string()
        },
        x: geometry.x.map(|value| units::points(value as f64)),
        y: geometry.y.map(|value| units::points(value as f64)),
    }))
}

fn text_run_props(style: &PdfTextStyle) -> Option<RunProps> {
    let mut properties = RunProps::default();
    if style.bold {
        properties.bold = Some(true);
    }
    if style.italic {
        properties.italic = Some(true);
    }
    if style.underline {
        properties.underline = Some("single".to_string());
    }
    if style.strikethrough {
        properties.strike = Some(true);
    }
    if style.superscript {
        properties.vert_align = Some("superscript".to_string());
    } else if style.subscript {
        properties.vert_align = Some("subscript".to_string());
    }
    if let Some(size) = style.font_size {
        properties.size = Some(units::points(size as f64));
    }
    if let Some(color) = &style.color {
        properties.color = normalize_hex(color).or_else(|| Some(color.clone()));
    }
    if let Some(font) = &style.font_name {
        properties.font = Some(font.clone());
    }
    if let Some(background) = &style.background_color {
        properties.highlight = normalize_hex(background).or_else(|| Some(background.clone()));
    }
    (!properties.is_empty()).then_some(properties)
}

fn text_paragraph(text: &str) -> Paragraph {
    Paragraph {
        runs: vec![Run::Text(TextRun {
            text: text.to_string(),
            ..TextRun::default()
        })],
        ..Paragraph::default()
    }
}

fn insert_text(map: &mut BTreeMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        let value = value.trim();
        if !value.is_empty() {
            map.insert(key.to_string(), value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_pdf() -> Vec<u8> {
        let content = "BT /F1 12 Tf 20 50 Td (Hello PDF) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];

        let mut output = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(output.len());
            output.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
        }
        let xref = output.len();
        output.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            output.push_str(&format!("{offset:010} 00000 n \n"));
        }
        output.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        output.into_bytes()
    }

    #[test]
    fn parses_a_minimal_pdf_page_and_text() {
        let (document, media) = parse(&minimal_pdf()).expect("parse pdf");
        assert!(media.is_empty());
        assert_eq!(document.format, Format::Pdf);
        assert_eq!(document.sections.len(), 1);

        let section = &document.sections[0];
        assert_eq!(section.kind, SectionKind::Page);
        let page = section.page.as_ref().expect("page setup");
        assert_eq!(page.width, Some(20000));
        assert_eq!(page.height, Some(10000));

        let text = section
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(paragraph) => Some(
                    paragraph
                        .runs
                        .iter()
                        .filter_map(|run| match run {
                            Run::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Hello PDF"), "text was {text:?}");

        let run = section
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Paragraph(paragraph) => paragraph.runs.iter().find_map(|run| match run {
                    Run::Text(text) => Some(text),
                    _ => None,
                }),
                _ => None,
            })
            .expect("text run");
        assert_eq!(run.text, "Hello PDF");
    }
}
