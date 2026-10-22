//! End-to-end git textconv smoke test.
//!
//! Ignored by default: it needs `git` plus a shell that can spawn the textconv
//! command. On Windows under restricted sandboxes MSYS `sh` cannot create its
//! signal pipe, so run it from a normal terminal:
//!
//! ```text
//! cargo test --test git_textconv -- --ignored --nocapture
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "offdiff-git-{}-{name}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn docx(align: &str, text: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:pPr><w:jc w:val="{align}"/></w:pPr><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:body>
</w:document>"#
    );

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("word/document.xml", options).unwrap();
        writer.write_all(document.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    buffer
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
#[ignore = "needs git and a shell that can spawn the textconv command"]
fn git_diff_shows_an_alignment_change() {
    let dir = temp_dir("textconv");
    std::fs::write(dir.join("report.docx"), docx("center", "Hello")).expect("write fixture");

    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "offdiff@example.invalid"]);
    git(&dir, &["config", "user.name", "OffDiff Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "v1"]);

    let binary = env!("CARGO_BIN_EXE_offdiff");
    let init = Command::new(binary)
        .args(["git-init", "--bin", binary])
        .current_dir(&dir)
        .output()
        .expect("run offdiff git-init");
    assert!(
        init.status.success(),
        "git-init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    std::fs::write(dir.join("report.docx"), docx("right", "Hello")).expect("write v2");

    let diff = git(&dir, &["diff", "--", "report.docx"]);
    let text = String::from_utf8_lossy(&diff.stdout);
    assert!(
        text.contains(r#""align":"center""#),
        "missing old side:\n{text}"
    );
    assert!(
        text.contains(r#""align":"right""#),
        "missing new side:\n{text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
