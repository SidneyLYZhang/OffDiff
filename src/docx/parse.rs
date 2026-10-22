//! WordprocessingML document parsing.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use roxmltree::Node;

use crate::error::{OffDiffError, Result};
use crate::model::{
    Block, CellProps, Defaults, Document, Format, ImageRun, NumberingLevel, PageSetup, Paragraph,
    RowProps, Run, RunProps, Section, SectionKind, Style, Table, TableCell, TableRow, TextRun,
};
use crate::ooxml::rels::{self, Relationships};
use crate::ooxml::{units, xml, Package};

use super::styles::{self, parse_numbering, StyleSheet, Theme};
use crate::media::MediaStore;

type MemoryPackage = Package<Cursor<Vec<u8>>>;

/// Parse a .docx container into the IR plus the embedded media bytes.
pub fn parse(bytes: &[u8]) -> Result<(Document, BTreeMap<String, Vec<u8>>)> {
    let mut package = MemoryPackage::from_bytes(bytes)?;
    let document_xml = package.read("word/document.xml")?;
    let styles_xml = package.read_opt("word/styles.xml")?;
    let theme_xml = package.read_opt("word/theme/theme1.xml")?;
    let numbering_xml = package.read_opt("word/numbering.xml")?;
    let relationships = match package.read_opt("word/_rels/document.xml.rels")? {
        Some(bytes) => rels::parse(&bytes)?,
        None => Relationships::new(),
    };

    let theme = Theme::parse(theme_xml.as_deref())?;
    let sheet = StyleSheet::parse(styles_xml.as_deref(), &theme)?;
    let numbering = parse_numbering(numbering_xml.as_deref())?;
    let properties = crate::ooxml::read_properties(&mut package)?;

    let mut unsupported = package_unsupported(&package);
    let document = xml::parse(&document_xml)?;
    let body = xml::descendant(document.root_element(), "body")
        .ok_or_else(|| OffDiffError::Corrupt("word/document.xml has no body".into()))?;
    scan_unsupported(body, &mut unsupported);
    let page = parse_page_setup(body);

    let mut media = MediaStore::new();
    let blocks = {
        let mut context = Context {
            theme: &theme,
            relationships: &relationships,
            package: &mut package,
            media: &mut media,
        };
        context.collect_blocks(body)?
    };
    let (media_meta, media_bytes) = media.into_parts();

    // Keep only styles the document actually reaches, plus their `basedOn`
    // chain and the defaults. Word regenerates dozens of unused built-in
    // styles on every save; keeping them would produce noise on upgrade.
    let referenced = expand_styles(&sheet.styles, referenced_styles(&blocks, &sheet.defaults));
    let styles: BTreeMap<String, Style> = referenced
        .iter()
        .filter_map(|id| {
            sheet
                .styles
                .get(id)
                .map(|style| (id.clone(), style.clone()))
        })
        .collect();
    let numbering = prune_numbering(&numbering, &sheet, &blocks);

    let mut document = Document::new(Format::Docx);
    document.properties = properties;
    document.properties.unsupported = unsupported;
    document.defaults = Some(sheet.defaults.clone());
    document.styles = styles;
    document.numbering = numbering;
    document.media = media_meta;
    document.sections.push(Section {
        kind: SectionKind::Body,
        name: None,
        hidden: None,
        page,
        cols: Vec::new(),
        merges: Vec::new(),
        blocks,
        notes: Vec::new(),
    });

    Ok((document, media_bytes))
}

struct Context<'a> {
    theme: &'a Theme,
    relationships: &'a Relationships,
    package: &'a mut MemoryPackage,
    media: &'a mut MediaStore,
}

impl<'a> Context<'a> {
    fn collect_blocks(&mut self, node: Node<'_, '_>) -> Result<Vec<Block>> {
        let mut blocks = Vec::new();
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "p" => blocks.push(Block::Paragraph(self.paragraph(child)?)),
                "tbl" => blocks.push(Block::Table(self.table(child)?)),
                "sdt" => {
                    if let Some(content) = xml::child(child, "sdtContent") {
                        blocks.extend(self.collect_blocks(content)?);
                    }
                }
                "customXml" => blocks.extend(self.collect_blocks(child)?),
                _ => {}
            }
        }
        Ok(blocks)
    }

    fn paragraph(&mut self, node: Node<'_, '_>) -> Result<Paragraph> {
        let properties = xml::child(node, "pPr");
        let style = properties
            .and_then(|p| xml::child(p, "pStyle"))
            .and_then(xml::val)
            .map(str::to_string);
        let direct = properties
            .map(|p| styles::parse_para_props(Some(p)))
            .filter(|props| !props.is_empty());

        let mut runs: Vec<Run> = Vec::new();
        for descendant in node.descendants().filter(Node::is_element) {
            if descendant.tag_name().name() != "r" {
                continue;
            }
            if nearest_paragraph(descendant) != Some(node) {
                continue;
            }
            if is_deleted(descendant) || is_in_textbox(descendant) {
                continue;
            }
            let link = self.link_for(descendant);
            self.run(descendant, link, &mut runs)?;
        }

        Ok(Paragraph {
            style,
            direct,
            runs,
        })
    }

    fn run(&mut self, node: Node<'_, '_>, link: Option<String>, out: &mut Vec<Run>) -> Result<()> {
        let properties = xml::child(node, "rPr");
        let style = properties
            .and_then(|p| xml::child(p, "rStyle"))
            .and_then(xml::val)
            .map(str::to_string);
        let direct = properties
            .map(|p| styles::parse_run_props(Some(p), self.theme))
            .filter(|props| !props.is_empty());

        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "t" => {
                    let text = xml::text(child);
                    if !text.is_empty() {
                        push_text(out, text, &style, &direct, &link);
                    }
                }
                "tab" => out.push(Run::Tab),
                "br" => {
                    let kind = xml::attr(child, "type").unwrap_or("line");
                    out.push(Run::Break(crate::model::BreakRun {
                        kind: kind.to_ascii_lowercase(),
                    }));
                }
                "cr" => out.push(Run::Break(crate::model::BreakRun {
                    kind: "line".to_string(),
                })),
                "noBreakHyphen" => push_text(out, "\u{2011}".to_string(), &style, &direct, &link),
                "drawing" => {
                    if let Some(image) = self.image(child)? {
                        out.push(Run::Image(image));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn link_for(&self, node: Node<'_, '_>) -> Option<String> {
        let hyperlink = node
            .ancestors()
            .find(|a| a.is_element() && a.tag_name().name() == "hyperlink")?;
        if let Some(id) = xml::attr(hyperlink, "id") {
            if let Some(relationship) = self.relationships.get(id) {
                if relationship.external {
                    return Some(relationship.target.clone());
                }
            }
        }
        xml::attr(hyperlink, "anchor").map(|anchor| format!("#{anchor}"))
    }

    fn image(&mut self, drawing: Node<'_, '_>) -> Result<Option<ImageRun>> {
        let Some(blip) = xml::descendant(drawing, "blip") else {
            return Ok(None);
        };
        let Some(id) = xml::attr(blip, "embed") else {
            return Ok(None);
        };
        let Some(relationship) = self.relationships.get(id) else {
            return Ok(None);
        };
        if relationship.external {
            return Ok(None);
        }
        let part = rels::resolve("word/document.xml", &relationship.target);
        let Context { package, media, .. } = self;
        let sha256 = media.add(package, &part)?;

        let extent = drawing
            .descendants()
            .find(|n| n.tag_name().name() == "extent");
        let (width, height) = extent
            .map(|node| {
                (
                    xml::attr(node, "cx")
                        .and_then(units::parse_i64)
                        .map(units::emu),
                    xml::attr(node, "cy")
                        .and_then(units::parse_i64)
                        .map(units::emu),
                )
            })
            .unwrap_or((None, None));

        let anchor = if drawing
            .descendants()
            .any(|n| n.tag_name().name() == "anchor")
        {
            "float"
        } else {
            "inline"
        };

        let doc_pr = drawing
            .descendants()
            .find(|n| n.tag_name().name() == "docPr");
        let alt = doc_pr
            .and_then(|n| xml::attr(n, "descr").or_else(|| xml::attr(n, "title")))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = doc_pr
            .and_then(|n| xml::attr(n, "name"))
            .map(str::to_string);

        let (x, y) = if anchor == "float" {
            (offset(drawing, "positionH"), offset(drawing, "positionV"))
        } else {
            (None, None)
        };

        Ok(Some(ImageRun {
            sha256,
            alt,
            name,
            width,
            height,
            anchor: anchor.to_string(),
            x,
            y,
        }))
    }

    fn table(&mut self, node: Node<'_, '_>) -> Result<Table> {
        let style = xml::child(node, "tblPr")
            .and_then(|properties| xml::child(properties, "tblStyle"))
            .and_then(xml::val)
            .map(str::to_string);

        let grid = xml::child(node, "tblGrid")
            .map(|grid| {
                xml::children(grid, "gridCol")
                    .filter_map(|column| {
                        xml::attr(column, "w")
                            .and_then(units::parse_i64)
                            .map(units::twips)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut rows = Vec::new();
        for row in xml::children(node, "tr") {
            let direct = xml::child(row, "trPr")
                .map(|properties| RowProps {
                    height: xml::child(properties, "trHeight")
                        .and_then(|height| xml::attr(height, "val"))
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    header: xml::child(properties, "tblHeader").and_then(xml::on_off),
                })
                .filter(|properties| *properties != RowProps::default());

            let mut cells = Vec::new();
            for cell in xml::children(row, "tc") {
                let direct = xml::child(cell, "tcPr")
                    .map(|properties| {
                        let width_node = xml::child(properties, "tcW");
                        let width = width_node
                            .filter(|node| matches!(xml::attr(*node, "type"), None | Some("dxa")))
                            .and_then(|node| xml::attr(node, "w"))
                            .and_then(units::parse_i64)
                            .map(units::twips);
                        CellProps {
                            width,
                            col_span: xml::child(properties, "gridSpan")
                                .and_then(xml::val)
                                .and_then(units::parse_i64)
                                .map(|span| span.max(1) as u32),
                            row_span: None,
                            v_merge: xml::child(properties, "vMerge").map(|node| {
                                xml::val(node)
                                    .map(|value| value.to_ascii_lowercase())
                                    .unwrap_or_else(|| "continue".to_string())
                            }),
                            shading: xml::child(properties, "shd")
                                .and_then(|node| xml::attr(node, "fill"))
                                .and_then(styles::normalize_hex),
                            v_align: xml::child(properties, "vAlign")
                                .and_then(xml::val)
                                .map(str::to_string),
                        }
                    })
                    .filter(|properties| *properties != CellProps::default());

                let blocks = self.collect_blocks(cell)?;
                cells.push(TableCell { direct, blocks });
            }
            rows.push(TableRow { direct, cells });
        }
        compute_row_spans(&mut rows);

        Ok(Table { style, grid, rows })
    }
}

fn push_text(
    out: &mut Vec<Run>,
    text: String,
    style: &Option<String>,
    direct: &Option<RunProps>,
    link: &Option<String>,
) {
    if let Some(Run::Text(last)) = out.last_mut() {
        if last.style == *style && last.direct == *direct && last.link == *link {
            last.text.push_str(&text);
            return;
        }
    }
    out.push(Run::Text(TextRun {
        text,
        style: style.clone(),
        direct: direct.clone(),
        link: link.clone(),
        bbox: None,
    }));
}

fn referenced_styles(blocks: &[Block], defaults: &Defaults) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    for id in [
        defaults.paragraph.as_ref(),
        defaults.character.as_ref(),
        defaults.table.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        used.insert(id.clone());
    }
    collect_block_styles(blocks, &mut used);
    used
}

fn collect_block_styles(blocks: &[Block], used: &mut BTreeSet<String>) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                if let Some(style) = &paragraph.style {
                    used.insert(style.clone());
                }
                for run in &paragraph.runs {
                    if let Run::Text(text) = run {
                        if let Some(style) = &text.style {
                            used.insert(style.clone());
                        }
                    }
                }
            }
            Block::Table(table) => {
                if let Some(style) = &table.style {
                    used.insert(style.clone());
                }
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_block_styles(&cell.blocks, used);
                    }
                }
            }
            _ => {}
        }
    }
}

fn expand_styles(styles: &BTreeMap<String, Style>, mut used: BTreeSet<String>) -> BTreeSet<String> {
    let mut queue: Vec<String> = used.iter().cloned().collect();
    while let Some(id) = queue.pop() {
        if let Some(style) = styles.get(&id) {
            if let Some(base) = &style.based_on {
                if used.insert(base.clone()) {
                    queue.push(base.clone());
                }
            }
        }
    }
    used
}

fn prune_numbering(
    numbering: &BTreeMap<String, NumberingLevel>,
    sheet: &StyleSheet,
    blocks: &[Block],
) -> BTreeMap<String, NumberingLevel> {
    let mut used = BTreeSet::new();
    collect_numbering(blocks, sheet, &mut used);
    numbering
        .iter()
        .filter(|(key, _)| {
            key.split_once(':')
                .map(|(id, _)| used.contains(id))
                .unwrap_or(false)
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn collect_numbering(blocks: &[Block], sheet: &StyleSheet, used: &mut BTreeSet<String>) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                let properties =
                    sheet.para_effective(paragraph.style.as_deref(), paragraph.direct.as_ref());
                if let Some(num) = properties.num {
                    used.insert(num.id);
                }
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_numbering(&cell.blocks, sheet, used);
                    }
                }
            }
            _ => {}
        }
    }
}

fn offset(drawing: Node<'_, '_>, axis: &str) -> Option<i64> {
    drawing
        .descendants()
        .find(|node| node.tag_name().name() == axis)
        .and_then(|node| xml::child(node, "posOffset"))
        .and_then(|node| node.text())
        .and_then(|text| text.trim().parse::<i64>().ok())
        .map(units::emu)
}

fn nearest_paragraph<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.ancestors()
        .find(|ancestor| ancestor.is_element() && ancestor.tag_name().name() == "p")
}

fn is_deleted(node: Node<'_, '_>) -> bool {
    node.ancestors().any(|ancestor| {
        ancestor.is_element() && matches!(ancestor.tag_name().name(), "del" | "moveFrom")
    })
}

fn is_in_textbox(node: Node<'_, '_>) -> bool {
    node.ancestors()
        .any(|ancestor| ancestor.is_element() && ancestor.tag_name().name() == "txbxContent")
}

fn compute_row_spans(rows: &mut [TableRow]) {
    let layout: Vec<Vec<u32>> = rows
        .iter()
        .map(|row| {
            let mut column = 0u32;
            row.cells
                .iter()
                .map(|cell| {
                    let start = column;
                    column += cell
                        .direct
                        .as_ref()
                        .and_then(|direct| direct.col_span)
                        .unwrap_or(1);
                    start
                })
                .collect()
        })
        .collect();

    for row_index in 0..rows.len() {
        for cell_index in 0..rows[row_index].cells.len() {
            let restart = rows[row_index].cells[cell_index]
                .direct
                .as_ref()
                .and_then(|direct| direct.v_merge.as_deref())
                == Some("restart");
            if !restart {
                continue;
            }
            let start = layout[row_index][cell_index];
            let mut span = 1u32;
            for next in (row_index + 1)..rows.len() {
                let continues = layout[next]
                    .iter()
                    .position(|&column| column == start)
                    .and_then(|position| {
                        rows[next].cells[position]
                            .direct
                            .as_ref()
                            .and_then(|direct| direct.v_merge.as_deref())
                    })
                    == Some("continue");
                if !continues {
                    break;
                }
                span += 1;
            }
            if span > 1 {
                if let Some(direct) = rows[row_index].cells[cell_index].direct.as_mut() {
                    direct.row_span = Some(span);
                }
            }
        }
    }
}

fn parse_page_setup(body: Node<'_, '_>) -> Option<PageSetup> {
    let section = xml::child(body, "sectPr")?;
    let mut page = PageSetup::default();
    if let Some(size) = xml::child(section, "pgSz") {
        page.width = xml::attr(size, "w")
            .and_then(units::parse_i64)
            .map(units::twips);
        page.height = xml::attr(size, "h")
            .and_then(units::parse_i64)
            .map(units::twips);
        page.orientation = xml::attr(size, "orient").map(|value| value.to_ascii_lowercase());
    }
    if let Some(margins) = xml::child(section, "pgMar") {
        let read = |name: &str| {
            xml::attr(margins, name)
                .and_then(units::parse_i64)
                .map(units::twips)
        };
        page.margin = Some(crate::model::Margins {
            top: read("top"),
            right: read("right"),
            bottom: read("bottom"),
            left: read("left"),
            header: read("header"),
            footer: read("footer"),
        });
    }
    Some(page)
}

fn package_unsupported(package: &MemoryPackage) -> BTreeSet<String> {
    let mut unsupported = BTreeSet::new();
    for name in package.names() {
        if name.starts_with("word/header") || name.starts_with("word/footer") {
            unsupported.insert("headers-footers".to_string());
        } else if name == "word/footnotes.xml" {
            unsupported.insert("footnotes".to_string());
        } else if name == "word/endnotes.xml" {
            unsupported.insert("endnotes".to_string());
        } else if name == "word/comments.xml" {
            unsupported.insert("comments".to_string());
        }
    }
    unsupported
}

fn scan_unsupported(body: Node<'_, '_>, unsupported: &mut BTreeSet<String>) {
    let mut sections = 0usize;
    for node in body.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "ins" | "del" | "moveFrom" | "moveTo" => {
                unsupported.insert("tracked-changes".to_string());
            }
            "fldChar" | "instrText" | "fldSimple" => {
                unsupported.insert("fields".to_string());
            }
            "oMath" | "oMathPara" => {
                unsupported.insert("math".to_string());
            }
            "chart" => {
                unsupported.insert("charts".to_string());
            }
            "txbxContent" => {
                unsupported.insert("text-boxes".to_string());
            }
            "pict" => {
                unsupported.insert("vml".to_string());
            }
            "object" => {
                unsupported.insert("embedded-objects".to_string());
            }
            "altChunk" => {
                unsupported.insert("alt-chunks".to_string());
            }
            "sectPr" => sections += 1,
            _ => {}
        }
    }
    if sections > 1 {
        unsupported.insert("multiple-sections".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in parts {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer
    }

    fn document(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<w:body>{body}</w:body></w:document>"#
        )
    }

    fn styles() -> &'static [u8] {
        br#"<w:styles xmlns:w="w">
  <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
</w:styles>"#
    }

    #[test]
    fn parses_paragraphs_runs_and_formatting() {
        let body = r#"
<w:p>
  <w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/></w:pPr>
  <w:r><w:rPr><w:b/><w:i/></w:rPr><w:t xml:space="preserve">Hello</w:t></w:r>
  <w:r><w:rPr><w:u w:val="double"/><w:color w:val="FF0000"/><w:sz w:val="44"/></w:rPr><w:t xml:space="preserve"> world</w:t></w:r>
</w:p>
<w:p></w:p>
"#;
        let bytes = package(&[
            ("word/document.xml", document(body).as_bytes()),
            ("word/styles.xml", styles()),
        ]);
        let (doc, _media) = parse(&bytes).unwrap();
        assert_eq!(doc.sections.len(), 1);
        let blocks = &doc.sections[0].blocks;
        assert_eq!(blocks.len(), 2);

        let Block::Paragraph(paragraph) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(paragraph.style.as_deref(), Some("Heading1"));
        assert_eq!(
            paragraph.direct.as_ref().unwrap().align.as_deref(),
            Some("center")
        );
        assert_eq!(paragraph.runs.len(), 2);
        let Run::Text(first) = &paragraph.runs[0] else {
            panic!("expected text");
        };
        assert_eq!(first.text, "Hello");
        assert_eq!(first.direct.as_ref().unwrap().bold, Some(true));
        assert_eq!(first.direct.as_ref().unwrap().italic, Some(true));
        let Run::Text(second) = &paragraph.runs[1] else {
            panic!("expected text");
        };
        assert_eq!(second.text, " world");
        assert_eq!(
            second.direct.as_ref().unwrap().underline.as_deref(),
            Some("double")
        );
        assert_eq!(
            second.direct.as_ref().unwrap().color.as_deref(),
            Some("#FF0000")
        );
        assert_eq!(second.direct.as_ref().unwrap().size, Some(2200));

        let Block::Paragraph(empty) = &blocks[1] else {
            panic!("expected paragraph");
        };
        assert!(empty.runs.is_empty());
    }

    #[test]
    fn keeps_tabs_breaks_and_whitespace() {
        let body = r#"<w:p><w:r><w:t xml:space="preserve">  a  </w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p>"#;
        let bytes = package(&[("word/document.xml", document(body).as_bytes())]);
        let (doc, _) = parse(&bytes).unwrap();
        let Block::Paragraph(paragraph) = &doc.sections[0].blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(paragraph.runs.len(), 5);
        let Run::Text(first) = &paragraph.runs[0] else {
            panic!("expected text");
        };
        assert_eq!(first.text, "  a  ");
        assert!(matches!(paragraph.runs[1], Run::Tab));
        assert!(matches!(paragraph.runs[3], Run::Break(_)));
    }

    #[test]
    fn parses_tables_with_spans_and_shading() {
        let body = r#"
<w:tbl>
  <w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr>
  <w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/></w:tblGrid>
  <w:tr>
    <w:tc><w:tcPr><w:gridSpan w:val="2"/><w:shd w:fill="EEEEEE"/></w:tcPr><w:p><w:r><w:t>head</w:t></w:r></w:p></w:tc>
  </w:tr>
  <w:tr>
    <w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
    <w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc>
  </w:tr>
  <w:tr>
    <w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p></w:p></w:tc>
    <w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>
  </w:tr>
</w:tbl>
"#;
        let bytes = package(&[("word/document.xml", document(body).as_bytes())]);
        let (doc, _) = parse(&bytes).unwrap();
        let Block::Table(table) = &doc.sections[0].blocks[0] else {
            panic!("expected table");
        };
        assert_eq!(table.style.as_deref(), Some("TableGrid"));
        assert_eq!(table.grid, vec![10000, 15000]);
        assert_eq!(table.rows.len(), 3);
        assert_eq!(
            table.rows[0].cells[0].direct.as_ref().unwrap().col_span,
            Some(2)
        );
        assert_eq!(
            table.rows[1].cells[0].direct.as_ref().unwrap().row_span,
            Some(2)
        );
    }

    #[test]
    fn extracts_images_and_hashes_them() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&10u32.to_be_bytes());
        png.extend_from_slice(&20u32.to_be_bytes());

        let body = r#"
<w:p><w:r><w:drawing>
  <wp:inline>
    <wp:extent cx="1270000" cy="2540000"/>
    <wp:docPr id="1" name="Picture 1" descr="A logo"/>
    <a:graphic><a:graphicData><a:blip r:embed="rId5"/></a:graphicData></a:graphic>
  </wp:inline>
</w:drawing></w:r></w:p>
"#;
        let rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;
        let bytes = package(&[
            ("word/document.xml", document(body).as_bytes()),
            ("word/_rels/document.xml.rels", rels),
            ("word/media/image1.png", &png),
        ]);
        let (doc, media_bytes) = parse(&bytes).unwrap();
        let Block::Paragraph(paragraph) = &doc.sections[0].blocks[0] else {
            panic!("expected paragraph");
        };
        let Run::Image(image) = &paragraph.runs[0] else {
            panic!("expected image");
        };
        assert_eq!(image.anchor, "inline");
        assert_eq!(image.alt.as_deref(), Some("A logo"));
        assert_eq!(image.width, Some(10000));
        assert_eq!(image.height, Some(20000));
        assert_eq!(doc.media[&image.sha256].width, Some(10));
        assert_eq!(doc.media[&image.sha256].height, Some(20));
        assert_eq!(media_bytes.len(), 1);
    }

    #[test]
    fn reads_metadata_and_flags_unsupported_parts() {
        let core = br#"<cp:coreProperties xmlns:cp="cp" xmlns:dc="dc">
  <dc:title>Sample</dc:title><dc:creator>Ada</dc:creator>
  <cp:lastModifiedBy>Bob</cp:lastModifiedBy><cp:revision>3</cp:revision>
</cp:coreProperties>"#;
        let app = br#"<Properties><Application>Microsoft Office Word</Application><AppVersion>16.0</AppVersion><Company>ACME</Company></Properties>"#;
        let bytes = package(&[
            ("word/document.xml", document("<w:p/>").as_bytes()),
            ("docProps/core.xml", core),
            ("docProps/app.xml", app),
            ("word/comments.xml", b"<comments/>"),
        ]);
        let (doc, _) = parse(&bytes).unwrap();
        assert_eq!(
            doc.properties.stable.get("title").map(String::as_str),
            Some("Sample")
        );
        assert_eq!(
            doc.properties.stable.get("author").map(String::as_str),
            Some("Ada")
        );
        assert_eq!(
            doc.properties.stable.get("company").map(String::as_str),
            Some("ACME")
        );
        assert_eq!(
            doc.properties.volatile.get("revision").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            doc.properties
                .volatile
                .get("application")
                .map(String::as_str),
            Some("Microsoft Office Word")
        );
        assert!(!doc.properties.stable.contains_key("application"));
        assert!(doc.properties.unsupported.contains("comments"));
    }

    #[test]
    fn prunes_unused_styles_and_numbering() {
        let styles_xml = br#"<w:styles xmlns:w="w">
          <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
          <w:style w:type="paragraph" w:styleId="Used"><w:name w:val="Used"/><w:basedOn w:val="Normal"/></w:style>
          <w:style w:type="paragraph" w:styleId="Unused"><w:name w:val="Unused"/></w:style>
        </w:styles>"#;
        let numbering = r#"<w:numbering xmlns:w="w">
          <w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/></w:lvl></w:abstractNum>
          <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
          <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
        </w:numbering>"#;
        let body = r#"
<w:p><w:pPr><w:pStyle w:val="Used"/><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>
"#;
        let bytes = package(&[
            ("word/document.xml", document(body).as_bytes()),
            ("word/styles.xml", styles_xml),
            ("word/numbering.xml", numbering.as_bytes()),
        ]);
        let (doc, _) = parse(&bytes).unwrap();

        assert!(doc.styles.contains_key("Used"));
        assert!(doc.styles.contains_key("Normal"));
        assert!(!doc.styles.contains_key("Unused"));

        assert!(doc.numbering.contains_key("1:0"));
        assert!(!doc.numbering.contains_key("2:0"));
    }

    #[test]
    fn marks_tracked_changes_and_fields() {
        let body = r#"<w:p>
  <w:r><w:fldChar w:fldCharType="begin"/></w:r>
  <w:ins><w:r><w:t>new</w:t></w:r></w:ins>
  <w:del><w:r><w:delText>old</w:delText></w:r></w:del>
</w:p>"#;
        let bytes = package(&[("word/document.xml", document(body).as_bytes())]);
        let (doc, _) = parse(&bytes).unwrap();
        assert!(doc.properties.unsupported.contains("tracked-changes"));
        assert!(doc.properties.unsupported.contains("fields"));
        let Block::Paragraph(paragraph) = &doc.sections[0].blocks[0] else {
            panic!("expected paragraph");
        };
        // Inserted text is kept; deleted text is not.
        assert_eq!(paragraph.runs.len(), 1);
    }
}
