//! PresentationML slide parsing.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use roxmltree::Node;

use crate::error::Result;
use crate::media::MediaStore;
use crate::model::{
    Block, BreakRun, CellProps, Document, Format, ImageRun, Indent, PageSetup, ParaProps,
    Paragraph, Rect, RowProps, Run, RunProps, Section, SectionKind, Shape, Table, TableCell,
    TableRow, TextRun,
};
use crate::ooxml::rels::{self, Relationships};
use crate::ooxml::{normalize_hex, units, xml, Package};

type MemoryPackage = Package<Cursor<Vec<u8>>>;

pub fn parse(bytes: &[u8]) -> Result<(Document, BTreeMap<String, Vec<u8>>)> {
    let mut package = MemoryPackage::from_bytes(bytes)?;
    let presentation_xml = package.read("ppt/presentation.xml")?;
    let presentation_rels = match package.read_opt("ppt/_rels/presentation.xml.rels")? {
        Some(bytes) => rels::parse(&bytes)?,
        None => Relationships::new(),
    };
    let mut properties = crate::ooxml::read_properties(&mut package)?;
    let mut unsupported = package_unsupported(&package);
    let mut media = MediaStore::new();

    let presentation = xml::parse(&presentation_xml)?;
    let root = presentation.root_element();
    let slide_size = xml::child(root, "sldSz").map(|node| PageSetup {
        width: xml::attr(node, "cx")
            .and_then(units::parse_i64)
            .map(units::emu),
        height: xml::attr(node, "cy")
            .and_then(units::parse_i64)
            .map(units::emu),
        orientation: None,
        rotation: None,
        margin: None,
    });

    let mut sections = Vec::new();
    if let Some(list) = xml::child(root, "sldIdLst") {
        for slide_ref in xml::children(list, "sldId") {
            let Some(id) = xml::attr_ns(slide_ref, "relationships", "id") else {
                continue;
            };
            let Some(relationship) = presentation_rels.get(id) else {
                continue;
            };
            let part = rels::resolve("ppt/presentation.xml", &relationship.target);
            let slide_xml = package.read(&part)?;
            let slide_rels = match package.read_opt(&rels::path_for(&part))? {
                Some(bytes) => rels::parse(&bytes)?,
                None => Relationships::new(),
            };
            let parsed = parse_slide(
                &slide_xml,
                &part,
                &slide_rels,
                &mut package,
                &mut media,
                &mut unsupported,
            )?;
            sections.push(Section {
                kind: SectionKind::Slide,
                name: parsed.name,
                hidden: parsed.hidden.then_some(true),
                page: slide_size.clone(),
                cols: Vec::new(),
                merges: Vec::new(),
                blocks: parsed.blocks,
                notes: parsed.notes,
            });
        }
    }

    let (media_meta, media_bytes) = media.into_parts();
    let mut document = Document::new(Format::Pptx);
    properties.unsupported = unsupported;
    document.properties = properties;
    document.media = media_meta;
    document.sections = sections;

    Ok((document, media_bytes))
}

struct ParsedSlide {
    blocks: Vec<Block>,
    notes: Vec<Block>,
    name: Option<String>,
    hidden: bool,
}

fn parse_slide(
    bytes: &[u8],
    part: &str,
    relationships: &Relationships,
    package: &mut MemoryPackage,
    media: &mut MediaStore,
    unsupported: &mut BTreeSet<String>,
) -> Result<ParsedSlide> {
    let document = xml::parse(bytes)?;
    let root = document.root_element();
    let hidden = matches!(xml::attr(root, "show"), Some("0") | Some("false"));
    let common_slide = xml::child(root, "cSld");
    let name = common_slide
        .and_then(|node| xml::attr(node, "name"))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    scan_slide_unsupported(root, unsupported);

    let shapes = if let Some(tree) = common_slide.and_then(|node| xml::child(node, "spTree")) {
        let mut context = SlideContext {
            relationships,
            slide_part: part,
            package,
            media,
            unsupported,
        };
        context.shapes(tree)?
    } else {
        Vec::new()
    };

    let notes = match relationships
        .values()
        .find(|relationship| relationship.kind.ends_with("/notesSlide"))
    {
        Some(relationship) => {
            let notes_part = rels::resolve(part, &relationship.target);
            let notes_xml = package.read(&notes_part)?;
            parse_notes(&notes_xml, unsupported)?
        }
        None => Vec::new(),
    };

    Ok(ParsedSlide {
        blocks: shapes,
        notes,
        name,
        hidden,
    })
}

struct SlideContext<'a> {
    relationships: &'a Relationships,
    slide_part: &'a str,
    package: &'a mut MemoryPackage,
    media: &'a mut MediaStore,
    unsupported: &'a mut BTreeSet<String>,
}

impl<'a> SlideContext<'a> {
    fn shapes(&mut self, parent: Node<'_, '_>) -> Result<Vec<Block>> {
        let mut shapes = Vec::new();
        for child in parent.children().filter(Node::is_element) {
            if let Some(shape) = self.shape(child)? {
                shapes.push(Block::Shape(shape));
            }
        }
        Ok(shapes)
    }

    fn shape(&mut self, node: Node<'_, '_>) -> Result<Option<Shape>> {
        let kind = node.tag_name().name().to_string();
        let name = xml::descendant(node, "cNvPr")
            .and_then(|node| xml::attr(node, "name"))
            .map(str::to_string);
        let position = shape_position(node);

        let shape = match kind.as_str() {
            "sp" => {
                let paragraphs = xml::child(node, "txBody")
                    .map(|body| collect_paragraphs(body, self.unsupported))
                    .unwrap_or_default();
                Shape {
                    kind,
                    name,
                    position,
                    paragraphs,
                    image: None,
                    table: None,
                    children: Vec::new(),
                }
            }
            "pic" => {
                let image = self.picture(node)?;
                Shape {
                    kind,
                    name,
                    position,
                    paragraphs: Vec::new(),
                    image,
                    table: None,
                    children: Vec::new(),
                }
            }
            "graphicFrame" => {
                let table = xml::descendant(node, "tbl").map(|table| self.table(table));
                Shape {
                    kind,
                    name,
                    position,
                    paragraphs: Vec::new(),
                    image: None,
                    table,
                    children: Vec::new(),
                }
            }
            "grpSp" => {
                let children = self
                    .shapes(node)?
                    .into_iter()
                    .filter_map(|block| match block {
                        Block::Shape(shape) => Some(shape),
                        _ => None,
                    })
                    .collect();
                Shape {
                    kind,
                    name,
                    position,
                    paragraphs: Vec::new(),
                    image: None,
                    table: None,
                    children,
                }
            }
            "cxnSp" => Shape {
                kind,
                name,
                position,
                paragraphs: Vec::new(),
                image: None,
                table: None,
                children: Vec::new(),
            },
            _ => return Ok(None),
        };
        Ok(Some(shape))
    }

    fn picture(&mut self, node: Node<'_, '_>) -> Result<Option<ImageRun>> {
        let Some(blip) = xml::descendant(node, "blip") else {
            return Ok(None);
        };
        let Some(embed) = xml::attr_ns(blip, "relationships", "embed") else {
            return Ok(None);
        };
        let Some(relationship) = self.relationships.get(embed) else {
            return Ok(None);
        };
        if relationship.external {
            return Ok(None);
        }
        let part = rels::resolve(self.slide_part, &relationship.target);
        let package = &mut *self.package;
        let media = &mut *self.media;
        let sha256 = media.add(package, &part)?;

        let position = shape_position(node);
        let c_nv = xml::descendant(node, "cNvPr");
        let alt = c_nv
            .and_then(|node| xml::attr(node, "descr").or_else(|| xml::attr(node, "title")))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = c_nv
            .and_then(|node| xml::attr(node, "name"))
            .map(str::to_string);

        Ok(Some(ImageRun {
            sha256,
            alt,
            name,
            width: position.map(|rect| rect.w),
            height: position.map(|rect| rect.h),
            anchor: "float".to_string(),
            x: position.map(|rect| rect.x),
            y: position.map(|rect| rect.y),
        }))
    }

    fn table(&mut self, table: Node<'_, '_>) -> Table {
        let grid = xml::child(table, "tblGrid")
            .map(|grid| {
                xml::children(grid, "gridCol")
                    .filter_map(|column| {
                        xml::attr(column, "w")
                            .and_then(units::parse_i64)
                            .map(units::emu)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut rows = Vec::new();
        for row in xml::children(table, "tr") {
            let direct = xml::attr(row, "h")
                .and_then(units::parse_i64)
                .map(|height| RowProps {
                    height: Some(units::emu(height)),
                    header: None,
                })
                .filter(|properties| *properties != RowProps::default());

            let mut cells = Vec::new();
            for cell in xml::children(row, "tc") {
                let direct = xml::child(cell, "tcPr")
                    .map(|properties| CellProps {
                        width: xml::attr(properties, "w")
                            .and_then(units::parse_i64)
                            .map(units::emu),
                        col_span: xml::attr(cell, "gridSpan")
                            .and_then(units::parse_i64)
                            .map(|span| span.max(1) as u32),
                        row_span: xml::attr(cell, "rowSpan")
                            .and_then(units::parse_i64)
                            .map(|span| span.max(1) as u32),
                        v_merge: (xml::attr(cell, "vMerge") == Some("1"))
                            .then(|| "continue".to_string()),
                        shading: xml::child(properties, "solidFill")
                            .and_then(|fill| xml::child(fill, "srgbClr"))
                            .and_then(xml::val)
                            .and_then(normalize_hex),
                        v_align: xml::attr(properties, "anchor").map(|anchor| {
                            match anchor {
                                "ctr" => "center",
                                "b" => "bottom",
                                other => other,
                            }
                            .to_string()
                        }),
                    })
                    .filter(|properties| *properties != CellProps::default());

                let blocks = xml::child(cell, "txBody")
                    .map(|body| {
                        collect_paragraphs(body, self.unsupported)
                            .into_iter()
                            .map(Block::Paragraph)
                            .collect()
                    })
                    .unwrap_or_default();
                cells.push(TableCell { direct, blocks });
            }
            rows.push(TableRow { direct, cells });
        }

        Table {
            style: None,
            grid,
            rows,
        }
    }
}

fn collect_paragraphs(tx_body: Node<'_, '_>, unsupported: &mut BTreeSet<String>) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    for node in xml::children(tx_body, "p") {
        let properties = xml::child(node, "pPr");
        let direct = properties
            .map(|ppr| ParaProps {
                align: xml::attr(ppr, "algn").map(normalize_align),
                level: xml::attr(ppr, "lvl")
                    .and_then(units::parse_i64)
                    .map(|value| value.clamp(0, 8) as u8),
                indent: {
                    let left = xml::attr(ppr, "marL")
                        .and_then(units::parse_i64)
                        .map(units::emu);
                    let hanging = xml::attr(ppr, "indent")
                        .and_then(units::parse_i64)
                        .map(units::emu);
                    if left.is_some() || hanging.is_some() {
                        Some(Indent {
                            left,
                            right: None,
                            first_line: None,
                            hanging,
                        })
                    } else {
                        None
                    }
                },
                ..ParaProps::default()
            })
            .filter(|properties| *properties != ParaProps::default());

        if let Some(properties) = properties {
            if xml::child(properties, "buChar").is_some()
                || xml::child(properties, "buAutoNum").is_some()
                || xml::child(properties, "buBlip").is_some()
            {
                unsupported.insert("bullets".to_string());
            }
        }
        if direct
            .as_ref()
            .and_then(|properties| properties.level)
            .map(|level| level > 0)
            .unwrap_or(false)
        {
            unsupported.insert("bullets".to_string());
        }

        let mut runs = Vec::new();
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "r" | "fld" => {
                    let properties = run_props(xml::child(child, "rPr"));
                    let text = xml::child(child, "t").map(xml::text).unwrap_or_default();
                    if !text.is_empty() {
                        runs.push(Run::Text(TextRun {
                            text,
                            direct: if properties.is_empty() {
                                None
                            } else {
                                Some(properties)
                            },
                            ..TextRun::default()
                        }));
                    }
                }
                "br" => runs.push(Run::Break(BreakRun {
                    kind: "line".to_string(),
                })),
                _ => {}
            }
        }

        if runs.is_empty() && direct.is_none() {
            continue;
        }
        paragraphs.push(Paragraph {
            style: None,
            direct,
            runs,
        });
    }
    paragraphs
}

fn run_props(node: Option<Node<'_, '_>>) -> RunProps {
    let Some(node) = node else {
        return RunProps::default();
    };
    let mut properties = RunProps::default();

    if let Some(value) = xml::attr(node, "b") {
        properties.bold = Some(value != "0");
    }
    if let Some(value) = xml::attr(node, "i") {
        properties.italic = Some(value != "0");
    }
    if let Some(value) = xml::attr(node, "u") {
        properties.underline = Some(
            match value {
                "sng" => "single",
                "dbl" => "double",
                "heavy" => "thick",
                "none" => "none",
                other => other,
            }
            .to_string(),
        );
    }
    if let Some(value) = xml::attr(node, "strike") {
        properties.strike = Some(value != "noStrike");
    }
    if let Some(value) = xml::attr(node, "sz").and_then(units::parse_i64) {
        // DrawingML font size is already in hundredths of a point.
        properties.size = Some(value);
    }
    if let Some(value) = xml::attr(node, "spc").and_then(units::parse_i64) {
        properties.spacing = Some(value);
    }
    if let Some(value) = xml::attr(node, "baseline").and_then(units::parse_i64) {
        properties.vert_align = Some(
            if value > 0 {
                "superscript"
            } else if value < 0 {
                "subscript"
            } else {
                "baseline"
            }
            .to_string(),
        );
    }
    if let Some(color) = xml::child(node, "solidFill")
        .and_then(|fill| xml::child(fill, "srgbClr"))
        .and_then(xml::val)
        .and_then(normalize_hex)
    {
        properties.color = Some(color);
    }
    if let Some(font) = xml::child(node, "latin").and_then(|node| xml::attr(node, "typeface")) {
        properties.font = Some(font.to_string());
    }

    properties
}

fn normalize_align(value: &str) -> String {
    match value {
        "l" => "left",
        "ctr" => "center",
        "r" => "right",
        "just" => "justify",
        "dist" => "distribute",
        other => other,
    }
    .to_string()
}

fn shape_position(node: Node<'_, '_>) -> Option<Rect> {
    let transform = xml::descendant(node, "xfrm")?;
    let offset = xml::child(transform, "off");
    let extent = xml::child(transform, "ext");
    Some(Rect {
        x: offset
            .and_then(|node| xml::attr(node, "x"))
            .and_then(units::parse_i64)
            .map(units::emu)
            .unwrap_or(0),
        y: offset
            .and_then(|node| xml::attr(node, "y"))
            .and_then(units::parse_i64)
            .map(units::emu)
            .unwrap_or(0),
        w: extent
            .and_then(|node| xml::attr(node, "cx"))
            .and_then(units::parse_i64)
            .map(units::emu)
            .unwrap_or(0),
        h: extent
            .and_then(|node| xml::attr(node, "cy"))
            .and_then(units::parse_i64)
            .map(units::emu)
            .unwrap_or(0),
    })
}

fn parse_notes(bytes: &[u8], unsupported: &mut BTreeSet<String>) -> Result<Vec<Block>> {
    let document = xml::parse(bytes)?;
    let mut blocks = Vec::new();
    for tx_body in document
        .root_element()
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "txBody")
    {
        for paragraph in collect_paragraphs(tx_body, unsupported) {
            if !paragraph.runs.is_empty() {
                blocks.push(Block::Paragraph(paragraph));
            }
        }
    }
    Ok(blocks)
}

fn package_unsupported(package: &MemoryPackage) -> BTreeSet<String> {
    let mut unsupported = BTreeSet::new();
    for name in package.names() {
        if name.starts_with("ppt/charts/") {
            unsupported.insert("charts".to_string());
        }
        if name.starts_with("ppt/diagrams/") {
            unsupported.insert("smartart".to_string());
        }
        if name.starts_with("ppt/embeddings/") {
            unsupported.insert("embedded-objects".to_string());
        }
        if name.starts_with("ppt/comments/") {
            unsupported.insert("comments".to_string());
        }
    }
    unsupported
}

fn scan_slide_unsupported(root: Node<'_, '_>, unsupported: &mut BTreeSet<String>) {
    for node in root.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "timing" => {
                unsupported.insert("animations".to_string());
            }
            "transition" => {
                unsupported.insert("transitions".to_string());
            }
            "chart" => {
                unsupported.insert("charts".to_string());
            }
            "videoFile" | "audioFile" => {
                unsupported.insert("media-playback".to_string());
            }
            _ => {}
        }
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

    const PRESENTATION: &str = r#"<p:presentation xmlns:a="a" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="p">
      <p:sldIdLst>
        <p:sldId id="256" r:id="rId2"/>
        <p:sldId id="257" r:id="rId5"/>
      </p:sldIdLst>
      <p:sldSz cx="12192000" cy="6858000"/>
    </p:presentation>"#;

    const PRESENTATION_RELS: &str = r#"<Relationships xmlns="p">
      <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
      <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>
    </Relationships>"#;

    const SLIDE1: &str = r#"<p:sld xmlns:a="a" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="p" show="1">
      <p:cSld name="Intro"><p:spTree>
        <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
        <p:grpSpPr/>
        <p:sp>
          <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
          <p:spPr><a:xfrm><a:off x="1270000" y="635000"/><a:ext cx="6350000" cy="1270000"/></a:xfrm></p:spPr>
          <p:txBody><a:bodyPr/><a:lstStyle/>
            <a:p><a:pPr algn="ctr" lvl="0"/><a:r><a:rPr b="1" sz="2400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Hello</a:t></a:r></a:p>
            <a:p><a:r><a:t>Second</a:t></a:r></a:p>
          </p:txBody>
        </p:sp>
        <p:pic>
          <p:nvPicPr><p:cNvPr id="3" name="Logo" descr="logo"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
          <p:blipFill><a:blip r:embed="rId2"/></p:blipFill>
          <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm></p:spPr>
        </p:pic>
        <p:graphicFrame>
          <p:nvGraphicFramePr><p:cNvPr id="4" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
          <p:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="1270000"/></p:xfrm>
          <a:graphic><a:graphicData><a:tbl>
            <a:tblGrid><a:gridCol w="1270000"/><a:gridCol w="1270000"/></a:tblGrid>
            <a:tr h="254000">
              <a:tc><a:txBody><a:p><a:r><a:t>A</a:t></a:r></a:p></a:txBody></a:tc>
              <a:tc gridSpan="1"><a:txBody><a:p><a:r><a:t>B</a:t></a:r></a:p></a:txBody></a:tc>
            </a:tr>
          </a:tbl></a:graphicData></a:graphic>
        </p:graphicFrame>
      </p:spTree></p:cSld>
    </p:sld>"#;

    const SLIDE1_RELS: &str = r#"<Relationships xmlns="p">
      <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
      <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/>
    </Relationships>"#;

    const NOTES1: &str = r#"<p:notes xmlns:a="a" xmlns:p="p"><p:cSld><p:spTree>
      <p:sp><p:nvSpPr><p:cNvPr id="1" name="Notes"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr>
        <p:txBody><a:p><a:r><a:t>Speaker note</a:t></a:r></a:p></p:txBody>
      </p:sp>
    </p:spTree></p:cSld></p:notes>"#;

    const SLIDE2: &str = r#"<p:sld xmlns:a="a" xmlns:p="p" show="0">
      <p:cSld name="Hidden"><p:spTree>
        <p:sp><p:nvSpPr><p:cNvPr id="1" name="Text"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
          <p:txBody><a:p><a:r><a:t>secret</a:t></a:r></a:p></p:txBody>
        </p:sp>
      </p:spTree></p:cSld>
    </p:sld>"#;

    fn presentation() -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&10u32.to_be_bytes());
        png.extend_from_slice(&20u32.to_be_bytes());
        let parts: Vec<(&str, &[u8])> = vec![
            ("ppt/presentation.xml", PRESENTATION.as_bytes()),
            (
                "ppt/_rels/presentation.xml.rels",
                PRESENTATION_RELS.as_bytes(),
            ),
            ("ppt/slides/slide1.xml", SLIDE1.as_bytes()),
            ("ppt/slides/_rels/slide1.xml.rels", SLIDE1_RELS.as_bytes()),
            ("ppt/slides/slide2.xml", SLIDE2.as_bytes()),
            ("ppt/notesSlides/notesSlide1.xml", NOTES1.as_bytes()),
            ("ppt/media/image1.png", &png),
        ];
        package(&parts)
    }

    #[test]
    fn parses_slides_shapes_text_tables_and_images() {
        let (document, media_bytes) = parse(&presentation()).unwrap();
        assert_eq!(document.sections.len(), 2);
        assert_eq!(document.sections[0].name.as_deref(), Some("Intro"));
        assert_eq!(
            document.sections[0].page.as_ref().unwrap().width,
            Some(96000)
        );

        let blocks = &document.sections[0].blocks;
        assert_eq!(blocks.len(), 3);

        let Block::Shape(title) = &blocks[0] else {
            panic!("expected shape");
        };
        assert_eq!(title.name.as_deref(), Some("Title"));
        assert_eq!(
            title.position,
            Some(Rect {
                x: 10000,
                y: 5000,
                w: 50000,
                h: 10000
            })
        );
        assert_eq!(title.paragraphs.len(), 2);
        let first = &title.paragraphs[0];
        assert_eq!(
            first.direct.as_ref().unwrap().align.as_deref(),
            Some("center")
        );
        assert_eq!(first.direct.as_ref().unwrap().level, Some(0));
        let Run::Text(run) = &first.runs[0] else {
            panic!("expected text");
        };
        assert_eq!(run.text, "Hello");
        let properties = run.direct.as_ref().unwrap();
        assert_eq!(properties.bold, Some(true));
        assert_eq!(properties.size, Some(2400));
        assert_eq!(properties.color.as_deref(), Some("#FF0000"));

        let Block::Shape(picture) = &blocks[1] else {
            panic!("expected picture");
        };
        let image = picture.image.as_ref().unwrap();
        assert_eq!(image.alt.as_deref(), Some("logo"));
        assert_eq!(image.width, Some(10000));
        assert_eq!(document.media[&image.sha256].width, Some(10));
        assert_eq!(media_bytes.len(), 1);

        let Block::Shape(frame) = &blocks[2] else {
            panic!("expected table frame");
        };
        let table = frame.table.as_ref().unwrap();
        assert_eq!(table.grid, vec![10000, 10000]);
        assert_eq!(table.rows[0].direct.as_ref().unwrap().height, Some(2000));
        assert_eq!(table.rows[0].cells.len(), 2);

        assert_eq!(document.sections[0].notes.len(), 1);
        let Block::Paragraph(note) = &document.sections[0].notes[0] else {
            panic!("expected note paragraph");
        };
        let Run::Text(note_run) = &note.runs[0] else {
            panic!("expected note text");
        };
        assert_eq!(note_run.text, "Speaker note");
    }

    #[test]
    fn marks_hidden_slides() {
        let (document, _) = parse(&presentation()).unwrap();
        assert_eq!(document.sections[1].name.as_deref(), Some("Hidden"));
        assert_eq!(document.sections[1].hidden, Some(true));
    }
}
