//! Contract tests that exercise the built binary, not just the library API.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};

fn temp_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "offdiff-cli-{}-{name}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn docx(text: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:body>
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

fn offdiff() -> Command {
    Command::new(env!("CARGO_BIN_EXE_offdiff"))
}

/// A one-page PDF with a single line of Helvetica text.
fn minimal_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 50 Td (Hello PDF) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];

    let mut output = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let xref = output.len();
    output.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        output.push_str(&format!("{offset:010} 00000 n \n"));
    }
    output.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    output.into_bytes()
}

#[test]
fn textconv_never_fails_on_unsupported_input() {
    let dir = temp_dir("textconv-error");
    let input = dir.join("notes.txt");
    std::fs::write(&input, b"not a document").unwrap();

    let output = offdiff().arg("textconv").arg(&input).output().unwrap();
    assert!(
        output.status.success(),
        "textconv must exit 0 so git diff keeps working"
    );

    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.matches('\n').count(), 1, "one line expected: {text}");
    assert!(text.contains(r#""type":"error""#), "{text}");
    assert!(text.contains(r#""error":"unsupported""#), "{text}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn textconv_output_is_deterministic() {
    let dir = temp_dir("textconv-deterministic");
    let input = dir.join("report.docx");
    std::fs::write(&input, docx("Hello")).unwrap();

    let first: Output = offdiff().arg("textconv").arg(&input).output().unwrap();
    let second = offdiff().arg("textconv").arg(&input).output().unwrap();
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert!(String::from_utf8_lossy(&first.stdout).contains(r#""schema":"offdiff/1""#));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_writes_json_and_markdown() {
    let dir = temp_dir("export");
    let input = dir.join("sample.docx");
    std::fs::write(&input, docx("Hello")).unwrap();
    let out = dir.join("out");

    let output = offdiff()
        .arg("export")
        .arg(&input)
        .arg("--format")
        .arg("both")
        .arg("--out-dir")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = std::fs::read_to_string(out.join("sample.json")).expect("json");
    assert!(json.starts_with("{\n\"schema\":\"offdiff/1\","), "{json}");
    let markdown = std::fs::read_to_string(out.join("sample.md")).expect("markdown");
    assert!(markdown.contains("Hello"), "{markdown}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_writes_a_pdf_projection() {
    let dir = temp_dir("pdf-export");
    let input = dir.join("paper.pdf");
    std::fs::write(&input, minimal_pdf()).unwrap();
    let out = dir.join("out");

    let output = offdiff()
        .arg("export")
        .arg(&input)
        .arg("--format")
        .arg("both")
        .arg("--out-dir")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let markdown = std::fs::read_to_string(out.join("paper.md")).expect("markdown");
    assert!(markdown.contains("Hello PDF"), "{markdown}");
    let json = std::fs::read_to_string(out.join("paper.json")).expect("json");
    assert!(json.contains("\"format\":\"pdf\""), "{json}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_both_requires_out_dir() {
    let dir = temp_dir("export-requires-dir");
    let input = dir.join("sample.docx");
    std::fs::write(&input, docx("Hello")).unwrap();

    let output = offdiff()
        .arg("export")
        .arg(&input)
        .arg("--format")
        .arg("both")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--out-dir"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&dir).ok();
}
