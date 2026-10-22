//! OffDiff: structural extraction of Office and PDF documents.
//!
//! The crate centres on one in-memory representation ([`model::Document`]) and
//! renders every output (JSON artifact, Markdown projection, git diff view)
//! from it.

pub mod cli;
pub mod detect;
pub mod docx;
pub mod error;
pub mod media;
pub mod model;
pub mod ooxml;
pub mod pdf;
pub mod pptx;
pub mod render;
pub mod xlsx;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::error::{OffDiffError, Result};
use crate::model::{Document, Format};
use crate::render::json::{self, Profile};
use crate::render::markdown;

/// A parsed document together with the embedded media bytes referenced by its
/// media table.
#[derive(Debug)]
pub struct Parsed {
    pub document: Document,
    pub media: BTreeMap<String, Vec<u8>>,
}

/// Detect the format from the bytes and parse it.
pub fn parse_bytes(bytes: &[u8]) -> Result<Parsed> {
    let format = detect::detect(bytes)?;
    parse_with_format(bytes, format)
}

/// Parse with an explicitly known format.
pub fn parse_with_format(bytes: &[u8], format: Format) -> Result<Parsed> {
    let (document, media) = match format {
        Format::Docx => docx::parse(bytes)?,
        Format::Xlsx => xlsx::parse(bytes)?,
        Format::Pptx => pptx::parse(bytes)?,
        Format::Pdf => pdf::parse(bytes)?,
    };
    Ok(Parsed { document, media })
}

/// Read and parse a file.
pub fn parse_path(path: &Path) -> Result<Parsed> {
    let bytes = std::fs::read(path).map_err(|e| OffDiffError::from_io_with_path(e, path))?;
    parse_bytes(&bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Markdown,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaMode {
    None,
    Copy,
    Embed,
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub out_dir: Option<PathBuf>,
    pub media: MediaMode,
    pub pretty: bool,
    pub rich: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            format: ExportFormat::Json,
            out_dir: None,
            media: MediaMode::None,
            pretty: false,
            rich: false,
        }
    }
}

/// Export a document according to the options.
pub fn export(input: &Path, options: &ExportOptions) -> Result<()> {
    let mut parsed = parse_path(input)?;
    if options.media == MediaMode::Embed {
        embed_media(&mut parsed);
    }

    match &options.out_dir {
        None => {
            if options.format == ExportFormat::Both {
                return Err(OffDiffError::InvalidInput(
                    "--out-dir is required when exporting both formats".to_string(),
                ));
            }
            if options.media == MediaMode::Copy {
                return Err(OffDiffError::InvalidInput(
                    "--out-dir is required with --media copy".to_string(),
                ));
            }
            match options.format {
                ExportFormat::Json => write_stdout(&json_text(&parsed.document, options.pretty)?),
                ExportFormat::Markdown => {
                    let text = markdown::render(
                        &parsed.document,
                        &markdown::Options {
                            rich: options.rich,
                            media_prefix: None,
                        },
                    );
                    write_stdout(&text);
                }
                ExportFormat::Both => unreachable!(),
            }
        }
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|e| OffDiffError::from_io_with_path(e, dir))?;
            let stem = stem_of(input);
            if options.media == MediaMode::Copy {
                write_media(dir, &stem, &parsed)?;
            }
            let prefix = if options.media == MediaMode::Copy {
                Some(format!("{stem}.media/"))
            } else {
                None
            };

            if matches!(options.format, ExportFormat::Json | ExportFormat::Both) {
                let path = dir.join(format!("{stem}.json"));
                write_file(&path, &json_text(&parsed.document, options.pretty)?)?;
            }
            if matches!(options.format, ExportFormat::Markdown | ExportFormat::Both) {
                let text = markdown::render(
                    &parsed.document,
                    &markdown::Options {
                        rich: options.rich,
                        media_prefix: prefix,
                    },
                );
                let path = dir.join(format!("{stem}.md"));
                write_file(&path, &text)?;
            }
        }
    }
    Ok(())
}

fn json_text(document: &Document, pretty: bool) -> Result<String> {
    if pretty {
        Ok(format!("{}\n", serde_json::to_string_pretty(document)?))
    } else {
        json::render(document, Profile::Artifact)
    }
}

fn embed_media(parsed: &mut Parsed) {
    for (sha256, media) in parsed.document.media.iter_mut() {
        if media.data.is_none() {
            if let Some(bytes) = parsed.media.get(sha256) {
                media.data = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
            }
        }
    }
}

fn write_media(dir: &Path, stem: &str, parsed: &Parsed) -> Result<()> {
    if parsed.media.is_empty() {
        return Ok(());
    }
    let media_dir = dir.join(format!("{stem}.media"));
    std::fs::create_dir_all(&media_dir)
        .map_err(|e| OffDiffError::from_io_with_path(e, &media_dir))?;
    for (sha256, bytes) in &parsed.media {
        let mime = parsed
            .document
            .media
            .get(sha256)
            .map(|media| media.mime.as_str())
            .unwrap_or("application/octet-stream");
        let path = media_dir.join(format!("{sha256}.{}", markdown::extension_for_mime(mime)));
        std::fs::write(&path, bytes).map_err(|e| OffDiffError::from_io_with_path(e, &path))?;
    }
    Ok(())
}

fn stem_of(input: &Path) -> String {
    input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("document")
        .to_string()
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|e| OffDiffError::from_io_with_path(e, path))
}

fn write_stdout(contents: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(contents.as_bytes());
    let _ = stdout.flush();
}

/// Entry point shared by the binary and integration tests.
pub fn run(cli: cli::Cli) -> Result<()> {
    match cli.command {
        cli::Command::Export(args) => export_command(args),
        cli::Command::Textconv(args) => {
            textconv_command(args);
            Ok(())
        }
        cli::Command::GitInit(args) => git_init_command(args),
    }
}

fn export_command(args: cli::ExportArgs) -> Result<()> {
    let options = ExportOptions {
        format: match args.format {
            cli::FormatArg::Json => ExportFormat::Json,
            cli::FormatArg::Md => ExportFormat::Markdown,
            cli::FormatArg::Both => ExportFormat::Both,
        },
        out_dir: args.out_dir,
        media: match args.media {
            cli::MediaArg::None => MediaMode::None,
            cli::MediaArg::Copy => MediaMode::Copy,
            cli::MediaArg::Embed => MediaMode::Embed,
        },
        pretty: args.pretty,
        rich: args.rich,
    };
    export(&args.input, &options)
}

/// textconv must never fail the git command, so errors become a stable
/// one-line placeholder instead of a non-zero exit.
fn textconv_command(args: cli::TextconvArgs) {
    let _ = args.tool_version;
    let text = match parse_path(&args.input) {
        Ok(parsed) => {
            let profile = if args.include_volatile {
                Profile::Artifact
            } else {
                Profile::Diff
            };
            json::render(&parsed.document, profile).unwrap_or_else(|_| error_line("render_failed"))
        }
        Err(error) => error_line(error_kind(&error)),
    };
    write_stdout(&text);
}

fn error_line(kind: &str) -> String {
    format!("{{\"type\":\"error\",\"error\":\"{kind}\"}}\n")
}

fn error_kind(error: &OffDiffError) -> &'static str {
    match error {
        OffDiffError::FileNotFound { .. } => "missing",
        OffDiffError::UnsupportedFormat { .. } | OffDiffError::UnsupportedContent(_) => {
            "unsupported"
        }
        OffDiffError::InvalidZip { .. }
        | OffDiffError::Zip(_)
        | OffDiffError::Corrupt(_)
        | OffDiffError::Xml(_)
        | OffDiffError::MissingPart { .. } => "corrupt",
        OffDiffError::Io { .. } | OffDiffError::IoPlain(_) => "io_error",
        OffDiffError::Unavailable(_) => "unavailable",
        _ => "error",
    }
}

const ATTRIBUTES: [&str; 4] = [
    "*.docx diff=offdiff",
    "*.xlsx diff=offdiff",
    "*.pptx diff=offdiff",
    "*.pdf diff=offdiff",
];

fn git_init_command(args: cli::GitInitArgs) -> Result<()> {
    let scope = if args.global { "--global" } else { "--local" };
    let (attributes_path, root) = if args.global {
        (global_attributes_path()?, None)
    } else {
        let root = repository_root()?;
        (root.join(".gitattributes"), Some(root))
    };

    let binary = args
        .bin
        .as_ref()
        .map(|path| path.display().to_string().replace('\\', "/"))
        .unwrap_or_else(|| "offdiff".to_string());
    let binary = if binary.contains(' ') {
        format!("\"{binary}\"")
    } else {
        binary
    };
    let command = format!(
        "{binary} textconv --tool-version {}",
        env!("CARGO_PKG_VERSION")
    );

    git_output(&["config", scope, "diff.offdiff.textconv", command.as_str()])?;
    git_output(&["config", scope, "diff.offdiff.binary", "true"])?;
    git_output(&["config", scope, "diff.offdiff.cachetextconv", "true"])?;

    let added = ensure_attributes(&attributes_path)?;

    let scope_label = if args.global { "global" } else { "repository" };
    println!("Configured diff driver 'offdiff' in the {scope_label} scope.");
    println!("  textconv: {command}");
    if added.is_empty() {
        println!(
            "  attributes: {} (already up to date)",
            attributes_path.display()
        );
    } else {
        println!(
            "  attributes: {} (+{} line{})",
            attributes_path.display(),
            added.len(),
            if added.len() == 1 { "" } else { "s" }
        );
    }
    if let Some(root) = root {
        println!("  repository: {}", root.display());
    }
    Ok(())
}

fn git_output(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|error| OffDiffError::Git(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(OffDiffError::Git(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repository_root() -> Result<PathBuf> {
    let root = git_output(&["rev-parse", "--show-toplevel"])?;
    if root.is_empty() {
        return Err(OffDiffError::Git(
            "not inside a git repository; initialise one or use --global".to_string(),
        ));
    }
    Ok(PathBuf::from(root))
}

fn global_attributes_path() -> Result<PathBuf> {
    let configured = std::process::Command::new("git")
        .args(["config", "--global", "--get", "core.attributesFile"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());

    if let Some(path) = configured {
        return Ok(expand_home(&path));
    }
    let home = home_dir().ok_or_else(|| {
        OffDiffError::Git("cannot determine the home directory for --global".to_string())
    })?;
    Ok(home.join(".config").join("git").join("attributes"))
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn ensure_attributes(path: &Path) -> Result<Vec<String>> {
    let mut contents = std::fs::read_to_string(path).unwrap_or_default();
    let mut added = Vec::new();
    for line in ATTRIBUTES {
        if contents.lines().any(|existing| existing.trim() == line) {
            continue;
        }
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(line);
        contents.push('\n');
        added.push(line.to_string());
    }
    if !added.is_empty() || !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| OffDiffError::from_io_with_path(e, parent))?;
        }
        write_file(path, &contents)?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("offdiff-{}-{name}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn docx_bytes(text: &str) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:body>
</w:document>"#
        );
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("word/document.xml", options).unwrap();
            writer.write_all(document.as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn parses_the_bundled_docx_fixture() {
        use crate::model::Block;

        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/test.docx");
        let parsed = parse_path(&path).expect("bundled fixture");
        assert_eq!(parsed.document.format, Format::Docx);
        assert!(parsed.document.properties.stable.contains_key("author"));

        let section = &parsed.document.sections[0];
        assert_eq!(section.blocks.len(), 2);
        let Block::Paragraph(first) = &section.blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(
            first.direct.as_ref().unwrap().align.as_deref(),
            Some("center")
        );
        let Block::Paragraph(second) = &section.blocks[1] else {
            panic!("expected paragraph");
        };
        assert!(second.runs.iter().any(|run| matches!(
            run,
            crate::model::Run::Text(text) if text.direct.as_ref().and_then(|p| p.bold) == Some(true)
        )));
    }

    #[test]
    fn bundled_docx_matches_golden_projections() {
        use crate::render::json::{self, Profile};
        use crate::render::markdown;

        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let parsed = parse_path(&root.join("examples/test.docx")).expect("bundled fixture");

        let json_text = json::render(&parsed.document, Profile::Artifact).expect("json");
        let expected_json =
            std::fs::read_to_string(root.join("tests/golden/test.json")).expect("golden json");
        assert_eq!(
            json_text, expected_json,
            "JSON artifact drifted from golden"
        );

        let markdown_text = markdown::render(
            &parsed.document,
            &markdown::Options {
                rich: false,
                media_prefix: Some("test.media/".to_string()),
            },
        );
        let expected_markdown =
            std::fs::read_to_string(root.join("tests/golden/test.md")).expect("golden markdown");
        assert_eq!(
            markdown_text, expected_markdown,
            "Markdown projection drifted from golden"
        );
    }

    #[test]
    fn exports_json_and_markdown_to_a_directory() {
        let dir = temp_dir("export");
        let input = dir.join("sample.docx");
        std::fs::write(&input, docx_bytes("Hello")).unwrap();
        let out = dir.join("out");

        export(
            &input,
            &ExportOptions {
                format: ExportFormat::Both,
                out_dir: Some(out.clone()),
                ..ExportOptions::default()
            },
        )
        .expect("export");

        let json_text = std::fs::read_to_string(out.join("sample.json")).expect("json");
        let parsed: Document = serde_json::from_str(&json_text).expect("valid json");
        assert_eq!(parsed.schema, "offdiff/1");
        assert_eq!(parsed.format, Format::Docx);

        let markdown_text = std::fs::read_to_string(out.join("sample.md")).expect("markdown");
        assert!(markdown_text.contains("Hello"), "{markdown_text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_rejects_media_copy_without_out_dir() {
        let dir = temp_dir("media");
        let input = dir.join("sample.docx");
        std::fs::write(&input, docx_bytes("Hello")).unwrap();
        let result = export(
            &input,
            &ExportOptions {
                media: MediaMode::Copy,
                ..ExportOptions::default()
            },
        );
        assert!(matches!(result, Err(OffDiffError::InvalidInput(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn textconv_error_line_is_single_line_json() {
        let line = error_line("corrupt");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["error"], "corrupt");
    }

    #[test]
    fn error_kinds_are_stable() {
        assert_eq!(
            error_kind(&OffDiffError::MissingPart {
                part: "word/document.xml".into()
            }),
            "corrupt"
        );
        assert_eq!(
            error_kind(&OffDiffError::UnsupportedContent("x".into())),
            "unsupported"
        );
        assert_eq!(
            error_kind(&OffDiffError::FileNotFound {
                path: PathBuf::from("x")
            }),
            "missing"
        );
    }

    #[test]
    fn attributes_are_appended_once() {
        let dir = temp_dir("attributes");
        let path = dir.join(".gitattributes");
        std::fs::write(&path, "*.docx -text\n").unwrap();

        let added = ensure_attributes(&path).unwrap();
        assert_eq!(added.len(), 4);
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.starts_with("*.docx -text\n"));
        assert!(first.contains("*.xlsx diff=offdiff"));

        let added_again = ensure_attributes(&path).unwrap();
        assert!(added_again.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stem_falls_back_for_extensionless_names() {
        assert_eq!(stem_of(Path::new("a/b/report.docx")), "report");
        assert_eq!(stem_of(Path::new("a/b/noext")), "noext");
    }
}
