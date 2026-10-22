//! Error model shared by the library and the CLI.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OffDiffError {
    #[error("File not found: {path}\n  Check that the file path is correct.")]
    FileNotFound { path: PathBuf },

    #[error("IO error while accessing '{path}': {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("IO error: {0}")]
    IoPlain(#[from] std::io::Error),

    #[error(
        "'{path}' is not a supported Office or PDF file: {reason}\n  Supported formats: .docx, .xlsx, .pptx, .pdf"
    )]
    UnsupportedFormat { path: PathBuf, reason: String },

    #[error("Unsupported document content: {0}")]
    UnsupportedContent(String),

    #[error(
        "Failed to read ZIP archive: {path}\n  The file may be corrupted or password protected."
    )]
    InvalidZip { path: PathBuf },

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Missing required part '{part}' in the package")]
    MissingPart { part: String },

    #[error("Document is corrupt: {0}")]
    Corrupt(String),

    #[error("XML parsing error: {0}")]
    Xml(String),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Git integration failed: {0}")]
    Git(String),

    #[error("Feature is not available in this build: {0}")]
    Unavailable(String),
}

impl OffDiffError {
    pub fn from_io_with_path(source: std::io::Error, path: &std::path::Path) -> Self {
        if source.kind() == std::io::ErrorKind::NotFound {
            OffDiffError::FileNotFound {
                path: path.to_path_buf(),
            }
        } else {
            OffDiffError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, OffDiffError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reports_the_path() {
        let err = OffDiffError::FileNotFound {
            path: PathBuf::from("missing.docx"),
        };
        let message = err.to_string();
        assert!(message.contains("missing.docx"));
        assert!(message.contains("File not found"));
    }

    #[test]
    fn unsupported_format_lists_supported_ones() {
        let err = OffDiffError::UnsupportedFormat {
            path: PathBuf::from("notes.txt"),
            reason: "not a ZIP or PDF container".into(),
        };
        let message = err.to_string();
        assert!(message.contains("notes.txt"));
        assert!(message.contains(".docx"));
        assert!(message.contains(".pdf"));
    }
}
