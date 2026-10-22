//! Read-only access to an OPC (ZIP) package.
//!
//! Works both from a file path and from an in-memory buffer, which keeps the
//! parser testable and lets the textconv entry point consume Git's temp files
//! without caring where they live.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use zip::ZipArchive;

use crate::error::{OffDiffError, Result};

/// Refuse to inflate individual parts larger than this. Protects against
/// decompression bombs in untrusted documents.
const MAX_PART_BYTES: u64 = 256 * 1024 * 1024;

pub struct Package<R: Read + Seek> {
    archive: ZipArchive<R>,
    names: BTreeSet<String>,
}

impl Package<File> {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| OffDiffError::from_io_with_path(e, path))?;
        Self::new(file)
    }
}

impl Package<Cursor<Vec<u8>>> {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::new(Cursor::new(bytes.to_vec()))
    }
}

impl<R: Read + Seek> Package<R> {
    pub fn new(reader: R) -> Result<Self> {
        let archive = ZipArchive::new(reader)?;
        let names = archive.file_names().map(str::to_string).collect();
        Ok(Package { archive, names })
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// Read a required part.
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        if !self.has(name) {
            return Err(OffDiffError::MissingPart {
                part: name.to_string(),
            });
        }
        let mut entry = self.archive.by_name(name).map_err(OffDiffError::Zip)?;
        if entry.size() > MAX_PART_BYTES {
            return Err(OffDiffError::Corrupt(format!(
                "part '{name}' is larger than {MAX_PART_BYTES} bytes"
            )));
        }
        let mut buffer = Vec::with_capacity(entry.size().min(1 << 20) as usize);
        entry.read_to_end(&mut buffer)?;
        Ok(buffer)
    }

    /// Read an optional part.
    pub fn read_opt(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        if !self.has(name) {
            return Ok(None);
        }
        self.read(name).map(Some)
    }

    /// Read a required part as UTF-8 text.
    pub fn read_string(&mut self, name: &str) -> Result<String> {
        let bytes = self.read(name)?;
        String::from_utf8(bytes)
            .map_err(|_| OffDiffError::Corrupt(format!("part '{name}' is not valid UTF-8")))
    }

    /// Read an optional part as UTF-8 text.
    pub fn read_string_opt(&mut self, name: &str) -> Result<Option<String>> {
        match self.read_opt(name)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| OffDiffError::Corrupt(format!("part '{name}' is not valid UTF-8"))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn reads_parts_from_memory() {
        let bytes = zip_with(&[
            ("word/document.xml", b"<doc/>"),
            ("word/styles.xml", b"<s/>"),
        ]);
        let mut package = Package::from_bytes(&bytes).expect("package");
        assert!(package.has("word/document.xml"));
        assert_eq!(package.read("word/document.xml").unwrap(), b"<doc/>");
        assert_eq!(package.read_string("word/styles.xml").unwrap(), "<s/>");
        assert!(package.read_opt("word/numbering.xml").unwrap().is_none());
    }

    #[test]
    fn missing_required_part_is_reported() {
        let bytes = zip_with(&[("word/document.xml", b"<doc/>")]);
        let mut package = Package::from_bytes(&bytes).unwrap();
        match package.read("word/styles.xml") {
            Err(OffDiffError::MissingPart { part }) => {
                assert_eq!(part, "word/styles.xml")
            }
            other => panic!("expected MissingPart, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_zip_input() {
        assert!(Package::from_bytes(b"not a zip").is_err());
    }
}
