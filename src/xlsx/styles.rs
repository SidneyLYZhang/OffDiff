//! `xl/styles.xml`: cell format records.

use std::collections::BTreeMap;

use roxmltree::Node;

use crate::error::Result;
use crate::model::{XfAlign, XfBorder, XfFont, XfProps};
use crate::ooxml::{units, xml};

#[derive(Debug, Clone, Default)]
pub struct Styles {
    xfs: Vec<XfProps>,
    date_xfs: Vec<bool>,
}

impl Styles {
    pub fn parse(bytes: Option<&[u8]>) -> Result<Styles> {
        let Some(bytes) = bytes else {
            return Ok(Styles::default());
        };
        let document = xml::parse(bytes)?;
        let root = document.root_element();

        let mut custom_formats: BTreeMap<u32, String> = BTreeMap::new();
        if let Some(formats) = xml::child(root, "numFmts") {
            for format in xml::children(formats, "numFmt") {
                if let (Some(id), Some(code)) = (
                    xml::attr(format, "numFmtId").and_then(units::parse_i64),
                    xml::attr(format, "formatCode"),
                ) {
                    custom_formats.insert(id as u32, code.to_string());
                }
            }
        }

        let fonts: Vec<XfFont> = xml::child(root, "fonts")
            .map(|fonts| xml::children(fonts, "font").map(parse_font).collect())
            .unwrap_or_default();
        let fills: Vec<Option<String>> = xml::child(root, "fills")
            .map(|fills| xml::children(fills, "fill").map(parse_fill).collect())
            .unwrap_or_default();
        let borders: Vec<XfBorder> = xml::child(root, "borders")
            .map(|borders| xml::children(borders, "border").map(parse_border).collect())
            .unwrap_or_default();

        let mut xfs = Vec::new();
        let mut date_xfs = Vec::new();
        if let Some(cell_xfs) = xml::child(root, "cellXfs") {
            for xf in xml::children(cell_xfs, "xf") {
                let num_fmt_id = xml::attr(xf, "numFmtId")
                    .and_then(units::parse_i64)
                    .unwrap_or(0) as u32;
                let code = num_fmt_code(num_fmt_id, &custom_formats);
                let mut properties = XfProps::default();
                if num_fmt_id != 0 {
                    properties.num_fmt = code.clone();
                }

                let font_id = xml::attr(xf, "fontId")
                    .and_then(units::parse_i64)
                    .unwrap_or(0);
                if font_id > 0 {
                    if let Some(font) = fonts.get(font_id as usize) {
                        if *font != XfFont::default() {
                            properties.font = Some(font.clone());
                        }
                    }
                }

                let fill_id = xml::attr(xf, "fillId")
                    .and_then(units::parse_i64)
                    .unwrap_or(0);
                if fill_id > 0 {
                    if let Some(Some(fill)) = fills.get(fill_id as usize) {
                        properties.fill = Some(fill.clone());
                    }
                }

                let border_id = xml::attr(xf, "borderId")
                    .and_then(units::parse_i64)
                    .unwrap_or(0);
                if border_id > 0 {
                    if let Some(border) = borders.get(border_id as usize) {
                        if *border != XfBorder::default() {
                            properties.border = Some(border.clone());
                        }
                    }
                }

                if let Some(alignment) = xml::child(xf, "alignment") {
                    let align = XfAlign {
                        horizontal: xml::attr(alignment, "horizontal").map(str::to_string),
                        vertical: xml::attr(alignment, "vertical").map(str::to_string),
                        wrap: xml::attr(alignment, "wrapText")
                            .map(|value| value == "1" || value == "true"),
                    };
                    if align != XfAlign::default() {
                        properties.align = Some(align);
                    }
                }

                date_xfs.push(is_date_format(num_fmt_id, code.as_deref()));
                xfs.push(properties);
            }
        }

        Ok(Styles { xfs, date_xfs })
    }

    pub fn xf(&self, index: usize) -> Option<&XfProps> {
        self.xfs.get(index)
    }

    pub fn is_date(&self, index: usize) -> bool {
        self.date_xfs.get(index).copied().unwrap_or(false)
    }

    pub fn is_empty(&self, index: usize) -> bool {
        self.xf(index).map(XfProps::is_empty).unwrap_or(true)
    }
}

fn parse_font(node: Node<'_, '_>) -> XfFont {
    let mut font = XfFont::default();
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "b" => font.bold = xml::on_off(child),
            "i" => font.italic = xml::on_off(child),
            "u" => font.underline = xml::on_off(child),
            "sz" => {
                if let Some(value) = xml::val(child).and_then(units::parse_f64) {
                    font.size = Some((value * 100.0).round() as i64);
                }
            }
            "color" => font.color = color_from(child),
            "name" => font.name = xml::val(child).map(str::to_string),
            _ => {}
        }
    }
    font
}

fn parse_fill(node: Node<'_, '_>) -> Option<String> {
    let pattern = xml::child(node, "patternFill")?;
    let kind = xml::attr(pattern, "patternType").unwrap_or("none");
    if kind == "none" {
        return None;
    }
    xml::child(pattern, "fgColor").and_then(color_from)
}

fn parse_border(node: Node<'_, '_>) -> XfBorder {
    XfBorder {
        top: border_side(xml::child(node, "top")),
        bottom: border_side(xml::child(node, "bottom")),
        left: border_side(xml::child(node, "left")),
        right: border_side(xml::child(node, "right")),
    }
}

fn border_side(node: Option<Node<'_, '_>>) -> Option<String> {
    let node = node?;
    let style = xml::attr(node, "style")?;
    Some(match xml::child(node, "color").and_then(color_from) {
        Some(color) => format!("{style} {color}"),
        None => style.to_string(),
    })
}

fn color_from(node: Node<'_, '_>) -> Option<String> {
    let rgb = xml::attr(node, "rgb")?.trim();
    let hex = if rgb.len() == 8 { &rgb[2..] } else { rgb };
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", hex.to_ascii_uppercase()))
}

fn num_fmt_code(id: u32, custom: &BTreeMap<u32, String>) -> Option<String> {
    if let Some(code) = custom.get(&id) {
        return Some(code.clone());
    }
    let code = match id {
        0 => return None,
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        49 => "@",
        _ => return None,
    };
    Some(code.to_string())
}

fn is_date_format(id: u32, code: Option<&str>) -> bool {
    if matches!(id, 14..=22 | 27..=36 | 45..=47 | 50..=58) {
        return true;
    }
    let Some(code) = code else {
        return false;
    };

    let mut cleaned = String::new();
    let mut quoted = false;
    let mut bracketed = false;
    let mut escaped = false;
    for character in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            '[' if !quoted => bracketed = true,
            ']' if !quoted => bracketed = false,
            _ if !quoted && !bracketed => cleaned.push(character.to_ascii_lowercase()),
            _ => {}
        }
    }

    cleaned
        .chars()
        .any(|character| matches!(character, 'y' | 'd' | 'h' | 's'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<styleSheet xmlns="s">
  <numFmts><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts>
  <fonts>
    <font><sz val="11"/><color rgb="FF000000"/><name val="Calibri"/></font>
    <font><b/><sz val="12"/><color rgb="FFFF0000"/></font>
  </fonts>
  <fills>
    <fill><patternFill patternType="none"/></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/></patternFill></fill>
  </fills>
  <borders>
    <border/>
    <border><left style="thin"><color rgb="FF000000"/></left></border>
  </borders>
  <cellXfs>
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="164" fontId="1" fillId="1" borderId="1" xfId="0">
      <alignment horizontal="center" wrapText="1"/>
    </xf>
  </cellXfs>
</styleSheet>"#;

    #[test]
    fn parses_cell_formats() {
        let styles = Styles::parse(Some(STYLES.as_bytes())).unwrap();
        assert!(styles.is_empty(0));
        assert!(!styles.is_date(0));

        let xf = styles.xf(1).unwrap();
        assert_eq!(xf.num_fmt.as_deref(), Some("yyyy-mm-dd"));
        let font = xf.font.as_ref().unwrap();
        assert_eq!(font.bold, Some(true));
        assert_eq!(font.size, Some(1200));
        assert_eq!(font.color.as_deref(), Some("#FF0000"));
        assert_eq!(xf.fill.as_deref(), Some("#FFFF00"));
        assert_eq!(
            xf.border.as_ref().unwrap().left.as_deref(),
            Some("thin #000000")
        );
        assert_eq!(
            xf.align.as_ref().unwrap().horizontal.as_deref(),
            Some("center")
        );
        assert_eq!(xf.align.as_ref().unwrap().wrap, Some(true));
        assert!(styles.is_date(1));
    }

    #[test]
    fn detects_date_codes_outside_quotes() {
        assert!(is_date_format(0, Some("yyyy-mm-dd")));
        assert!(is_date_format(0, Some("hh:mm:ss")));
        assert!(!is_date_format(0, Some("0.00")));
        assert!(!is_date_format(0, Some("\"yyyy\"0")));
        assert!(!is_date_format(2, Some("0.00")));
    }

    #[test]
    fn missing_styles_part_is_tolerated() {
        let styles = Styles::parse(None).unwrap();
        assert!(styles.xf(0).is_none());
        assert!(!styles.is_date(0));
    }
}
