//! Content-based format detection.
//!
//! Git hands textconv a randomly named temp file whose extension may or may not
//! survive, so the format is always decided from the bytes themselves.

use std::path::Path;

use crate::error::{OffDiffError, Result};
use crate::model::Format;
use crate::ooxml::Package;

/// Detect a supported format from the leading bytes of a file.
pub fn detect(bytes: &[u8]) -> Result<Format> {
    if bytes.starts_with(b"%PDF-") {
        return Ok(Format::Pdf);
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        let package = Package::from_bytes(bytes)?;
        if package.has("word/document.xml") {
            return Ok(Format::Docx);
        }
        if package.has("xl/workbook.xml") {
            return Ok(Format::Xlsx);
        }
        if package.has("ppt/presentation.xml") {
            return Ok(Format::Pptx);
        }
        return Err(OffDiffError::UnsupportedContent(
            "ZIP container has no OOXML document part".into(),
        ));
    }
    Err(OffDiffError::UnsupportedContent(
        "content is neither a ZIP (Office) nor a PDF container".into(),
    ))
}

/// Read a file and detect its format.
pub fn detect_path(path: &Path) -> Result<(Format, Vec<u8>)> {
    let bytes = std::fs::read(path).map_err(|e| OffDiffError::from_io_with_path(e, path))?;
    match detect(&bytes) {
        Ok(format) => Ok((format, bytes)),
        Err(OffDiffError::UnsupportedContent(reason)) => Err(OffDiffError::UnsupportedFormat {
            path: path.to_path_buf(),
            reason,
        }),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;

    fn docx_like() -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            writer
                .start_file(
                    "word/document.xml",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"<w:document/>").unwrap();
            writer.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn detects_pdf() {
        assert_eq!(detect(b"%PDF-1.7\n...").unwrap(), Format::Pdf);
    }

    #[test]
    fn detects_docx_from_container_contents() {
        assert_eq!(detect(&docx_like()).unwrap(), Format::Docx);
    }

    #[test]
    fn rejects_unknown_content() {
        assert!(matches!(
            detect(b"hello world"),
            Err(OffDiffError::UnsupportedContent(_))
        ));
    }

    #[test]
    fn rejects_zip_without_document_part() {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            writer
                .start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hello").unwrap();
            writer.finish().unwrap();
        }
        assert!(matches!(
            detect(&buffer),
            Err(OffDiffError::UnsupportedContent(_))
        ));
    }
}
