//! Bounded XML/HTML DOM construction and compatibility fallback pruning
//! matching `documentTextWorker.ts:20-161`.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::escape::unescape_with;
use quick_xml::events::{BytesStart, Event};

use crate::doc_extractor::{DOC_MAX_DEPTH, DOC_MAX_XML_NODES, DocInternalErr};
use crate::doc_extractor_entities::resolve_entity_name;
use crate::doc_extractor_odf_ws::{collapse_odf_whitespace, repeat_count};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkupMode {
    Xml,
    Docx,
    Odf,
    Xhtml,
    Html,
}

#[derive(Debug, Clone)]
pub(crate) enum TextPart {
    Text(String),
    CellStart,
    CellEnd,
    Fallback(String),
}

#[derive(Debug, Clone)]
pub(crate) enum XmlChild {
    Element(XmlNode),
    Text(String),
}

#[derive(Debug, Clone)]
pub(crate) struct XmlNode {
    pub(crate) name: String,
    pub(crate) attrs: BTreeMap<String, String>,
    pub(crate) children: Vec<XmlChild>,
    pub(crate) space_count: Option<usize>,
    pub(crate) has_text: bool,
    pub(crate) has_text_choice: bool,
}

impl XmlNode {
    pub(crate) fn empty_root() -> Self {
        Self {
            name: "root".to_owned(),
            attrs: BTreeMap::new(),
            children: Vec::new(),
            space_count: None,
            has_text: false,
            has_text_choice: false,
        }
    }

    pub(crate) fn child_elements(&self) -> impl Iterator<Item = &XmlNode> {
        self.children.iter().filter_map(|child| match child {
            XmlChild::Element(node) => Some(node),
            XmlChild::Text(_) => None,
        })
    }

    pub(crate) fn text_only_children(&self) -> String {
        let mut out = String::new();
        for child in &self.children {
            if let XmlChild::Text(t) = child {
                out.push_str(t);
            }
        }
        out
    }
}

pub(crate) fn is_hidden_tag(name: &str) -> bool {
    matches!(
        name,
        "head"
            | "script"
            | "style"
            | "binary-data"
            | "rPh"
            | "delText"
            | "moveFrom"
            | "instrText"
            | "delInstrText"
            | "tracked-changes"
            | "pPr"
            | "positionH"
            | "positionV"
            | "sizeRelH"
            | "sizeRelV"
            | "fldData"
    )
}

fn is_html_void_tag(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

pub(crate) fn parse_xml(source: &str, mode: MarkupMode) -> Result<XmlNode, DocInternalErr> {
    let mut reader = Reader::from_str(source);
    let cfg = reader.config_mut();
    cfg.trim_text(false);
    if mode == MarkupMode::Html {
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.allow_dangling_amp = true;
    }

    let mut stack: Vec<XmlNode> = vec![XmlNode::empty_root()];
    let mut node_count = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                let name = normalize_tag_name(e.name().as_ref(), mode);
                let void_html = mode == MarkupMode::Html && is_html_void_tag(&name);
                let node = build_node(&name, e, mode, &mut node_count, stack.len())?;
                if void_html {
                    push_and_close_empty(&mut stack, node, mode)?;
                } else {
                    stack.push(node);
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = normalize_tag_name(e.name().as_ref(), mode);
                let node = build_node(&name, e, mode, &mut node_count, stack.len())?;
                push_and_close_empty(&mut stack, node, mode)?;
            }
            Ok(Event::End(ref e)) => {
                let close_name = normalize_tag_name(e.name().as_ref(), mode);
                if mode == MarkupMode::Html {
                    while stack.len() > 1 {
                        let is_match = stack.last().is_some_and(|top| top.name == close_name);
                        close_top(&mut stack, mode);
                        if is_match {
                            break;
                        }
                    }
                } else if stack.len() > 1 {
                    close_top(&mut stack, mode);
                }
            }
            Ok(Event::Text(e)) => push_text(&mut stack, e.as_ref()),
            Ok(Event::CData(e)) => push_text(&mut stack, e.as_ref()),
            Ok(Event::GeneralRef(e)) => {
                if let Some(ch) = e
                    .resolve_char_ref()
                    .map_err(|err| DocInternalErr::Xml(err.to_string()))?
                {
                    let mut buf = [0u8; 4];
                    push_text(&mut stack, ch.encode_utf8(&mut buf));
                } else {
                    let entity = e.as_ref();
                    if let Some(resolved) = resolve_entity_name(entity) {
                        push_text(&mut stack, resolved);
                    } else if mode == MarkupMode::Html {
                        push_text(&mut stack, &format!("&{entity};"));
                    } else {
                        return Err(DocInternalErr::Xml(format!(
                            "unknown XML entity: &{entity};"
                        )));
                    }
                }
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_)) => {}
            Err(err) => {
                if mode == MarkupMode::Html {
                    break;
                }
                return Err(DocInternalErr::Xml(err.to_string()));
            }
        }
    }

    while stack.len() > 1 {
        close_top(&mut stack, mode);
    }
    Ok(stack.pop().unwrap_or_else(XmlNode::empty_root))
}

fn normalize_tag_name(raw: &str, mode: MarkupMode) -> String {
    let local = match raw.rsplit_once(':') {
        Some((_, suffix)) => suffix,
        None => raw,
    };
    if mode == MarkupMode::Html {
        local.to_ascii_lowercase()
    } else {
        local.to_owned()
    }
}

fn build_node(
    name: &str,
    e: &BytesStart<'_>,
    mode: MarkupMode,
    node_count: &mut usize,
    depth: usize,
) -> Result<XmlNode, DocInternalErr> {
    *node_count += 1;
    if *node_count > DOC_MAX_XML_NODES || depth > DOC_MAX_DEPTH {
        return Err(DocInternalErr::Truncated);
    }
    let mut attrs = BTreeMap::new();
    for attr_res in e.attributes() {
        let attr = match attr_res {
            Ok(attr) => attr,
            Err(_) if mode == MarkupMode::Html => continue,
            Err(err) => return Err(DocInternalErr::Xml(err.to_string())),
        };
        let key = attr.key.as_ref().to_owned();
        let val = unescape_with(&attr.value, resolve_entity_name).map_or_else(
            |_| attr.value.as_ref().to_owned(),
            std::borrow::Cow::into_owned,
        );
        let local_key = match key.rsplit_once(':') {
            Some((_, suffix)) => suffix.to_owned(),
            None => key.clone(),
        };
        attrs.insert(key, val.clone());
        attrs.insert(local_key, val);
    }

    let space_count = if mode == MarkupMode::Odf && name == "s" {
        Some(repeat_count(attrs.get("c").map(String::as_str))?)
    } else {
        None
    };

    Ok(XmlNode {
        name: name.to_owned(),
        attrs,
        children: Vec::new(),
        space_count,
        has_text: false,
        has_text_choice: false,
    })
}

fn push_and_close_empty(
    stack: &mut Vec<XmlNode>,
    node: XmlNode,
    mode: MarkupMode,
) -> Result<(), DocInternalErr> {
    if stack.len() + 1 > DOC_MAX_DEPTH {
        return Err(DocInternalErr::Truncated);
    }
    stack.push(node);
    close_top(stack, mode);
    Ok(())
}

fn push_text(stack: &mut [XmlNode], text: &str) {
    if let Some(top) = stack.last_mut() {
        top.children.push(XmlChild::Text(text.to_owned()));
        if !text.trim().is_empty() {
            top.has_text = true;
        }
    }
}

fn close_top(stack: &mut Vec<XmlNode>, mode: MarkupMode) {
    let Some(mut node) = stack.pop() else {
        return;
    };
    let Some(parent) = stack.last_mut() else {
        return;
    };
    if mode == MarkupMode::Odf && matches!(node.name.as_str(), "p" | "h") {
        collapse_odf_whitespace(&mut node);
    }
    if (mode == MarkupMode::Docx && node.name == "del")
        || (mode == MarkupMode::Odf && node.name == "annotation" && parent.name != "semantics")
    {
        return;
    }
    if node.name == "Fallback" && !retain_compatibility_fallback(parent, &node) {
        return;
    }
    let has_text = node.has_text;
    let node_name = node.name.clone();
    parent.children.push(XmlChild::Element(node));
    if is_hidden_tag(&node_name) || (!has_text && node_name != "noBreakHyphen") {
        return;
    }
    parent.has_text = true;
    if node_name == "Choice" {
        parent.has_text_choice = true;
    }
}

fn retain_compatibility_fallback(parent: &mut XmlNode, fallback: &XmlNode) -> bool {
    if parent.has_text_choice {
        return false;
    }
    if fallback.has_text || has_inline_tab(fallback, &["tab", "ptab"]) {
        parent.children.retain(|child| match child {
            XmlChild::Text(_) => true,
            XmlChild::Element(el) => el.name != "Choice" || !has_inline_tab(el, &["ptab"]),
        });
    }
    true
}

fn has_inline_tab(node: &XmlNode, names: &[&str]) -> bool {
    !is_hidden_tag(&node.name)
        && (names.contains(&node.name.as_str())
            || node
                .child_elements()
                .any(|child| has_inline_tab(child, names)))
}
