//! Command line interface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "offdiff",
    version,
    about = "Structural diff and export for Office and PDF documents",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Export a document as a JSON artifact and/or a Markdown projection.
    Export(ExportArgs),
    /// Print the git diff view for one document (used by `git textconv`).
    Textconv(TextconvArgs),
    /// Configure a repository (or the global git config) to diff Office files.
    GitInit(GitInitArgs),
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Source .docx / .xlsx / .pptx / .pdf file.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Which projections to write.
    #[arg(long, value_enum, default_value_t = FormatArg::Json)]
    pub format: FormatArg,

    /// Directory for `<stem>.json`, `<stem>.md` and `<stem>.media/`.
    #[arg(long, value_name = "DIR")]
    pub out_dir: Option<PathBuf>,

    /// How embedded media is represented.
    #[arg(long, value_enum, default_value_t = MediaArg::None)]
    pub media: MediaArg,

    /// Indent the JSON artifact for human reading instead of one unit per line.
    #[arg(long)]
    pub pretty: bool,

    /// Emit inline HTML for formatting Markdown cannot express.
    #[arg(long)]
    pub rich: bool,
}

#[derive(Args, Debug)]
pub struct TextconvArgs {
    /// Document path appended by git.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Part of the configured command string so git's cache invalidates when
    /// the tool is upgraded. Accepted and ignored at runtime.
    #[arg(long, value_name = "VERSION", hide = true)]
    pub tool_version: Option<String>,

    /// Include volatile properties (timestamps, revision counters).
    #[arg(long)]
    pub include_volatile: bool,
}

#[derive(Args, Debug)]
pub struct GitInitArgs {
    /// Write the diff driver to the global git config and attributes file.
    #[arg(long)]
    pub global: bool,

    /// Absolute path to the offdiff binary, if it is not on PATH.
    #[arg(long, value_name = "PATH")]
    pub bin: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FormatArg {
    Json,
    Md,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum MediaArg {
    None,
    Copy,
    Embed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_textconv_with_the_path_git_appends() {
        let cli = Cli::try_parse_from([
            "offdiff",
            "textconv",
            "--tool-version",
            "0.1.0",
            "C:/tmp/git-blob-1/report.docx",
        ])
        .expect("parse");
        match cli.command {
            Command::Textconv(args) => {
                assert_eq!(args.input, PathBuf::from("C:/tmp/git-blob-1/report.docx"));
                assert_eq!(args.tool_version.as_deref(), Some("0.1.0"));
                assert!(!args.include_volatile);
            }
            other => panic!("expected textconv, got {other:?}"),
        }
    }

    #[test]
    fn export_defaults_to_json() {
        let cli = Cli::try_parse_from(["offdiff", "export", "report.docx"]).expect("parse");
        match cli.command {
            Command::Export(args) => {
                assert_eq!(args.format, FormatArg::Json);
                assert_eq!(args.media, MediaArg::None);
                assert!(args.out_dir.is_none());
            }
            other => panic!("expected export, got {other:?}"),
        }
    }
}
