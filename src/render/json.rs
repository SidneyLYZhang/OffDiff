//! Line-oriented JSON projection.
//!
//! The artifact is a single valid JSON document, but every semantic unit
//! (paragraph, table row, shape, text block, style definition) occupies exactly
//! one line. That keeps `git diff` hunks small and stable while remaining
//! machine-readable.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::Result;
use crate::model::{Block, Document, Properties, Section};

/// Which set of fields a projection exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Full JSON artifact, including volatile document properties.
    Artifact,
    /// Git diff view: volatile properties are removed so that saving a document
    /// without editing it does not produce a diff.
    Diff,
}

impl Profile {
    fn includes_volatile(self) -> bool {
        matches!(self, Profile::Artifact)
    }
}

pub fn render(document: &Document, profile: Profile) -> Result<String> {
    let mut out = String::new();
    out.push_str("{\n\"schema\":");
    push(&mut out, &document.schema)?;
    out.push_str(",\n\"format\":");
    push(&mut out, &document.format)?;
    out.push_str(",\n\"properties\":");
    out.push_str(&properties(&document.properties, profile)?);
    out.push_str(",\n\"defaults\":");
    match &document.defaults {
        Some(defaults) => push(&mut out, defaults)?,
        None => out.push_str("{}"),
    }
    out.push_str(",\n\"styles\":");
    write_map(&mut out, &document.styles)?;
    out.push_str(",\n\"numbering\":");
    write_map(&mut out, &document.numbering)?;
    out.push_str(",\n\"media\":");
    write_map(&mut out, &document.media)?;
    out.push_str(",\n\"sections\":[");
    if document.sections.is_empty() {
        out.push(']');
    } else {
        out.push('\n');
        for (index, section) in document.sections.iter().enumerate() {
            if index > 0 {
                out.push_str(",\n");
            }
            write_section(&mut out, section)?;
        }
        out.push_str("\n]");
    }
    out.push_str("\n}\n");
    Ok(out)
}

fn properties(properties: &Properties, profile: Profile) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    if !properties.stable.is_empty() {
        parts.push(format!(
            "\"stable\":{}",
            serde_json::to_string(&properties.stable)?
        ));
    }
    if profile.includes_volatile() && !properties.volatile.is_empty() {
        parts.push(format!(
            "\"volatile\":{}",
            serde_json::to_string(&properties.volatile)?
        ));
    }
    if !properties.unsupported.is_empty() {
        parts.push(format!(
            "\"unsupported\":{}",
            serde_json::to_string(&properties.unsupported)?
        ));
    }
    Ok(format!("{{{}}}", parts.join(",")))
}

fn write_map<T: Serialize>(out: &mut String, map: &BTreeMap<String, T>) -> Result<()> {
    if map.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    out.push_str("{\n");
    for (index, (key, value)) in map.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        push(out, key)?;
        out.push(':');
        push(out, value)?;
    }
    out.push_str("\n}");
    Ok(())
}

fn write_section(out: &mut String, section: &Section) -> Result<()> {
    out.push_str("{\"kind\":");
    push(out, &section.kind)?;
    if let Some(name) = &section.name {
        out.push_str(",\"name\":");
        push(out, name)?;
    }
    if let Some(hidden) = section.hidden {
        out.push_str(",\"hidden\":");
        push(out, &hidden)?;
    }
    if let Some(page) = &section.page {
        out.push_str(",\"page\":");
        push(out, page)?;
    }
    if !section.cols.is_empty() {
        out.push_str(",\"cols\":");
        push(out, &section.cols)?;
    }
    if !section.merges.is_empty() {
        out.push_str(",\"merges\":");
        push(out, &section.merges)?;
    }
    out.push_str(",\"blocks\":[");
    write_blocks(out, &section.blocks)?;
    out.push(']');
    if !section.notes.is_empty() {
        out.push_str(",\"notes\":[");
        write_blocks(out, &section.notes)?;
        out.push(']');
    }
    out.push('}');
    Ok(())
}

fn write_blocks(out: &mut String, blocks: &[Block]) -> Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }
    out.push('\n');
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        push(out, block)?;
    }
    out.push('\n');
    Ok(())
}

fn push<T: Serialize + ?Sized>(out: &mut String, value: &T) -> Result<()> {
    out.push_str(&serde_json::to_string(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn sample() -> Document {
        let mut document = Document::new(Format::Docx);
        document
            .properties
            .stable
            .insert("title".into(), "Sample".into());
        document
            .properties
            .volatile
            .insert("modified".into(), "2026-10-21T00:00:00Z".into());
        document.defaults = Some(Defaults {
            paragraph: Some("Normal".into()),
            ..Defaults::default()
        });
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
                ..Style::new(StyleKind::Paragraph)
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
                    runs: vec![Run::Text(TextRun {
                        text: "line\nbreak".into(),
                        ..TextRun::default()
                    })],
                    ..Paragraph::default()
                }),
            ],
            notes: Vec::new(),
        });
        document
    }

    #[test]
    fn artifact_is_valid_json_and_roundtrips() {
        let document = sample();
        let text = render(&document, Profile::Artifact).unwrap();
        let parsed: Document = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed, document);
    }

    #[test]
    fn every_block_occupies_one_line() {
        let document = sample();
        let text = render(&document, Profile::Artifact).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("{\"type\":\"paragraph\"")),
            "blocks must start their own line: {text}"
        );
        assert!(
            !lines.iter().any(|line| line.contains('\n')),
            "no embedded newlines"
        );
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn diff_profile_drops_volatile_properties() {
        let document = sample();
        let artifact = render(&document, Profile::Artifact).unwrap();
        assert!(artifact.contains("\"volatile\""));
        let diff = render(&document, Profile::Diff).unwrap();
        assert!(!diff.contains("\"volatile\""));
        assert!(diff.contains("\"stable\""));
    }

    #[test]
    fn writes_sheet_properties() {
        let mut document = Document::new(Format::Xlsx);
        document.sections.push(Section {
            kind: SectionKind::Sheet,
            name: Some("Data".into()),
            hidden: Some(true),
            page: None,
            cols: vec![crate::model::ColProps {
                index: 1,
                span: 2,
                width: Some("12.5".into()),
                hidden: None,
            }],
            merges: Vec::new(),
            blocks: Vec::new(),
            notes: Vec::new(),
        });

        let text = render(&document, Profile::Artifact).unwrap();
        assert!(text.contains("\"hidden\":true"), "{text}");
        assert!(
            text.contains("\"cols\":[{\"index\":1,\"span\":2,\"width\":\"12.5\"}]"),
            "{text}"
        );
        let parsed: Document = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.sections[0].hidden, Some(true));
        assert_eq!(parsed.sections[0].cols.len(), 1);
    }

    #[test]
    fn output_is_byte_identical_across_runs() {
        let document = sample();
        let first = render(&document, Profile::Diff).unwrap();
        let second = render(&document, Profile::Diff).unwrap();
        assert_eq!(first, second);
    }
}
