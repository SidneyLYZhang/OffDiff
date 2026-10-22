//! SpreadsheetML worksheet parsing.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use roxmltree::Node;

use crate::error::Result;
use crate::model::{
    Block, Cell, ColProps, Document, Format, MergeRange, Row, Scalar, Section, SectionKind, Style,
    StyleKind,
};
use crate::ooxml::rels::{self, Relationships};
use crate::ooxml::{units, xml, Package};

use super::styles::Styles;

type MemoryPackage = Package<Cursor<Vec<u8>>>;

pub fn parse(bytes: &[u8]) -> Result<(Document, BTreeMap<String, Vec<u8>>)> {
    let mut package = MemoryPackage::from_bytes(bytes)?;
    let workbook_xml = package.read("xl/workbook.xml")?;
    let relationships = match package.read_opt("xl/_rels/workbook.xml.rels")? {
        Some(bytes) => rels::parse(&bytes)?,
        None => Relationships::new(),
    };
    let shared = read_shared_strings(&mut package)?;
    let styles_xml = package.read_opt("xl/styles.xml")?;
    let styles = Styles::parse(styles_xml.as_deref())?;
    let mut properties = crate::ooxml::read_properties(&mut package)?;
    let mut unsupported = package_unsupported(&package);

    let workbook = xml::parse(&workbook_xml)?;
    let root = workbook.root_element();
    let date1904 = xml::child(root, "workbookPr")
        .and_then(|node| xml::attr(node, "date1904"))
        .map(|value| value == "1" || value == "true")
        .unwrap_or(false);

    if let Some(names) = xml::child(root, "definedNames") {
        if names.children().any(|node| node.is_element()) {
            unsupported.insert("defined-names".to_string());
        }
    }

    let mut sections = Vec::new();
    let mut used_styles: BTreeSet<usize> = BTreeSet::new();

    if let Some(sheets) = xml::child(root, "sheets") {
        for sheet in xml::children(sheets, "sheet") {
            let name = xml::attr(sheet, "name").unwrap_or("Sheet").to_string();
            let hidden = matches!(
                xml::attr(sheet, "state"),
                Some("hidden") | Some("veryHidden")
            );
            let Some(id) = xml::attr_ns(sheet, "relationships", "id") else {
                continue;
            };
            let Some(relationship) = relationships.get(id) else {
                continue;
            };
            let part = rels::resolve("xl/workbook.xml", &relationship.target);
            let sheet_xml = package.read(&part)?;
            let (blocks, merges, cols) = parse_sheet(
                &sheet_xml,
                &shared,
                &styles,
                date1904,
                &mut used_styles,
                &mut unsupported,
            )?;
            sections.push(Section {
                kind: SectionKind::Sheet,
                name: Some(name),
                hidden: hidden.then_some(true),
                page: None,
                cols,
                merges,
                blocks,
                notes: Vec::new(),
            });
        }
    }

    let mut document = Document::new(Format::Xlsx);
    properties.unsupported = unsupported;
    document.properties = properties;
    for index in used_styles {
        if styles.is_empty(index) {
            continue;
        }
        if let Some(properties) = styles.xf(index) {
            let mut style = Style::new(StyleKind::Cell);
            style.xf = Some(properties.clone());
            document.styles.insert(format!("xf{index}"), style);
        }
    }
    document.sections = sections;

    Ok((document, BTreeMap::new()))
}

fn read_shared_strings(package: &mut MemoryPackage) -> Result<Vec<String>> {
    let Some(bytes) = package.read_opt("xl/sharedStrings.xml")? else {
        return Ok(Vec::new());
    };
    let document = xml::parse(&bytes)?;
    let mut strings = Vec::new();
    for entry in xml::children(document.root_element(), "si") {
        let mut text = String::new();
        for node in entry
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "t")
        {
            text.push_str(&xml::text(node));
        }
        strings.push(text);
    }
    Ok(strings)
}

fn parse_sheet(
    bytes: &[u8],
    shared: &[String],
    styles: &Styles,
    date1904: bool,
    used_styles: &mut BTreeSet<usize>,
    unsupported: &mut BTreeSet<String>,
) -> Result<(Vec<Block>, Vec<MergeRange>, Vec<ColProps>)> {
    let document = xml::parse(bytes)?;
    let root = document.root_element();

    for node in root.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "conditionalFormatting" => {
                unsupported.insert("conditional-formatting".to_string());
            }
            "dataValidations" => {
                unsupported.insert("data-validation".to_string());
            }
            "hyperlinks" => {
                unsupported.insert("hyperlinks".to_string());
            }
            "drawing" | "legacyDrawing" => {
                unsupported.insert("images".to_string());
            }
            "tableParts" => {
                unsupported.insert("tables".to_string());
            }
            _ => {}
        }
    }

    let mut blocks = Vec::new();
    let mut previous_row = 0u32;
    if let Some(sheet_data) = xml::child(root, "sheetData") {
        for row_node in xml::children(sheet_data, "row") {
            let row_index = xml::attr(row_node, "r")
                .and_then(units::parse_i64)
                .map(|value| value.max(0) as u32)
                .unwrap_or(previous_row + 1);
            previous_row = row_index;
            let height = xml::attr(row_node, "ht")
                .and_then(units::parse_f64)
                .map(|points| (points * 100.0).round() as i64);
            let hidden = matches!(xml::attr(row_node, "hidden"), Some("1") | Some("true"));

            let mut cells = Vec::new();
            let mut previous_col = 0u32;
            for cell_node in xml::children(row_node, "c") {
                let reference = xml::attr(cell_node, "r");
                let col = reference
                    .and_then(parse_reference)
                    .map(|(col, _)| col)
                    .unwrap_or(previous_col + 1);
                previous_col = col;

                let style_index = xml::attr(cell_node, "s")
                    .and_then(units::parse_i64)
                    .map(|value| value.max(0) as usize);
                let style = style_index
                    .filter(|index| !styles.is_empty(*index))
                    .map(|index| {
                        used_styles.insert(index);
                        format!("xf{index}")
                    });

                let formula = xml::child(cell_node, "f")
                    .map(xml::text)
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());

                let kind = xml::attr(cell_node, "t").unwrap_or("n");
                let raw = xml::child(cell_node, "v")
                    .map(xml::text)
                    .unwrap_or_default();

                let value = match kind {
                    "s" => raw
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| shared.get(index))
                        .map(|text| Scalar::Text(text.clone())),
                    "inlineStr" => xml::child(cell_node, "is")
                        .map(|inline| {
                            inline
                                .descendants()
                                .filter(|node| node.is_element() && node.tag_name().name() == "t")
                                .map(|node| xml::text(node))
                                .collect::<String>()
                        })
                        .filter(|text| !text.is_empty())
                        .map(Scalar::Text),
                    "str" | "e" | "d" => {
                        if raw.is_empty() {
                            None
                        } else {
                            Some(Scalar::Text(raw.clone()))
                        }
                    }
                    "b" => match raw.trim() {
                        "1" | "true" => Some(Scalar::Bool(true)),
                        "0" | "false" => Some(Scalar::Bool(false)),
                        _ => None,
                    },
                    _ => {
                        let trimmed = raw.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            trimmed.parse::<f64>().ok().map(|number| {
                                if let Some(index) = style_index {
                                    if styles.is_date(index) {
                                        if let Some(date) = excel_date(number, date1904) {
                                            return Scalar::Text(date);
                                        }
                                    }
                                }
                                Scalar::Number(number)
                            })
                        }
                    }
                };

                if value.is_none() && formula.is_none() && style.is_none() {
                    continue;
                }
                cells.push(Cell {
                    col,
                    value,
                    style,
                    formula,
                });
            }

            if cells.is_empty() && height.is_none() && !hidden {
                continue;
            }
            blocks.push(Block::Row(Row {
                index: row_index,
                height,
                hidden: hidden.then_some(true),
                cells,
            }));
        }
    }

    let mut merges = Vec::new();
    if let Some(merge_cells) = xml::child(root, "mergeCells") {
        for merge in xml::children(merge_cells, "mergeCell") {
            if let Some(range) = xml::attr(merge, "ref").and_then(parse_range) {
                merges.push(range);
            }
        }
    }

    let mut cols = Vec::new();
    if let Some(cols_node) = xml::child(root, "cols") {
        for col in xml::children(cols_node, "col") {
            let index = xml::attr(col, "min")
                .and_then(units::parse_i64)
                .map(|value| value.max(0) as u32)
                .unwrap_or(0);
            if index == 0 {
                continue;
            }
            let max = xml::attr(col, "max")
                .and_then(units::parse_i64)
                .map(|value| value.max(0) as u32)
                .unwrap_or(index);
            let span = max.saturating_sub(index) + 1;
            let width = xml::attr(col, "width").map(str::to_string);
            let hidden = matches!(xml::attr(col, "hidden"), Some("1") | Some("true"));
            if width.is_none() && !hidden {
                continue;
            }
            cols.push(ColProps {
                index,
                span,
                width,
                hidden: hidden.then_some(true),
            });
        }
    }

    Ok((blocks, merges, cols))
}

fn package_unsupported(package: &MemoryPackage) -> BTreeSet<String> {
    let mut unsupported = BTreeSet::new();
    for name in package.names() {
        if name.starts_with("xl/charts/") {
            unsupported.insert("charts".to_string());
        }
        if name.starts_with("xl/drawings/") {
            unsupported.insert("images".to_string());
        }
        if name.starts_with("xl/pivotTables/") || name.starts_with("xl/pivotCache/") {
            unsupported.insert("pivot-tables".to_string());
        }
        if name.starts_with("xl/comments") {
            unsupported.insert("comments".to_string());
        }
    }
    unsupported
}

fn parse_reference(reference: &str) -> Option<(u32, u32)> {
    let reference = reference.replace('$', "");
    let split = reference.find(|character: char| character.is_ascii_digit())?;
    let (letters, digits) = reference.split_at(split);
    let col = column_index(letters)?;
    let row = digits.parse::<u32>().ok()?;
    if row == 0 || col == 0 {
        return None;
    }
    Some((col, row))
}

fn column_index(letters: &str) -> Option<u32> {
    if letters.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for character in letters.chars() {
        let digit = character.to_ascii_uppercase().to_digit(36)?;
        if !(10..=35).contains(&digit) {
            return None;
        }
        value = value.checked_mul(26)?.checked_add(digit - 9)?;
    }
    Some(value)
}

fn parse_range(reference: &str) -> Option<MergeRange> {
    let (start, end) = reference.split_once(':')?;
    let (c1, r1) = parse_reference(start)?;
    let (c2, r2) = parse_reference(end)?;
    Some(MergeRange { r1, r2, c1, c2 })
}

/// Convert an Excel serial number into an ISO date (or date-time) string.
///
/// Excel stores dates as days since 1899-12-30 (1900 system) or 1904-01-01
/// (1904 system). Serial 60, the fictitious 1900-02-29, is not special-cased;
/// it only matters for documents authored before March 1900.
fn excel_date(serial: f64, date1904: bool) -> Option<String> {
    if !serial.is_finite() || serial < 0.0 {
        return None;
    }
    let offset = if date1904 { 24107.0 } else { 25569.0 };
    let total_seconds = ((serial - offset) * 86400.0).round() as i64;
    let days = total_seconds.div_euclid(86400);
    let seconds = total_seconds.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    if !(1900..=9999).contains(&year) {
        return None;
    }
    if seconds == 0 {
        Some(format!("{year:04}-{month:02}-{day:02}"))
    } else {
        Some(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        ))
    }
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month, day)
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

    const WORKBOOK: &str = r#"<workbook xmlns="s" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
      <workbookPr/>
      <sheets>
        <sheet name="Data" sheetId="1" r:id="rId1"/>
        <sheet name="Hidden" sheetId="2" state="hidden" r:id="rId4"/>
      </sheets>
    </workbook>"#;

    const RELS: &str = r#"<Relationships xmlns="p">
      <Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/>
      <Relationship Id="rId4" Type="worksheet" Target="worksheets/sheet2.xml"/>
    </Relationships>"#;

    const SHARED: &str = r#"<sst xmlns="s">
      <si><t>Name</t></si><si><t>Value</t></si><si><t>alpha</t></si>
    </sst>"#;

    const STYLES: &str = r#"<styleSheet xmlns="s">
      <numFmts><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts>
      <fonts><font><sz val="11"/><name val="Calibri"/></font><font><b/><color rgb="FFFF0000"/></font></fonts>
      <fills><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/></patternFill></fill></fills>
      <borders><border/><border><left style="thin"><color rgb="FF000000"/></left></border></borders>
      <cellXfs>
        <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
        <xf numFmtId="164" fontId="1" fillId="1" borderId="1" xfId="0"><alignment horizontal="center"/></xf>
      </cellXfs>
    </styleSheet>"#;

    const SHEET1: &str = r#"<worksheet xmlns="s">
      <cols>
        <col min="1" max="2" width="12.5" customWidth="1"/>
        <col min="4" max="4" hidden="1"/>
      </cols>
      <sheetData>
        <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
        <row r="2" ht="24" hidden="1"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>42.5</v></c></row>
        <row r="3"><c r="A3" s="1"><v>45658</v></c><c r="B3"><f>SUM(B2:B2)</f><v>42.5</v></c></row>
        <row r="4"><c r="A4" t="inlineStr"><is><t>inline</t></is></c><c r="B4" t="b"><v>1</v></c></row>
      </sheetData>
      <mergeCells><mergeCell ref="A2:A3"/></mergeCells>
      <conditionalFormatting sqref="A1"><cfRule type="cellIs"/></conditionalFormatting>
    </worksheet>"#;

    const SHEET2: &str = r#"<worksheet xmlns="s"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>"#;

    fn workbook() -> Vec<u8> {
        package(&[
            ("xl/workbook.xml", WORKBOOK.as_bytes()),
            ("xl/_rels/workbook.xml.rels", RELS.as_bytes()),
            ("xl/sharedStrings.xml", SHARED.as_bytes()),
            ("xl/styles.xml", STYLES.as_bytes()),
            ("xl/worksheets/sheet1.xml", SHEET1.as_bytes()),
            ("xl/worksheets/sheet2.xml", SHEET2.as_bytes()),
        ])
    }

    #[test]
    fn parses_sheets_rows_cells_and_styles() {
        let (document, media) = parse(&workbook()).unwrap();
        assert!(media.is_empty());
        assert_eq!(document.sections.len(), 2);

        let data = &document.sections[0];
        assert_eq!(data.name.as_deref(), Some("Data"));
        assert!(data.hidden.is_none());
        assert_eq!(data.merges.len(), 1);
        assert_eq!(
            data.merges[0],
            MergeRange {
                r1: 2,
                r2: 3,
                c1: 1,
                c2: 1
            }
        );
        assert_eq!(data.cols.len(), 2);
        assert_eq!(data.cols[0].index, 1);
        assert_eq!(data.cols[0].span, 2);
        assert_eq!(data.cols[0].width.as_deref(), Some("12.5"));
        assert_eq!(data.cols[1].hidden, Some(true));

        assert_eq!(data.blocks.len(), 4);
        let Block::Row(row) = &data.blocks[1] else {
            panic!("expected row");
        };
        assert_eq!(row.index, 2);
        assert_eq!(row.height, Some(2400));
        assert_eq!(row.hidden, Some(true));

        let Block::Row(row3) = &data.blocks[2] else {
            panic!("expected row");
        };
        let a3 = &row3.cells[0];
        assert_eq!(a3.col, 1);
        assert_eq!(a3.style.as_deref(), Some("xf1"));
        assert!(matches!(&a3.value, Some(Scalar::Text(text)) if text == "2025-01-01"));
        let b3 = &row3.cells[1];
        assert_eq!(b3.formula.as_deref(), Some("SUM(B2:B2)"));
        assert!(matches!(b3.value, Some(Scalar::Number(value)) if value == 42.5));

        let Block::Row(row4) = &data.blocks[3] else {
            panic!("expected row");
        };
        assert!(matches!(&row4.cells[0].value, Some(Scalar::Text(text)) if text == "inline"));
        assert!(matches!(row4.cells[1].value, Some(Scalar::Bool(true))));

        assert!(document.styles.contains_key("xf1"));
        let style = &document.styles["xf1"];
        assert_eq!(
            style.xf.as_ref().unwrap().num_fmt.as_deref(),
            Some("yyyy-mm-dd")
        );
        assert!(document
            .properties
            .unsupported
            .contains("conditional-formatting"));
    }

    #[test]
    fn marks_hidden_sheets() {
        let (document, _) = parse(&workbook()).unwrap();
        assert_eq!(document.sections[1].name.as_deref(), Some("Hidden"));
        assert_eq!(document.sections[1].hidden, Some(true));
    }

    #[test]
    fn converts_excel_serial_dates() {
        assert_eq!(excel_date(25569.0, false).as_deref(), Some("1970-01-01"));
        assert_eq!(
            excel_date(25569.5, false).as_deref(),
            Some("1970-01-01T12:00:00")
        );
        assert_eq!(excel_date(45658.0, false).as_deref(), Some("2025-01-01"));
        assert_eq!(excel_date(24107.0, true).as_deref(), Some("1970-01-01"));
        assert!(excel_date(-1.0, false).is_none());
    }

    #[test]
    fn parses_cell_references() {
        assert_eq!(parse_reference("A1"), Some((1, 1)));
        assert_eq!(parse_reference("B3"), Some((2, 3)));
        assert_eq!(parse_reference("AA10"), Some((27, 10)));
        assert_eq!(parse_reference("$C$4"), Some((3, 4)));
        assert_eq!(parse_reference("1A"), None);
        assert_eq!(
            parse_range("A2:B3"),
            Some(MergeRange {
                r1: 2,
                r2: 3,
                c1: 1,
                c2: 2
            })
        );
    }

    #[test]
    fn rejects_package_without_workbook() {
        let bytes = package(&[("xl/worksheets/sheet1.xml", SHEET2.as_bytes())]);
        assert!(parse(&bytes).is_err());
    }
}
