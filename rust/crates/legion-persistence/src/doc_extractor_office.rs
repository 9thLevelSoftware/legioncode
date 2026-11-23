//! Bounded Office/EPUB ZIP archive readers and format dispatchers
//! matching `documentTextWorker.ts:285-361,554-739`.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use zip::ZipArchive;
use zip::result::ZipError;

use crate::doc_extractor::{DOC_MAX_EXPANDED_BYTES, DocInternalErr};
use crate::doc_extractor_entities::{
    decode_markup_bytes, percent_decode_href, posix_basename, posix_dirname, posix_join,
    posix_normalize,
};
use crate::doc_extractor_layout::{emit_paragraphs, emit_table, has_cell_text};
use crate::doc_extractor_odf_ws::{all_matching, first_matching};
use crate::doc_extractor_xlsx::extract_xlsx;
use crate::doc_extractor_xml::{MarkupMode, TextPart, XmlChild, XmlNode, parse_xml};

const MAX_ARCHIVE_ENTRIES: usize = 2048;

pub(crate) fn read_bounded_archive(
    path: &Path,
) -> Result<BTreeMap<String, String>, DocInternalErr> {
    let file = File::open(path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|err| DocInternalErr::Archive(err.to_string()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(DocInternalErr::Truncated);
    }

    let mut files = BTreeMap::new();
    let mut expanded_total = 0u64;

    for idx in 0..archive.len() {
        let entry = match archive.by_index(idx) {
            Ok(f) => f,
            Err(ZipError::UnsupportedArchive(msg))
                if msg.to_ascii_lowercase().contains("encrypt") =>
            {
                return Err(DocInternalErr::Archive("Encrypted archive".to_owned()));
            }
            Err(err) => return Err(DocInternalErr::Archive(err.to_string())),
        };

        let name = entry.name().to_owned();
        if !is_xml_archive_entry(&name) {
            continue;
        }
        let uncompressed = entry.size();
        let compressed = entry.compressed_size();
        let ratio_cap = compressed.saturating_mul(200).max(1024);
        if uncompressed > DOC_MAX_EXPANDED_BYTES || uncompressed > ratio_cap {
            return Err(DocInternalErr::Truncated);
        }
        expanded_total = expanded_total.saturating_add(uncompressed);
        if expanded_total > DOC_MAX_EXPANDED_BYTES {
            return Err(DocInternalErr::Truncated);
        }

        let read_cap = uncompressed.min(DOC_MAX_EXPANDED_BYTES);
        let mut buf = Vec::with_capacity(usize::try_from(read_cap).unwrap_or(0).min(65_536));
        let mut limited = entry.take(read_cap + 1);
        limited.read_to_end(&mut buf)?;
        if u64::try_from(buf.len()).unwrap_or(u64::MAX) > read_cap {
            return Err(DocInternalErr::Truncated);
        }
        files.insert(name, decode_markup_bytes(&buf));
    }

    Ok(files)
}

fn is_xml_archive_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".xml", ".rels", ".xhtml", ".html", ".htm", ".opf", ".svg"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

pub(crate) fn extract_office_or_epub(
    files: &BTreeMap<String, String>,
    ext: &str,
    parts: &mut Vec<TextPart>,
) -> Result<(), DocInternalErr> {
    let mode = match ext {
        "docx" => MarkupMode::Docx,
        "odt" | "ods" => MarkupMode::Odf,
        "odp" => MarkupMode::Odp,
        _ => MarkupMode::Xml,
    };
    let mut notes: Vec<String> = Vec::new();

    match ext {
        "docx" => {
            let main_part = resolve_docx_main_part(files, &mut notes)?;
            let doc = read_part(files, &main_part, mode, &mut notes)?;
            emit_paragraphs(&doc, parts)?;
        }
        "odt" => {
            let doc = read_part(files, "content.xml", mode, &mut notes)?;
            emit_paragraphs(&doc, parts)?;
        }
        "ods" | "odp" => {
            let doc = read_part(files, "content.xml", mode, &mut notes)?;
            let tag = if ext == "ods" { "table" } else { "page" };
            let mut containers = Vec::new();
            all_matching(&doc, &[tag], &mut containers);
            for (idx, item) in containers.into_iter().enumerate() {
                if ext == "ods" {
                    let sheet = item.attrs.get("name").map_or("Untitled", String::as_str);
                    parts.push(TextPart::Text(format!("\n\n## Sheet: {sheet}\n")));
                    emit_table(item, parts)?;
                    continue;
                }
                parts.push(TextPart::Text(format!("\n\n## Slide {}\n", idx + 1)));
                let notes_idx = item
                    .children
                    .iter()
                    .position(|c| matches!(c, XmlChild::Element(n) if n.name == "notes"));
                let (slide_body, slide_notes) = match notes_idx {
                    Some(pos) => {
                        let notes_node = match &item.children[pos] {
                            XmlChild::Element(n) => Some(n),
                            XmlChild::Text(_) => None,
                        };
                        let mut stripped = item.clone();
                        stripped.children.remove(pos);
                        (Some(stripped), notes_node)
                    }
                    None => (None, None),
                };
                emit_paragraphs(slide_body.as_ref().unwrap_or(item), parts)?;
                if let Some(notes_node) = slide_notes
                    && has_cell_text(notes_node)
                {
                    parts.push(TextPart::Text("\nNotes:\n".to_owned()));
                    emit_paragraphs(notes_node, parts)?;
                }
            }
        }
        "xlsx" => extract_xlsx(files, parts, &mut notes)?,
        "pptx" => extract_pptx(files, parts, &mut notes)?,
        "epub" => extract_epub(files, parts, &mut notes)?,
        _ => return Err(DocInternalErr::Unsupported(ext.to_owned())),
    }

    for note in notes {
        parts.push(TextPart::Text(note));
    }
    Ok(())
}

pub(crate) fn read_part(
    files: &BTreeMap<String, String>,
    name: &str,
    mode: MarkupMode,
    notes: &mut Vec<String>,
) -> Result<XmlNode, DocInternalErr> {
    let Some(raw) = files.get(name) else {
        notes.push(format!(
            "\n[Skipped document part: {name} (missing or invalid XML).]\n"
        ));
        return Ok(XmlNode::empty_root());
    };
    match parse_xml(raw, mode) {
        Ok(tree) => Ok(tree),
        Err(DocInternalErr::Truncated) => Err(DocInternalErr::Truncated),
        Err(_) => {
            notes.push(format!(
                "\n[Skipped document part: {name} (missing or invalid XML).]\n"
            ));
            Ok(XmlNode::empty_root())
        }
    }
}

pub(crate) fn read_relations(
    files: &BTreeMap<String, String>,
    name: &str,
    notes: &mut Vec<String>,
) -> Result<HashMap<String, String>, DocInternalErr> {
    let tree = read_part(files, name, MarkupMode::Xml, notes)?;
    let mut rel_nodes = Vec::new();
    all_matching(&tree, &["Relationship"], &mut rel_nodes);
    let mut map = HashMap::new();
    for rel in rel_nodes {
        if rel.attrs.get("TargetMode").map(String::as_str) == Some("External") {
            continue;
        }
        if let (Some(id), Some(target)) = (rel.attrs.get("Id"), rel.attrs.get("Target")) {
            map.insert(id.clone(), target.clone());
        }
    }
    Ok(map)
}

fn extract_pptx(
    files: &BTreeMap<String, String>,
    parts: &mut Vec<TextPart>,
    notes: &mut Vec<String>,
) -> Result<(), DocInternalErr> {
    let rels = read_relations(files, "ppt/_rels/presentation.xml.rels", notes)?;
    let pres_root = read_part(files, "ppt/presentation.xml", MarkupMode::Xml, notes)?;
    let Some(presentation) = pres_root.child_elements().next() else {
        return Ok(());
    };
    let Some(sld_lst) = presentation.child_elements().find(|n| n.name == "sldIdLst") else {
        return Ok(());
    };

    for (idx, slide) in sld_lst
        .child_elements()
        .filter(|n| n.name == "sldId")
        .enumerate()
    {
        let slide_num = idx + 1;
        parts.push(TextPart::Text(format!("\n\n## Slide {slide_num}\n")));
        let rel_id = slide
            .attrs
            .get("r:id")
            .or_else(|| slide.attrs.get("id"))
            .map(String::as_str);
        let Some(target) = rel_id.and_then(|id| rels.get(id)) else {
            notes.push(format!(
                "\n[Skipped document part: slide relationship {slide_num} (missing or invalid XML).]\n"
            ));
            continue;
        };
        let slide_path = posix_join("ppt", target);
        let slide_doc = read_part(files, &slide_path, MarkupMode::Xml, notes)?;
        emit_paragraphs(&slide_doc, parts)?;

        let dir = posix_dirname(&slide_path);
        let base = posix_basename(&slide_path);
        let slide_rels = posix_join(dir, &format!("_rels/{base}.rels"));
        if !files.contains_key(&slide_rels) {
            continue;
        }
        let rel_tree = read_part(files, &slide_rels, MarkupMode::Xml, notes)?;
        let mut rel_nodes = Vec::new();
        all_matching(&rel_tree, &["Relationship"], &mut rel_nodes);
        for rel in rel_nodes {
            let is_notes = rel
                .attrs
                .get("Type")
                .is_some_and(|t| t.ends_with("/notesSlide"));
            let is_ext = rel.attrs.get("TargetMode").map(String::as_str) == Some("External");
            let Some(note_target) = rel.attrs.get("Target") else {
                continue;
            };
            if !is_notes || is_ext || note_target.is_empty() {
                continue;
            }
            let note_path = posix_join(dir, note_target);
            let note_doc = read_part(files, &note_path, MarkupMode::Xml, notes)?;
            let notes_page = without_notes_page_placeholders(&note_doc);
            if !has_cell_text(&notes_page) {
                continue;
            }
            parts.push(TextPart::Text("Notes:\n".to_owned()));
            emit_paragraphs(&notes_page, parts)?;
        }
    }
    Ok(())
}

// Package relationship names the Word main part; Word for the web saves word/document2.xml (#40777).
fn resolve_docx_main_part(
    files: &BTreeMap<String, String>,
    notes: &mut Vec<String>,
) -> Result<String, DocInternalErr> {
    if files.contains_key("_rels/.rels") {
        let rel_tree = read_part(files, "_rels/.rels", MarkupMode::Xml, notes)?;
        let mut rel_nodes = Vec::new();
        all_matching(&rel_tree, &["Relationship"], &mut rel_nodes);
        if let Some(target) = rel_nodes.into_iter().find_map(|rel| {
            let is_office_doc = rel
                .attrs
                .get("Type")
                .is_some_and(|t| t.ends_with("/officeDocument"));
            let is_ext = rel.attrs.get("TargetMode").map(String::as_str) == Some("External");
            if is_office_doc && !is_ext {
                rel.attrs.get("Target").cloned()
            } else {
                None
            }
        }) {
            let normalized = posix_normalize(&target);
            let main_part = normalized.trim_start_matches('/');
            if !main_part.is_empty() && files.contains_key(main_part) {
                return Ok(main_part.to_owned());
            }
        }
    }
    Ok("word/document.xml".to_owned())
}

// A PPTX notes page also holds slide image, slide number, date, header, and footer placeholders (#40568).
fn is_notes_page_placeholder(node: &XmlNode) -> bool {
    if node.name != "sp" {
        return false;
    }
    let Some(ph) = first_matching(node, "ph") else {
        return false;
    };
    matches!(
        ph.attrs.get("type").map(String::as_str),
        Some("sldImg" | "sldNum" | "dt" | "hdr" | "ftr")
    )
}

fn without_notes_page_placeholders(node: &XmlNode) -> XmlNode {
    XmlNode {
        name: node.name.clone(),
        attrs: node.attrs.clone(),
        children: node
            .children
            .iter()
            .filter_map(|child| match child {
                XmlChild::Text(text) => Some(XmlChild::Text(text.clone())),
                XmlChild::Element(elem) => (!is_notes_page_placeholder(elem))
                    .then(|| XmlChild::Element(without_notes_page_placeholders(elem))),
            })
            .collect(),
        space_count: node.space_count,
        has_text: node.has_text,
        has_text_choice: node.has_text_choice,
    }
}

fn extract_epub(
    files: &BTreeMap<String, String>,
    parts: &mut Vec<TextPart>,
    notes: &mut Vec<String>,
) -> Result<(), DocInternalErr> {
    let container = read_part(files, "META-INF/container.xml", MarkupMode::Xml, notes)?;
    let rootfile = first_matching(&container, "rootfile")
        .and_then(|n| n.attrs.get("full-path"))
        .cloned();
    let Some(rootfile_path) = rootfile else {
        if notes.is_empty() {
            notes.push(
                "\n[Skipped document part: EPUB package (missing or invalid XML).]\n".to_owned(),
            );
        }
        return Ok(());
    };

    let package_xml = read_part(files, &rootfile_path, MarkupMode::Xml, notes)?;
    let mut item_nodes = Vec::new();
    all_matching(&package_xml, &["item"], &mut item_nodes);
    let mut items: HashMap<String, String> = HashMap::new();
    for item in item_nodes {
        if let (Some(id), Some(href)) = (item.attrs.get("id"), item.attrs.get("href")) {
            items.insert(id.clone(), href.clone());
        }
    }

    let mut itemrefs = Vec::new();
    all_matching(&package_xml, &["itemref"], &mut itemrefs);
    let root_dir = posix_dirname(&rootfile_path);
    for (idx, itemref) in itemrefs.into_iter().enumerate() {
        let chapter_num = idx + 1;
        parts.push(TextPart::Text(format!("\n\n## Chapter {chapter_num}\n")));
        let Some(href) = itemref.attrs.get("idref").and_then(|id| items.get(id)) else {
            notes.push(format!(
                "\n[Skipped document part: chapter {chapter_num} (missing or invalid XML).]\n"
            ));
            continue;
        };
        let Some(decoded) = percent_decode_href(href) else {
            notes.push(format!(
                "\n[Skipped document part: chapter {chapter_num} (missing or invalid XML).]\n"
            ));
            continue;
        };
        let chapter_path = posix_join(root_dir, &decoded);
        let chapter = read_part(files, &chapter_path, MarkupMode::Xhtml, notes)?;
        emit_paragraphs(&chapter, parts)?;
    }
    Ok(())
}
