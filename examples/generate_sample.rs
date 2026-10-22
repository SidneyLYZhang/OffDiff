use std::env;
use std::fs::File;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// `(text, bold, italic, font size in half-points, font name)`.
type RunSpec<'a> = (&'a str, bool, bool, &'a str, &'a str);
/// `(style id, alignment, runs)`.
type ParagraphSpec<'a> = (&'a str, Option<&'a str>, Vec<RunSpec<'a>>);

fn main() {
    let args: Vec<String> = env::args().collect();
    let output_path = args.get(1).map(|s| s.as_str()).unwrap_or("sample.docx");

    let paragraphs = vec![
        (
            "Heading1",
            Some("center"),
            vec![("OffDiff 演示文档", true, false, "32", "Calibri")],
        ),
        (
            "Normal",
            None,
            vec![(
                "这是一个用于演示 OffDiff 解析能力的示例文档。",
                false,
                false,
                "22",
                "Calibri",
            )],
        ),
        (
            "Normal",
            None,
            vec![(
                "OffDiff 支持提取以下格式信息：",
                false,
                false,
                "22",
                "Calibri",
            )],
        ),
        (
            "Normal",
            None,
            vec![
                ("• 粗体文本", true, false, "22", "Calibri"),
                ("  ", false, false, "22", "Calibri"),
                ("• 斜体文本", false, true, "22", "Calibri"),
                ("  ", false, false, "22", "Calibri"),
                ("• 带下划线的文本", false, false, "22", "Calibri"),
            ],
        ),
        (
            "Normal",
            None,
            vec![
                ("同时支持多种格式的", false, false, "22", "Calibri"),
                ("混合使用", true, true, "24", "Georgia"),
                (
                    "，比如粗体加斜体加不同字体。",
                    false,
                    false,
                    "22",
                    "Calibri",
                ),
            ],
        ),
        (
            "Normal",
            None,
            vec![(
                "段落样式也会被提取，例如标题样式、对齐方式和缩进等。",
                false,
                false,
                "22",
                "Calibri",
            )],
        ),
        (
            "Normal",
            Some("right"),
            vec![("右对齐的段落示例", false, false, "22", "Calibri")],
        ),
        ("Normal", None, vec![("", false, false, "22", "Calibri")]),
        (
            "Normal",
            None,
            vec![("—— OffDiff 团队", false, false, "22", "Calibri")],
        ),
    ];

    let body = build_document_xml(&paragraphs);
    let docx_bytes = create_docx(&body);

    let mut file = File::create(output_path).expect("Failed to create file");
    file.write_all(&docx_bytes).expect("Failed to write file");
    println!("Sample docx generated: {}", output_path);
}

fn build_document_xml(paragraphs: &[ParagraphSpec<'_>]) -> String {
    let mut body = String::new();

    for (style_name, alignment, runs) in paragraphs {
        let mut p_pr = String::new();
        if style_name != &"Normal" || alignment.is_some() {
            p_pr.push_str("<w:pPr>");
            if style_name != &"Normal" {
                p_pr.push_str(&format!("<w:pStyle w:val=\"{}\"/>", style_name));
            }
            if let Some(align) = alignment {
                p_pr.push_str(&format!("<w:jc w:val=\"{}\"/>", align));
            }
            p_pr.push_str("</w:pPr>");
        }

        let mut runs_xml = String::new();
        for (text, bold, italic, size, font) in runs {
            let mut r_pr = String::new();
            if *bold || *italic || !size.is_empty() || !font.is_empty() {
                r_pr.push_str("<w:rPr>");
                if *bold {
                    r_pr.push_str("<w:b/>");
                }
                if *italic {
                    r_pr.push_str("<w:i/>");
                }
                if !size.is_empty() {
                    r_pr.push_str(&format!("<w:sz w:val=\"{}\"/>", size));
                }
                if !font.is_empty() {
                    r_pr.push_str(&format!(
                        "<w:rFonts w:ascii=\"{}\" w:hAnsi=\"{}\"/>",
                        font, font
                    ));
                }
                r_pr.push_str("</w:rPr>");
            }
            runs_xml.push_str(&format!(
                "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                r_pr, text
            ));
        }

        body.push_str(&format!("<w:p>{}{}</w:p>", p_pr, runs_xml));
    }

    body
}

fn create_docx(body: &str) -> Vec<u8> {
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
{}
</w:body>
</w:document>"#,
        body
    );

    let content_types_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    let rels_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(content_types_xml.as_bytes()).unwrap();

        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(rels_xml.as_bytes()).unwrap();

        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();

        zip.finish().unwrap();
    }
    buf
}
