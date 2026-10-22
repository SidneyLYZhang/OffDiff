//! Namespace-agnostic helpers over `roxmltree`.
//!
//! OOXML producers are free to choose namespace prefixes, so every lookup in
//! this crate matches on local names instead of `w:`/`a:`/`r:` prefixes.

use roxmltree::{Document, Node};

use crate::error::{OffDiffError, Result};

/// Parse an XML part. OOXML parts are UTF-8; anything else is a corrupt file.
pub fn parse(bytes: &[u8]) -> Result<Document<'_>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| OffDiffError::Xml(format!("XML part is not valid UTF-8: {e}")))?;
    Document::parse(text).map_err(|e| OffDiffError::Xml(format!("XML parsing error: {e}")))
}

/// First element child with the given local name.
pub fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// All element children with the given local name.
pub fn children<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// Attribute by local name, ignoring its namespace.
pub fn attr<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

/// Attribute by local name inside a namespace whose URI contains
/// `namespace_fragment` (used for `r:id` / `r:embed`).
pub fn attr_ns<'a, 'input>(
    node: Node<'a, 'input>,
    namespace_fragment: &str,
    name: &str,
) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| {
            attribute.name() == name
                && attribute
                    .namespace()
                    .map(|namespace| namespace.contains(namespace_fragment))
                    .unwrap_or(false)
        })
        .map(|attribute| attribute.value())
}

/// `w:val` attribute shorthand.
pub fn val<'a, 'input>(node: Node<'a, 'input>) -> Option<&'a str> {
    attr(node, "val")
}

/// Interpret an OOXML on/off property: absent `w:val` means on.
pub fn on_off(node: Node<'_, '_>) -> Option<bool> {
    match val(node) {
        None => Some(true),
        Some(v) => match v.trim() {
            "0" | "false" | "off" => Some(false),
            "1" | "true" | "on" => Some(true),
            _ => Some(true),
        },
    }
}

/// Parse `w:val` as an integer.
pub fn num<'a, 'input>(node: Node<'a, 'input>) -> Option<i64> {
    val(node).and_then(super::units::parse_i64)
}

/// Concatenate the direct text children of a node.
pub fn text(node: Node<'_, '_>) -> String {
    let mut out = String::new();
    for child in node.children() {
        if let Some(t) = child.text() {
            out.push_str(t);
        }
    }
    out
}

/// Depth-first search for the first descendant with the given local name.
pub fn descendant<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

/// Depth-first search for the first descendant matching a predicate.
pub fn find_descendant<'a, 'input>(
    node: Node<'a, 'input>,
    mut predicate: impl FnMut(Node<'a, 'input>) -> bool,
) -> Option<Node<'a, 'input>> {
    node.descendants().find(|n| n.is_element() && predicate(*n))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p w:rsidR="00AB">
      <w:pPr><w:jc w:val="both"/></w:pPr>
      <w:r><w:rPr><w:b/><w:sz w:val="44"/></w:rPr><w:t xml:space="preserve">Hello</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;

    #[test]
    fn finds_nodes_and_attributes_without_prefixes() {
        let doc = parse(SAMPLE.as_bytes()).expect("parse");
        let body = descendant(doc.root_element(), "body").expect("body");
        let paragraph = child(body, "p").expect("paragraph");
        assert_eq!(attr(paragraph, "rsidR"), Some("00AB"));
        let jc = descendant(paragraph, "jc").expect("jc");
        assert_eq!(val(jc), Some("both"));
        let text_node = descendant(paragraph, "t").expect("t");
        assert_eq!(text(text_node), "Hello");
        let bold = descendant(paragraph, "b").expect("b");
        assert_eq!(on_off(bold), Some(true));
    }

    #[test]
    fn on_off_honours_false_values() {
        let doc =
            parse("<rPr xmlns:w=\"w\"><b w:val=\"false\"/><i w:val=\"0\"/><u/></rPr>".as_bytes())
                .unwrap();
        let root = doc.root_element();
        assert_eq!(on_off(child(root, "b").unwrap()), Some(false));
        assert_eq!(on_off(child(root, "i").unwrap()), Some(false));
        assert_eq!(on_off(child(root, "u").unwrap()), Some(true));
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(parse(&[0xff, 0xfe, 0x00]).is_err());
    }
}
