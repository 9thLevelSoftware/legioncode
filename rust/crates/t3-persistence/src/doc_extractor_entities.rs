//! HTML/XML character entity decoding, UTF-16 BOM/XML encoding detection, and POSIX archive path helpers.

const HTML_NAMED_ENTITIES: &[(&str, &str)] = &[
    ("nbsp", "\u{00A0}"),
    ("iexcl", "\u{00A1}"),
    ("cent", "\u{00A2}"),
    ("pound", "\u{00A3}"),
    ("curren", "\u{00A4}"),
    ("yen", "\u{00A5}"),
    ("brvbar", "\u{00A6}"),
    ("sect", "\u{00A7}"),
    ("uml", "\u{00A8}"),
    ("copy", "\u{00A9}"),
    ("ordf", "\u{00AA}"),
    ("laquo", "\u{00AB}"),
    ("not", "\u{00AC}"),
    ("shy", "\u{00AD}"),
    ("reg", "\u{00AE}"),
    ("macr", "\u{00AF}"),
    ("deg", "\u{00B0}"),
    ("plusmn", "\u{00B1}"),
    ("sup2", "\u{00B2}"),
    ("sup3", "\u{00B3}"),
    ("acute", "\u{00B4}"),
    ("micro", "\u{00B5}"),
    ("para", "\u{00B6}"),
    ("middot", "\u{00B7}"),
    ("cedil", "\u{00B8}"),
    ("sup1", "\u{00B9}"),
    ("ordm", "\u{00BA}"),
    ("raquo", "\u{00BB}"),
    ("frac14", "\u{00BC}"),
    ("frac12", "\u{00BD}"),
    ("frac34", "\u{00BE}"),
    ("iquest", "\u{00BF}"),
    ("times", "\u{00D7}"),
    ("divide", "\u{00F7}"),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("sbquo", "\u{201A}"),
    ("ldquo", "\u{201C}"),
    ("rdquo", "\u{201D}"),
    ("bdquo", "\u{201E}"),
    ("dagger", "\u{2020}"),
    ("Dagger", "\u{2021}"),
    ("bull", "\u{2022}"),
    ("hellip", "\u{2026}"),
    ("permil", "\u{2030}"),
    ("prime", "\u{2032}"),
    ("Prime", "\u{2033}"),
    ("lsaquo", "\u{2039}"),
    ("rsaquo", "\u{203A}"),
    ("oline", "\u{203E}"),
    ("frasl", "\u{2044}"),
    ("euro", "\u{20AC}"),
    ("trade", "\u{2122}"),
    ("larr", "\u{2190}"),
    ("uarr", "\u{2191}"),
    ("rarr", "\u{2192}"),
    ("darr", "\u{2193}"),
    ("harr", "\u{2194}"),
    ("crarr", "\u{21B5}"),
    ("lArr", "\u{21D0}"),
    ("rArr", "\u{21D2}"),
    ("hArr", "\u{21D4}"),
    ("forall", "\u{2200}"),
    ("part", "\u{2202}"),
    ("exist", "\u{2203}"),
    ("empty", "\u{2205}"),
    ("nabla", "\u{2207}"),
    ("isin", "\u{2208}"),
    ("notin", "\u{2209}"),
    ("ni", "\u{220B}"),
    ("prod", "\u{220F}"),
    ("sum", "\u{2211}"),
    ("minus", "\u{2212}"),
    ("lowast", "\u{2217}"),
    ("radic", "\u{221A}"),
    ("prop", "\u{221D}"),
    ("infin", "\u{221E}"),
    ("ang", "\u{2220}"),
    ("and", "\u{2227}"),
    ("or", "\u{2228}"),
    ("cap", "\u{2229}"),
    ("cup", "\u{222A}"),
    ("int", "\u{222B}"),
    ("CounterClockwiseContourIntegral", "\u{2233}"),
    ("there4", "\u{2234}"),
    ("sim", "\u{223C}"),
    ("cong", "\u{2245}"),
    ("asymp", "\u{2248}"),
    ("NotEqualTilde", "\u{2242}\u{0338}"),
    ("ne", "\u{2260}"),
    ("equiv", "\u{2261}"),
    ("le", "\u{2264}"),
    ("ge", "\u{2265}"),
    ("sub", "\u{2282}"),
    ("sup", "\u{2283}"),
    ("nsub", "\u{2284}"),
    ("sube", "\u{2286}"),
    ("supe", "\u{2287}"),
    ("oplus", "\u{2295}"),
    ("otimes", "\u{2297}"),
    ("perp", "\u{22A5}"),
    ("sdot", "\u{22C5}"),
    ("alpha", "\u{03B1}"),
    ("beta", "\u{03B2}"),
    ("gamma", "\u{03B3}"),
    ("delta", "\u{03B4}"),
    ("epsilon", "\u{03B5}"),
    ("theta", "\u{03B8}"),
    ("lambda", "\u{03BB}"),
    ("mu", "\u{03BC}"),
    ("pi", "\u{03C0}"),
    ("sigma", "\u{03C3}"),
    ("phi", "\u{03C6}"),
    ("omega", "\u{03C9}"),
    ("Delta", "\u{0394}"),
    ("Sigma", "\u{03A3}"),
    ("Omega", "\u{03A9}"),
    ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"),
    ("thinsp", "\u{2009}"),
    ("zwnj", "\u{200C}"),
    ("zwj", "\u{200D}"),
    ("lrm", "\u{200E}"),
    ("rlm", "\u{200F}"),
    ("NewLine", "\n"),
    ("Tab", "\t"),
];

/// Resolves XML predefined and common HTML/XHTML named character entities (without `&` or `;`).
pub(crate) fn resolve_entity_name(entity: &str) -> Option<&'static str> {
    if let Some(xml) = quick_xml::escape::resolve_xml_entity(entity) {
        return Some(xml);
    }
    HTML_NAMED_ENTITIES
        .iter()
        .find(|&&(name, _)| name == entity)
        .map(|&(_, value)| value)
}

/// Decodes raw XML/HTML bytes honoring UTF-16LE/BE BOMs and XML UTF-16 byte heuristics
/// (`documentTextWorker.ts:344-350`), falling back to lossy UTF-8.
pub(crate) fn decode_markup_bytes(data: &[u8]) -> String {
    if data.len() >= 2 {
        let little_endian =
            (data[0] == 0xFF && data[1] == 0xFE) || (data[0] == 0x3C && data[1] == 0x00);
        let big_endian =
            (data[0] == 0xFE && data[1] == 0xFF) || (data[0] == 0x00 && data[1] == 0x3C);
        if little_endian || big_endian {
            let start = usize::from(
                (data[0] == 0xFF && data[1] == 0xFE) || (data[0] == 0xFE && data[1] == 0xFF),
            ) * 2;
            let (pairs, _) = data[start..].as_chunks::<2>();
            let units = pairs.iter().map(|&[a, b]| {
                if little_endian {
                    u16::from_le_bytes([a, b])
                } else {
                    u16::from_be_bytes([a, b])
                }
            });
            return char::decode_utf16(units)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect();
        }
    }
    let slice = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    String::from_utf8_lossy(slice).into_owned()
}

/// Normalizes a POSIX-style relative ZIP entry path (e.g. `xl/../xl/worksheets/a.xml`).
pub(crate) fn posix_normalize(path: &str) -> String {
    let leading_slash = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if leading_slash {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

/// Joins two POSIX path segments and normalizes `.` / `..`.
pub(crate) fn posix_join(base: &str, rel: &str) -> String {
    if let Some(stripped) = rel.strip_prefix('/') {
        return posix_normalize(stripped);
    }
    if base.is_empty() || base == "." {
        posix_normalize(rel)
    } else {
        posix_normalize(&format!("{base}/{rel}"))
    }
}

/// Returns the parent directory portion of a POSIX path (`""` for top-level files).
pub(crate) fn posix_dirname(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    }
}

/// Returns the base filename portion of a POSIX path.
pub(crate) fn posix_basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, base)) => base,
        None => path,
    }
}

/// Percent-decodes an EPUB `href` before `#` (matching `decodeURIComponent(href.split("#")[0])`),
/// returning `None` if an invalid `%XX` sequence or invalid UTF-8 is encountered.
pub(crate) fn percent_decode_href(href: &str) -> Option<String> {
    let before_frag = match href.split_once('#') {
        Some((head, _)) => head,
        None => href,
    };
    let bytes = before_frag.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hi = hex_nibble(*bytes.get(idx + 1)?)?;
            let lo = hex_nibble(*bytes.get(idx + 2)?)?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
