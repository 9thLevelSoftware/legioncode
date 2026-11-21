//! Tests for `t3_persistence::doc_extractor` across HTML/XHTML, Office, ODF, EPUB, `.ipynb`, and `.rtf`.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use t3_persistence::doc_extractor::{
    DOC_TRUNCATED_SUFFIX, DocExtractError, extract_document_text, extract_html_fragment_text,
};
use zip::CompressionMethod;
use zip::write::FileOptions;

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("create test archive");
    let mut writer = zip::ZipWriter::new(file);
    let opts = FileOptions::default().compression_method(CompressionMethod::Stored);
    for &(name, data) in entries {
        writer.start_file(name, opts).expect("start zip entry");
        writer.write_all(data).expect("write zip entry");
    }
    writer.finish().expect("finish zip");
}

fn write_stored_zip_raw(
    path: &Path,
    entries: &[(&str, &[u8])],
    override_entry_count: Option<u16>,
    override_first_uncompressed: Option<u32>,
) {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (idx, &(name, data)) in entries.iter().enumerate() {
        let offset = u32::try_from(local.len()).expect("offset fits");
        let name_bytes = name.as_bytes();
        let data_len = u32::try_from(data.len()).expect("len fits");
        let declared_len = if idx == 0 {
            override_first_uncompressed.unwrap_or(data_len)
        } else {
            data_len
        };
        let name_len = u16::try_from(name_bytes.len()).expect("name fits");
        let crc = crc32_ieee(data);

        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&data_len.to_le_bytes());
        local.extend_from_slice(&data_len.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name_bytes);
        local.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&data_len.to_le_bytes());
        central.extend_from_slice(&declared_len.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let cd_offset = u32::try_from(local.len()).expect("cd offset fits");
    let cd_size = u32::try_from(central.len()).expect("cd size fits");
    let count =
        override_entry_count.unwrap_or_else(|| u16::try_from(entries.len()).expect("count"));
    let mut eocd = Vec::new();
    eocd.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&cd_size.to_le_bytes());
    eocd.extend_from_slice(&cd_offset.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());

    local.extend_from_slice(&central);
    local.extend_from_slice(&eocd);
    fs::write(path, local).expect("write raw zip");
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = if crc & 1 != 0 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

#[test]
fn rejects_top_level_html_extensions_and_extracts_html_fragments_and_ipynb() {
    let dir = tempfile::tempdir().expect("tempdir");
    let html_bytes = b"<!DOCTYPE html><html><head><title>Hidden</title><meta charset=\"utf-8\"><style>.x{}</style></head>\
         <body><h1>Hello &mdash; World</h1><p>One<br>Two&nbsp;Three &NotEqualTilde; &CounterClockwiseContourIntegral;</p>\
         <script>const z = 1 < 2;</script><div>Tail</div></body></html>";

    for ext in [".html", "htm", ".xhtml"] {
        let path = dir
            .path()
            .join(format!("page.{ext}", ext = ext.trim_start_matches('.')));
        fs::write(&path, html_bytes).expect("write html");
        let err = extract_document_text(&path, ext).expect_err("top-level html rejects");
        assert!(matches!(err, DocExtractError::Unsupported(_)));
        assert_eq!(err.exit_code(), 2);
    }

    let frag_out = extract_html_fragment_text(html_bytes, false).expect("extract html fragment");
    assert_eq!(
        frag_out,
        "# Hello \u{2014} World\nOne\nTwo\u{00A0}Three \u{2242}\u{0338} \u{2233}\nTail"
    );

    let bad_xhtml_err =
        extract_html_fragment_text(b"<html><p>Unclosed</html>", true).expect_err("broken xhtml");
    assert_eq!(bad_xhtml_err.exit_code(), 2);

    let ipynb_path = dir.path().join("nb.ipynb");
    fs::write(
        &ipynb_path,
        r##"{"cells":[{"cell_type":"markdown","source":["# Stage 7 Title"]},{"cell_type":"code","source":["print('ok')"],"outputs":[{"output_type":"stream","text":["\u001b[32mok\u001b[0m\n"]}]}]}"##,
    )
    .expect("write ipynb");
    let ipynb_out = extract_document_text(&ipynb_path, ".ipynb").expect("ipynb");
    assert_eq!(
        ipynb_out,
        "## Cell 1 (markdown)\n# Stage 7 Title\n\n## Cell 2 (code)\nprint('ok')\nOutput:\nok"
    );

    let empty_nb = dir.path().join("empty.ipynb");
    fs::write(&empty_nb, r#"{"cells":[]}"#).expect("write empty ipynb");
    let empty_err = extract_document_text(&empty_nb, ".ipynb").expect_err("empty notebook rejects");
    assert!(matches!(empty_err, DocExtractError::NoTextExtracted));
    assert_eq!(empty_err.exit_code(), 2);
}

#[test]
fn extracts_docx_xlsx_and_pptx_with_structure_and_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let docx = dir.path().join("sample.docx");
    write_zip(
        &docx,
        &[
            (
                "word/document.xml",
                br#"<w:document><w:body><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:t>Report</w:t></w:p><w:p><w:t>Keep </w:t><w:del><w:delText>$500</w:delText></w:del><w:t>$700</w:t><w:noBreakHyphen/><w:t>net</w:t></w:p><w:tbl><w:tr><w:tc><w:p><w:t>A</w:t></w:p></w:tc><w:tc><w:p><w:t>B</w:t></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
            ),
            ("word/embeddings/unused.bin", &vec![0u8; 64 * 1024]),
        ],
    );
    let docx_out = extract_document_text(&docx, ".docx").expect("docx");
    assert_eq!(docx_out, "## Report\nKeep $700\u{2011}net\nA\tB");

    let xlsx = dir.path().join("book.xlsx");
    write_zip(
        &xlsx,
        &[
            (
                "xl/workbook.xml",
                br#"<workbook><sheets><sheet name="Escapes" r:id="a"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                br#"<Relationships><Relationship Id="a" Target="worksheets/a.xml"/></Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                br"<sst><si><t>first_x000D_second</t></si><si><t>_x005F_x0041_</t></si></sst>",
            ),
            (
                "xl/worksheets/a.xml",
                br#"<worksheet><row><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c><c r="D1" t="b"><v>1</v></c></row></worksheet>"#,
            ),
        ],
    );
    let xlsx_out = extract_document_text(&xlsx, ".xlsx").expect("xlsx");
    assert_eq!(xlsx_out, "## Sheet: Escapes\nfirst second\t\t_x0041_\ttrue");

    let pptx = dir.path().join("deck.pptx");
    write_zip(
        &pptx,
        &[
            (
                "ppt/presentation.xml",
                br#"<p:presentation><p:sldIdLst><p:sldId r:id="b"/><p:sldId r:id="a"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                br#"<Relationships><Relationship Id="a" Target="slides/s1.xml"/><Relationship Id="b" Target="slides/s2.xml"/></Relationships>"#,
            ),
            ("ppt/slides/s2.xml", b"<p:sld><a:p><a:t>Juniper</a:t></a:p></p:sld>"),
            ("ppt/slides/s1.xml", b"<p:sld><a:p><a:t>Gate 4</a:t></a:p></p:sld>"),
            (
                "ppt/slides/_rels/s2.xml.rels",
                br#"<Relationships><Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/n1.xml"/></Relationships>"#,
            ),
            ("ppt/notesSlides/n1.xml", b"<p:notes><a:p><a:t>ORCHID-81</a:t></a:p></p:notes>"),
        ],
    );
    let pptx_out = extract_document_text(&pptx, ".pptx").expect("pptx");
    assert!(pptx_out.contains("## Slide 1\nJuniper\nNotes:\nORCHID-81"));
    assert!(pptx_out.contains("## Slide 2\nGate 4"));
}

#[test]
fn extracts_odf_and_utf16_epub_and_enforces_limits_and_exit_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let odt = dir.path().join("note.odt");
    write_zip(
        &odt,
        &[(
            "content.xml",
            br#"<office:document><text:h text:outline-level="2">  Orchard </text:h><text:p>19<text:s text:c="3"/>violet<text:s/>crates<text:tab/>Dock</text:p></office:document>"#,
        )],
    );
    assert_eq!(
        extract_document_text(&odt, ".odt").expect("odt"),
        "## Orchard\n19   violet crates\tDock"
    );

    let utf16_chap: Vec<u8> = "\u{FEFF}<html><p>UTF-16 café 日本語</p></html>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let epub = dir.path().join("book.epub");
    write_zip(
        &epub,
        &[
            (
                "META-INF/container.xml",
                br#"<container><rootfile full-path="OEBPS/book.opf"/></container>"#,
            ),
            (
                "OEBPS/book.opf",
                br#"<package><item id="bad" href="ch%ZZ.xhtml"/><item id="ok" href="ch1.xhtml"/><itemref idref="bad"/><itemref idref="ok"/></package>"#,
            ),
            ("OEBPS/ch1.xhtml", &utf16_chap),
        ],
    );
    let epub_out = extract_document_text(&epub, ".epub").expect("epub");
    assert!(epub_out.contains("## Chapter 2\nUTF-16 café 日本語"));
    assert!(epub_out.contains("[Skipped document part: chapter 1 (missing or invalid XML).]"));

    let zip_bomb = dir.path().join("bomb.docx");
    write_stored_zip_raw(
        &zip_bomb,
        &[("word/document.xml", b"<document><p>Ok</p></document>")],
        None,
        Some(33 * 1024 * 1024),
    );
    assert!(
        extract_document_text(&zip_bomb, ".docx")
            .expect("truncated")
            .contains(DOC_TRUNCATED_SUFFIX.trim())
    );

    let deep_docx = dir.path().join("deep.docx");
    let deep_xml = format!(
        "<document>{}Lost{}</document>",
        "<div>".repeat(129),
        "</div>".repeat(129)
    );
    write_zip(&deep_docx, &[("word/document.xml", deep_xml.as_bytes())]);
    let deep_out = extract_document_text(&deep_docx, ".docx").expect("deep truncated");
    assert!(deep_out.contains("truncated"));
    assert!(!deep_out.contains("Lost"));

    let missing_err = extract_document_text(&dir.path().join("missing.docx"), ".docx")
        .expect_err("missing IO error");
    assert!(matches!(missing_err, DocExtractError::Io(_)));
    assert_eq!(missing_err.exit_code(), 1);

    let pdf_err = extract_document_text(&odt, ".pdf").expect_err("unsupported pdf");
    assert!(matches!(pdf_err, DocExtractError::Unsupported(_)));
    assert_eq!(pdf_err.exit_code(), 2);

    let rtf_path = dir.path().join("cyr.rtf");
    let mut rtf_bytes =
        br"{\rtf1\ansi\ansicpg1251 \'cf\'f0\'e8\'e2\'e5\'f2 {\ansicpg1252 \'e9} ".to_vec();
    rtf_bytes.extend_from_slice(&[0xCC, 0xEE, 0xF1, 0xEA, 0xE2, 0xE0, b'}']);
    fs::write(&rtf_path, rtf_bytes).expect("write rtf");
    assert_eq!(
        extract_document_text(&rtf_path, ".rtf").expect("rtf"),
        "Привет é Москва"
    );

    let mac_rtf_path = dir.path().join("macroman.rtf");
    let mac_rtf = br"{\rtf1\mac\ansicpg10000 Cafe\'8e \emdash  \'d2Apple\'d3 \'f0 {\deleted old typo }kept{\*\deleted hidden}\deleted wiped\deleted0  clean}";
    fs::write(&mac_rtf_path, mac_rtf).expect("write mac rtf");
    let mac_out = extract_document_text(&mac_rtf_path, ".rtf").expect("mac rtf");
    assert_eq!(mac_out, "Cafeé — “Apple” \u{F8FF} kept clean");
}
