//! WordprocessingML styles: theme colours and fonts, `docDefaults`, and the
//! `basedOn` inheritance chain.
//!
//! Resolved style entries deliberately exclude `docDefaults`; those live in
//! [`Defaults`] so that a change to the document defaults produces one line of
//! diff instead of rewriting every style.

use std::collections::{BTreeMap, BTreeSet};

use roxmltree::Node;

use crate::error::Result;
use crate::model::{
    Defaults, Indent, NumRef, NumberingLevel, ParaProps, RunProps, Spacing, Style, StyleKind,
};
use crate::ooxml::{units, xml};

/// Theme colours and fonts from `word/theme/theme1.xml`.
#[derive(Debug, Clone, Default)]
pub struct Theme {
    colors: BTreeMap<String, String>,
    fonts: BTreeMap<String, String>,
}

impl Theme {
    pub fn parse(bytes: Option<&[u8]>) -> Result<Theme> {
        let Some(bytes) = bytes else {
            return Ok(Theme::default());
        };
        let document = xml::parse(bytes)?;
        let root = document.root_element();
        let mut theme = Theme::default();

        if let Some(scheme) = xml::descendant(root, "clrScheme") {
            for node in scheme.children().filter(Node::is_element) {
                let name = node.tag_name().name().to_ascii_lowercase();
                let value = xml::child(node, "srgbClr")
                    .and_then(xml::val)
                    .or_else(|| xml::child(node, "sysClr").and_then(|n| xml::attr(n, "lastClr")));
                if let Some(value) = value {
                    if let Some(rgb) = normalize_hex(value) {
                        theme.colors.insert(name, rgb);
                    }
                }
            }
        }

        if let Some(fonts) = xml::descendant(root, "fontScheme") {
            for (tag, prefix) in [("majorFont", "major"), ("minorFont", "minor")] {
                let Some(font) = xml::child(fonts, tag) else {
                    continue;
                };
                for (child_tag, suffix) in [("latin", "Latin"), ("ea", "Ea")] {
                    let Some(node) = xml::child(font, child_tag) else {
                        continue;
                    };
                    let face = xml::attr(node, "typeface").unwrap_or("").trim();
                    if !face.is_empty() {
                        theme
                            .fonts
                            .insert(format!("{prefix}{suffix}"), face.to_string());
                    }
                }
            }
        }

        Ok(theme)
    }

    /// Resolve a `w:themeColor` token, optionally applying `themeTint` /
    /// `themeShade`.
    ///
    /// The tint/shade maths is a deterministic approximation of Word's HSL
    /// transform; it preserves the property that equal inputs compare equal.
    pub fn color(&self, token: &str, tint: Option<&str>, shade: Option<&str>) -> Option<String> {
        let key = canonical_color_token(token);
        let base = self.colors.get(key)?;
        let mut rgb = base.trim_start_matches('#').to_string();
        if let Some(byte) = tint.and_then(hex_byte) {
            rgb = apply_tint(&rgb, byte);
        }
        if let Some(byte) = shade.and_then(hex_byte) {
            rgb = apply_shade(&rgb, byte);
        }
        Some(format!("#{}", rgb.to_ascii_uppercase()))
    }

    pub fn font(&self, token: &str) -> Option<&str> {
        let key = match token {
            "minorHAnsi" | "minorAscii" | "minorBidi" => "minorLatin",
            "majorHAnsi" | "majorAscii" | "majorBidi" => "majorLatin",
            "minorEastAsia" => "minorEa",
            "majorEastAsia" => "majorEa",
            other => other,
        };
        self.fonts.get(key).map(String::as_str)
    }
}

fn canonical_color_token(token: &str) -> &str {
    match token {
        "dark1" | "text1" => "dk1",
        "light1" | "background1" => "lt1",
        "dark2" | "text2" => "dk2",
        "light2" | "background2" => "lt2",
        "hyperlink" => "hlink",
        "followedHyperlink" => "folhlink",
        other => other,
    }
}

pub use crate::ooxml::normalize_hex;

fn hex_byte(value: &str) -> Option<u8> {
    u8::from_str_radix(value.trim(), 16).ok()
}

fn channels(rgb: &str) -> Option<(f64, f64, f64)> {
    if rgb.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&rgb[0..2], 16).ok()? as f64;
    let g = u8::from_str_radix(&rgb[2..4], 16).ok()? as f64;
    let b = u8::from_str_radix(&rgb[4..6], 16).ok()? as f64;
    Some((r, g, b))
}

fn apply_tint(rgb: &str, tint: u8) -> String {
    let Some((r, g, b)) = channels(rgb) else {
        return rgb.to_string();
    };
    let f = tint as f64 / 255.0;
    let mix = |c: f64| (c + (255.0 - c) * f).round().clamp(0.0, 255.0) as u8;
    format!("{:02X}{:02X}{:02X}", mix(r), mix(g), mix(b))
}

fn apply_shade(rgb: &str, shade: u8) -> String {
    let Some((r, g, b)) = channels(rgb) else {
        return rgb.to_string();
    };
    let f = shade as f64 / 255.0;
    let mix = |c: f64| (c * f).round().clamp(0.0, 255.0) as u8;
    format!("{:02X}{:02X}{:02X}", mix(r), mix(g), mix(b))
}

fn resolve_color(node: Node<'_, '_>, theme: &Theme) -> Option<String> {
    if let Some(token) = xml::attr(node, "themeColor") {
        let tint = xml::attr(node, "themeTint");
        let shade = xml::attr(node, "themeShade");
        if let Some(color) = theme.color(token, tint, shade) {
            return Some(color);
        }
    }
    let value = xml::val(node)?;
    if value.eq_ignore_ascii_case("auto") {
        return Some("auto".to_string());
    }
    normalize_hex(value)
}

fn resolve_fonts(node: Node<'_, '_>, theme: &Theme) -> (Option<String>, Option<String>) {
    let mut latin = xml::attr(node, "ascii")
        .or_else(|| xml::attr(node, "hAnsi"))
        .map(str::to_string);
    if latin.is_none() {
        if let Some(token) = xml::attr(node, "asciiTheme").or_else(|| xml::attr(node, "hAnsiTheme"))
        {
            latin = theme.font(token).map(str::to_string);
        }
    }

    let mut east_asian = xml::attr(node, "eastAsia").map(str::to_string);
    if east_asian.is_none() {
        if let Some(token) = xml::attr(node, "eastAsiaTheme") {
            east_asian = theme.font(token).map(str::to_string);
        }
    }

    let duplicate = matches!((&latin, &east_asian), (Some(a), Some(b)) if a == b);
    if duplicate {
        east_asian = None;
    }

    (latin, east_asian)
}

/// Parse a `w:rPr` node into run properties.
pub fn parse_run_props(node: Option<Node<'_, '_>>, theme: &Theme) -> RunProps {
    let mut props = RunProps::default();
    let Some(node) = node else {
        return props;
    };

    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "b" => props.bold = xml::on_off(child),
            "i" => props.italic = xml::on_off(child),
            "strike" | "dstrike" => props.strike = xml::on_off(child),
            "caps" => props.caps = xml::on_off(child),
            "smallCaps" => props.small_caps = xml::on_off(child),
            "vanish" => props.hidden = xml::on_off(child),
            "u" => {
                let value = xml::val(child).unwrap_or("single");
                props.underline = Some(value.to_ascii_lowercase());
            }
            "sz" => {
                if let Some(size) = xml::num(child) {
                    props.size = Some(units::half_points(size));
                }
            }
            "color" => props.color = resolve_color(child, theme),
            "rFonts" => {
                let (latin, east_asian) = resolve_fonts(child, theme);
                props.font = latin;
                props.font_east_asian = east_asian;
            }
            "highlight" => {
                props.highlight = xml::val(child).map(|v| v.to_ascii_lowercase());
            }
            "vertAlign" => {
                props.vert_align = xml::val(child).map(|v| v.to_ascii_lowercase());
            }
            "spacing" => {
                if let Some(value) = xml::num(child) {
                    props.spacing = Some(units::twips(value));
                }
            }
            _ => {}
        }
    }

    props
}

/// Parse a `w:pPr` node into paragraph properties.
pub fn parse_para_props(node: Option<Node<'_, '_>>) -> ParaProps {
    let mut props = ParaProps::default();
    let Some(node) = node else {
        return props;
    };

    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "jc" => {
                props.align = xml::val(child).map(normalize_align);
            }
            "ind" => {
                props.indent = Some(Indent {
                    left: xml::attr(child, "left")
                        .or_else(|| xml::attr(child, "start"))
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    right: xml::attr(child, "right")
                        .or_else(|| xml::attr(child, "end"))
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    first_line: xml::attr(child, "firstLine")
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    hanging: xml::attr(child, "hanging")
                        .and_then(units::parse_i64)
                        .map(units::twips),
                });
            }
            "spacing" => {
                props.spacing = Some(Spacing {
                    before: xml::attr(child, "before")
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    after: xml::attr(child, "after")
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    line: xml::attr(child, "line")
                        .and_then(units::parse_i64)
                        .map(units::twips),
                    line_rule: xml::attr(child, "lineRule")
                        .map(|v| v.trim().replace("atLeast", "at-least").to_ascii_lowercase()),
                });
            }
            "numPr" => {
                let id = xml::child(child, "numId")
                    .and_then(xml::val)
                    .map(str::to_string);
                let level = xml::child(child, "ilvl")
                    .and_then(xml::val)
                    .and_then(units::parse_i64)
                    .unwrap_or(0)
                    .clamp(0, 8) as u8;
                if let Some(id) = id {
                    props.num = Some(NumRef { id, level });
                }
            }
            "pageBreakBefore" => props.page_break_before = xml::on_off(child),
            "keepNext" => props.keep_next = xml::on_off(child),
            "outlineLvl" => {
                if let Some(level) = xml::num(child) {
                    props.outline = Some(level.clamp(0, 8) as u8);
                }
            }
            "shd" => {
                props.shading = xml::attr(child, "fill").and_then(normalize_hex);
            }
            _ => {}
        }
    }

    props
}

fn normalize_align(value: &str) -> String {
    match value.trim() {
        "both" => "justify".to_string(),
        "start" => "start".to_string(),
        "end" => "end".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

#[derive(Debug, Default)]
struct RawStyle {
    kind: Option<StyleKind>,
    name: Option<String>,
    based_on: Option<String>,
    run: RunProps,
    para: ParaProps,
    is_default: bool,
}

/// Resolved styles for one document.
#[derive(Debug, Clone, Default)]
pub struct StyleSheet {
    pub styles: BTreeMap<String, Style>,
    pub defaults: Defaults,
}

impl StyleSheet {
    pub fn parse(styles_xml: Option<&[u8]>, theme: &Theme) -> Result<StyleSheet> {
        let mut sheet = StyleSheet::default();
        let Some(bytes) = styles_xml else {
            return Ok(sheet);
        };
        let document = xml::parse(bytes)?;
        let root = document.root_element();

        let mut base_run = RunProps::default();
        let mut base_para = ParaProps::default();
        if let Some(defaults) = xml::descendant(root, "docDefaults") {
            if let Some(rpr) = xml::child(defaults, "rPrDefault").and_then(|n| xml::child(n, "rPr"))
            {
                base_run = parse_run_props(Some(rpr), theme);
            }
            if let Some(ppr) = xml::child(defaults, "pPrDefault").and_then(|n| xml::child(n, "pPr"))
            {
                base_para = parse_para_props(Some(ppr));
            }
        }
        if !base_run.is_empty() {
            sheet.defaults.run = Some(base_run);
        }
        if !base_para.is_empty() {
            sheet.defaults.para = Some(base_para);
        }

        let mut raws: BTreeMap<String, RawStyle> = BTreeMap::new();
        for node in xml::children(root, "style") {
            let Some(id) = xml::attr(node, "styleId") else {
                continue;
            };
            let kind = match xml::attr(node, "type") {
                Some("paragraph") => StyleKind::Paragraph,
                Some("character") => StyleKind::Character,
                Some("table") => StyleKind::Table,
                _ => continue,
            };
            let raw = RawStyle {
                kind: Some(kind),
                name: xml::child(node, "name")
                    .and_then(xml::val)
                    .map(str::to_string),
                based_on: xml::child(node, "basedOn")
                    .and_then(xml::val)
                    .map(str::to_string),
                run: parse_run_props(xml::child(node, "rPr"), theme),
                para: parse_para_props(xml::child(node, "pPr")),
                is_default: matches!(
                    xml::attr(node, "default"),
                    Some("1") | Some("true") | Some("on")
                ),
            };
            raws.insert(id.to_string(), raw);
        }

        for (id, raw) in &raws {
            if raw.is_default {
                match raw.kind {
                    Some(StyleKind::Paragraph) => {
                        if sheet.defaults.paragraph.is_none() {
                            sheet.defaults.paragraph = Some(id.clone());
                        }
                    }
                    Some(StyleKind::Character) => {
                        if sheet.defaults.character.is_none() {
                            sheet.defaults.character = Some(id.clone());
                        }
                    }
                    Some(StyleKind::Table) if sheet.defaults.table.is_none() => {
                        sheet.defaults.table = Some(id.clone());
                    }
                    _ => {}
                }
            }
        }

        for (id, raw) in &raws {
            let (run, para) = resolve_chain(id, &raws);
            let mut style = Style::new(raw.kind.unwrap_or(StyleKind::Paragraph));
            style.name = raw.name.clone();
            style.based_on = raw.based_on.clone();
            if !run.is_empty() {
                style.run = Some(run);
            }
            if !para.is_empty() {
                style.para = Some(para);
            }
            sheet.styles.insert(id.clone(), style);
        }

        Ok(sheet)
    }

    pub fn style(&self, id: &str) -> Option<&Style> {
        self.styles.get(id)
    }

    /// Effective paragraph properties for a style reference plus direct
    /// formatting.
    pub fn para_effective(&self, style_id: Option<&str>, direct: Option<&ParaProps>) -> ParaProps {
        let mut props = self.defaults.para.clone().unwrap_or_default();
        let id = style_id.or(self.defaults.paragraph.as_deref());
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

    /// Effective run properties: document defaults, paragraph style, character
    /// style, then direct formatting.
    pub fn run_effective(
        &self,
        para_style: Option<&str>,
        run_style: Option<&str>,
        direct: Option<&RunProps>,
    ) -> RunProps {
        let mut props = self.defaults.run.clone().unwrap_or_default();
        let para_id = para_style.or(self.defaults.paragraph.as_deref());
        if let Some(style) = para_id.and_then(|id| self.styles.get(id)) {
            if let Some(run) = &style.run {
                props = props.merged(run);
            }
        }
        let run_id = run_style.or(self.defaults.character.as_deref());
        if let Some(style) = run_id.and_then(|id| self.styles.get(id)) {
            if let Some(run) = &style.run {
                props = props.merged(run);
            }
        }
        if let Some(direct) = direct {
            props = props.merged(direct);
        }
        props
    }

    /// Heading level (1-based) derived from `outlineLvl` or the style name.
    pub fn heading_level(&self, style_id: Option<&str>, direct: Option<&ParaProps>) -> Option<u8> {
        let para = self.para_effective(style_id, direct);
        if let Some(outline) = para.outline {
            return Some(outline.saturating_add(1).min(9));
        }
        let id = style_id?;
        let style = self.styles.get(id)?;
        let name = style.name.as_deref().unwrap_or(id).to_ascii_lowercase();
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

fn resolve_chain(id: &str, raws: &BTreeMap<String, RawStyle>) -> (RunProps, ParaProps) {
    let mut chain: Vec<&RawStyle> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut current = Some(id);
    while let Some(current_id) = current {
        if !seen.insert(current_id) {
            break;
        }
        let Some(raw) = raws.get(current_id) else {
            break;
        };
        chain.push(raw);
        current = raw.based_on.as_deref();
    }

    let mut run = RunProps::default();
    let mut para = ParaProps::default();
    for raw in chain.iter().rev() {
        run = run.merged(&raw.run);
        para = para.merged(&raw.para);
    }
    (run, para)
}

/// Parse `word/numbering.xml` into `"{numId}:{ilvl}"` entries.
pub fn parse_numbering(bytes: Option<&[u8]>) -> Result<BTreeMap<String, NumberingLevel>> {
    let Some(bytes) = bytes else {
        return Ok(BTreeMap::new());
    };
    let document = xml::parse(bytes)?;
    let root = document.root_element();

    let mut abstracts: BTreeMap<String, BTreeMap<String, NumberingLevel>> = BTreeMap::new();
    for abstract_num in xml::children(root, "abstractNum") {
        let Some(abstract_id) = xml::attr(abstract_num, "abstractNumId") else {
            continue;
        };
        let mut levels = BTreeMap::new();
        for level in xml::children(abstract_num, "lvl") {
            let ilvl = xml::attr(level, "ilvl")
                .and_then(units::parse_i64)
                .unwrap_or(0)
                .clamp(0, 8)
                .to_string();
            let format = xml::child(level, "numFmt")
                .and_then(xml::val)
                .unwrap_or("decimal")
                .to_string();
            let text = xml::child(level, "lvlText")
                .and_then(xml::val)
                .unwrap_or("")
                .to_string();
            let start = xml::child(level, "start").and_then(xml::num);
            let (indent, hanging) = xml::child(level, "pPr")
                .and_then(|p| xml::child(p, "ind"))
                .map(|ind| {
                    (
                        xml::attr(ind, "left")
                            .or_else(|| xml::attr(ind, "start"))
                            .and_then(units::parse_i64)
                            .map(units::twips),
                        xml::attr(ind, "hanging")
                            .and_then(units::parse_i64)
                            .map(units::twips),
                    )
                })
                .unwrap_or((None, None));
            levels.insert(
                ilvl,
                NumberingLevel {
                    format,
                    text,
                    start,
                    indent,
                    hanging,
                },
            );
        }
        abstracts.insert(abstract_id.to_string(), levels);
    }

    let mut out = BTreeMap::new();
    for num in xml::children(root, "num") {
        let Some(num_id) = xml::attr(num, "numId") else {
            continue;
        };
        let Some(abstract_id) = xml::child(num, "abstractNumId").and_then(xml::val) else {
            continue;
        };
        if let Some(levels) = abstracts.get(abstract_id) {
            for (ilvl, level) in levels {
                out.insert(format!("{num_id}:{ilvl}"), level.clone());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault>
    <w:pPrDefault><w:pPr><w:spacing w:after="160"/></w:pPr></w:pPrDefault>
  </w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:outlineLvl w:val="0"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="32"/></w:rPr>
  </w:style>
  <w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont">
    <w:name w:val="Default Paragraph Font"/>
  </w:style>
  <w:style w:type="character" w:styleId="Strong">
    <w:name w:val="Strong"/>
    <w:rPr><w:b/></w:rPr>
  </w:style>
</w:styles>"#;

    #[test]
    fn resolves_style_chain_and_defaults() {
        let theme = Theme::default();
        let sheet = StyleSheet::parse(Some(STYLES.as_bytes()), &theme).unwrap();
        assert_eq!(sheet.defaults.paragraph.as_deref(), Some("Normal"));
        assert_eq!(sheet.defaults.run.as_ref().unwrap().size, Some(1100));
        assert_eq!(
            sheet.defaults.run.as_ref().unwrap().font.as_deref(),
            Some("Calibri")
        );
        assert_eq!(
            sheet
                .defaults
                .para
                .as_ref()
                .unwrap()
                .spacing
                .as_ref()
                .unwrap()
                .after,
            Some(800)
        );

        let heading = sheet.style("Heading1").unwrap();
        assert_eq!(heading.based_on.as_deref(), Some("Normal"));
        assert_eq!(heading.run.as_ref().unwrap().size, Some(1600));
        assert_eq!(heading.para.as_ref().unwrap().outline, Some(0));
        assert_eq!(sheet.heading_level(Some("Heading1"), None), Some(1));

        let strong = sheet.style("Strong").unwrap();
        assert_eq!(strong.kind, StyleKind::Character);
        assert_eq!(strong.run.as_ref().unwrap().bold, Some(true));
    }

    #[test]
    fn effective_props_merge_defaults_style_and_direct() {
        let theme = Theme::default();
        let sheet = StyleSheet::parse(Some(STYLES.as_bytes()), &theme).unwrap();
        let direct = RunProps {
            italic: Some(true),
            ..RunProps::default()
        };
        let props = sheet.run_effective(Some("Heading1"), None, Some(&direct));
        assert_eq!(props.bold, Some(true));
        assert_eq!(props.italic, Some(true));
        assert_eq!(props.size, Some(1600));
        assert_eq!(props.font.as_deref(), Some("Calibri"));
    }

    #[test]
    fn resolves_theme_colors_and_fonts() {
        let theme = Theme::parse(None).unwrap();
        assert!(theme.color("accent1", None, None).is_none());
        assert!(theme.font("minorHAnsi").is_none());
    }

    #[test]
    fn breaks_inheritance_cycles() {
        let xml = r#"<w:styles xmlns:w="w">
          <w:style w:type="paragraph" w:styleId="A"><w:basedOn w:val="B"/></w:style>
          <w:style w:type="paragraph" w:styleId="B"><w:basedOn w:val="A"/></w:style>
        </w:styles>"#;
        let sheet = StyleSheet::parse(Some(xml.as_bytes()), &Theme::default()).unwrap();
        assert!(sheet.style("A").is_some());
        assert!(sheet.style("B").is_some());
    }

    #[test]
    fn drops_duplicate_east_asian_font() {
        let xml = r#"<w:styles xmlns:w="w">
          <w:style w:type="paragraph" w:styleId="A">
            <w:rPr><w:rFonts w:ascii="Calibri" w:eastAsia="Calibri"/></w:rPr>
          </w:style>
        </w:styles>"#;
        let sheet = StyleSheet::parse(Some(xml.as_bytes()), &Theme::default()).unwrap();
        let run = sheet.style("A").unwrap().run.as_ref().unwrap();
        assert_eq!(run.font.as_deref(), Some("Calibri"));
        assert!(run.font_east_asian.is_none());
    }

    #[test]
    fn parses_numbering_levels() {
        let xml = r#"<w:numbering xmlns:w="w">
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"#;
        let levels = parse_numbering(Some(xml.as_bytes())).unwrap();
        let level = levels.get("1:0").expect("level");
        assert_eq!(level.format, "bullet");
        assert_eq!(level.text, "•");
    }
}
