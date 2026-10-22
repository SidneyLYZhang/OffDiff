//! Shared OOXML infrastructure: package access, XML helpers, units.

pub mod package;
pub mod units;
pub mod xml;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};

use roxmltree::Node;

use crate::error::Result;
use crate::model::Properties;

pub use package::Package;

/// Normalise a hex colour to `#RRGGBB`, returning `None` for values that are
/// not six hex digits.
pub fn normalize_hex(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", value.to_ascii_uppercase()))
}

/// Read `docProps/core.xml` and `docProps/app.xml` into the stable/volatile
/// property maps.
///
/// Timestamps, revision counters and the producing application are volatile:
/// they change when a document is re-saved without any content edit.
pub fn read_properties<R: Read + Seek>(package: &mut Package<R>) -> Result<Properties> {
    let mut stable: BTreeMap<&str, String> = BTreeMap::new();
    let mut volatile: BTreeMap<&str, String> = BTreeMap::new();

    if let Some(bytes) = package.read_opt("docProps/core.xml")? {
        let document = xml::parse(&bytes)?;
        for node in document.root_element().children().filter(Node::is_element) {
            let value = xml::text(node).trim().to_string();
            if value.is_empty() {
                continue;
            }
            match node.tag_name().name() {
                "title" => stable.insert("title", value),
                "subject" => stable.insert("subject", value),
                "creator" => stable.insert("author", value),
                "keywords" => stable.insert("keywords", value),
                "description" => stable.insert("description", value),
                "category" => stable.insert("category", value),
                "contentStatus" => stable.insert("content_status", value),
                "identifier" => stable.insert("identifier", value),
                "language" => stable.insert("language", value),
                "version" => stable.insert("version", value),
                "created" => stable.insert("created", value),
                "lastModifiedBy" => volatile.insert("last_modified_by", value),
                "modified" => volatile.insert("modified", value),
                "revision" => volatile.insert("revision", value),
                _ => None,
            };
        }
    }

    if let Some(bytes) = package.read_opt("docProps/app.xml")? {
        let document = xml::parse(&bytes)?;
        for node in document.root_element().children().filter(Node::is_element) {
            let value = xml::text(node).trim().to_string();
            if value.is_empty() {
                continue;
            }
            match node.tag_name().name() {
                "Application" => volatile.insert("application", value),
                "AppVersion" => volatile.insert("app_version", value),
                "Company" => stable.insert("company", value),
                "Manager" => stable.insert("manager", value),
                _ => None,
            };
        }
    }

    Ok(Properties {
        stable: stable
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
        volatile: volatile
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
        unsupported: BTreeSet::new(),
    })
}

pub mod rels {
    //! Relationship parts (`_rels/*.rels`).

    use std::collections::BTreeMap;

    use super::xml;
    use crate::error::Result;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Relationship {
        pub id: String,
        /// Target as written in the part; relative to the source part.
        pub target: String,
        /// Relationship type URI.
        pub kind: String,
        pub external: bool,
    }

    pub type Relationships = BTreeMap<String, Relationship>;

    pub fn parse(bytes: &[u8]) -> Result<Relationships> {
        let document = xml::parse(bytes)?;
        let mut map = BTreeMap::new();
        for node in document
            .root_element()
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        {
            let Some(id) = xml::attr(node, "Id") else {
                continue;
            };
            let target = xml::attr(node, "Target").unwrap_or_default().to_string();
            let kind = xml::attr(node, "Type").unwrap_or_default().to_string();
            let external = xml::attr(node, "TargetMode") == Some("External");
            map.insert(
                id.to_string(),
                Relationship {
                    id: id.to_string(),
                    target,
                    kind,
                    external,
                },
            );
        }
        Ok(map)
    }

    /// `word/document.xml` -> `word/_rels/document.xml.rels`.
    pub fn path_for(part: &str) -> String {
        match part.rsplit_once('/') {
            Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
            None => format!("_rels/{part}.rels"),
        }
    }

    /// Resolve a relationship target against the part that declares it.
    pub fn resolve(base_part: &str, target: &str) -> String {
        let target = target.replace('\\', "/");
        if let Some(absolute) = target.strip_prefix('/') {
            return normalize(absolute);
        }
        let base_dir = base_part.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        if base_dir.is_empty() {
            normalize(&target)
        } else {
            normalize(&format!("{base_dir}/{target}"))
        }
    }

    fn normalize(path: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        parts.join("/")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn builds_relationship_part_path() {
            assert_eq!(
                path_for("word/document.xml"),
                "word/_rels/document.xml.rels"
            );
            assert_eq!(path_for("document.xml"), "_rels/document.xml.rels");
        }

        #[test]
        fn resolves_relative_targets() {
            assert_eq!(
                resolve("word/document.xml", "media/image1.png"),
                "word/media/image1.png"
            );
            assert_eq!(
                resolve("word/document.xml", "../customXml/item1.xml"),
                "customXml/item1.xml"
            );
            assert_eq!(
                resolve("word/document.xml", "/word/styles.xml"),
                "word/styles.xml"
            );
        }

        #[test]
        fn parses_relationship_entries() {
            let xml = br#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/>
</Relationships>"#;
            let rels = parse(xml).unwrap();
            assert_eq!(rels.len(), 2);
            assert_eq!(rels["rId1"].target, "media/image1.png");
            assert!(rels["rId2"].external);
            assert_eq!(rels["rId2"].target, "https://example.com");
        }
    }
}
