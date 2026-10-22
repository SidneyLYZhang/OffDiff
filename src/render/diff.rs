//! Git diff view: the JSON projection with volatile properties removed.

use crate::error::Result;
use crate::model::Document;

use super::json::{self, Profile};

pub fn render(document: &Document) -> Result<String> {
    json::render(document, Profile::Diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn diff_view_omits_modified_timestamp() {
        let mut document = Document::new(Format::Docx);
        document
            .properties
            .volatile
            .insert("modified".into(), "2026-10-21T00:00:00Z".into());
        document
            .properties
            .stable
            .insert("title".into(), "T".into());
        document.sections.push(Section {
            kind: SectionKind::Body,
            name: None,
            hidden: None,
            page: None,
            cols: Vec::new(),
            merges: Vec::new(),
            blocks: vec![Block::Paragraph(Paragraph::default())],
            notes: Vec::new(),
        });

        let text = render(&document).unwrap();
        assert!(text.contains("\"title\""));
        assert!(!text.contains("2026-10-21"));
        let parsed: Document = serde_json::from_str(&text).unwrap();
        assert!(parsed.properties.volatile.is_empty());
    }
}
