//! Minimal dependency-free HTTP/1.1 loopback client, external URL validation, media fetch,
//! and RFC 4648 base64 codec for the safe Rust desktop shell.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::Duration;

pub use crate::http_codec::{base64_decode, base64_encode};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Parsed `http://host[:port]/path` components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpUrl {
    /// Target hostname or IP literal (without IPv6 brackets).
    pub host: String,
    /// Target TCP port (`80` by default).
    pub port: u16,
    /// `Host` header value preserving port when non-empty.
    pub host_header: String,
    /// Request target starting with `/`.
    pub request_target: String,
}

/// Raw HTTP response collected from a loopback or external media fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP numeric status code.
    pub status: u16,
    /// Normalized `Content-Type` header if present.
    pub content_type: Option<String>,
    /// Decoded response body bytes.
    pub body: Vec<u8>,
}

/// Returns `true` when `raw_url` uses `http:`, `https:`, or `mailto:` cleanly without whitespace/control chars.
#[must_use]
pub fn is_valid_external_url(raw_url: &str) -> bool {
    let trimmed = raw_url.trim();
    if trimmed.is_empty()
        || trimmed
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return false;
    }
    let Some((scheme, rest)) = trimmed.split_once(':') else {
        return false;
    };
    let lower = scheme.to_ascii_lowercase();
    match lower.as_str() {
        "http" | "https" => {
            let Some(after_slashes) = rest.strip_prefix("//") else {
                return false;
            };
            let authority = after_slashes
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default();
            !authority.is_empty() && !authority.starts_with('.')
        }
        "mailto" => !rest.trim().is_empty(),
        _ => false,
    }
}

/// Parses an `http://` URL into host, port, `Host` header, and request target.
pub fn parse_http_url(raw_url: &str) -> Result<ParsedHttpUrl, String> {
    let trimmed = raw_url.trim();
    let Some((scheme, after_colon)) = trimmed.split_once("://") else {
        return Err(format!("missing URL scheme in `{raw_url}`"));
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return Err(format!(
            "unsupported non-http scheme `{scheme}` for TCP loopback"
        ));
    }
    let (authority, request_target) = match after_colon.find(['/', '?', '#']) {
        Some(idx) => {
            let auth = &after_colon[..idx];
            let suffix = &after_colon[idx..];
            let target = if suffix.starts_with('/') {
                suffix.to_owned()
            } else {
                format!("/{suffix}")
            };
            (auth, target)
        }
        None => (after_colon, "/".to_owned()),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(format!("invalid authority in URL `{raw_url}`"));
    }

    let (host, port) = parse_authority(authority)?;
    Ok(ParsedHttpUrl {
        host,
        port,
        host_header: authority.to_owned(),
        request_target,
    })
}

fn parse_authority(authority: &str) -> Result<(String, u16), String> {
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((ipv6, rest)) = after_open.split_once(']') else {
            return Err(format!("unclosed IPv6 bracket in `{authority}`"));
        };
        if ipv6.is_empty() {
            return Err("empty IPv6 host".to_owned());
        }
        let port = if let Some(raw_port) = rest.strip_prefix(':') {
            raw_port
                .parse::<u16>()
                .map_err(|_| format!("invalid port `{raw_port}`"))?
        } else if rest.is_empty() {
            80
        } else {
            return Err(format!("unexpected trailing host chars `{rest}`"));
        };
        return Ok((ipv6.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, raw_port)) if !raw_port.is_empty() => {
            if host.is_empty() {
                return Err("empty host".to_owned());
            }
            let port = raw_port
                .parse::<u16>()
                .map_err(|_| format!("invalid port `{raw_port}`"))?;
            Ok((host.to_owned(), port))
        }
        Some(_) => Err(format!("empty port in `{authority}`")),
        None => Ok((authority.to_owned(), 80)),
    }
}

/// Executes a bounded HTTP/1.1 request over TCP against an `http://` endpoint.
pub fn http_request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let parsed = parse_http_url(url)?;
    let addr = (parsed.host.as_str(), parsed.port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {}:{}: {e}", parsed.host, parsed.port))?
        .next()
        .ok_or_else(|| format!("no socket address for {}:{}", parsed.host, parsed.port))?;

    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let payload = body.unwrap_or(&[]);
    let mut head = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: */*\r\n",
        parsed.request_target, parsed.host_header
    );
    for (name, val) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(val);
        head.push_str("\r\n");
    }
    if body.is_some() {
        let _ = write!(head, "Content-Length: {}\r\n", payload.len());
    }
    head.push_str("\r\n");

    stream
        .write_all(head.as_bytes())
        .and_then(|()| {
            if payload.is_empty() {
                Ok(())
            } else {
                stream.write_all(payload)
            }
        })
        .and_then(|()| stream.flush())
        .map_err(|e| format!("write request failed: {e}"))?;

    let raw = read_bounded(&mut stream, MAX_RESPONSE_BYTES)?;
    parse_http_response(&raw)
}

/// Fetches HTTP/HTTPS media bytes safely without spawning a shell.
pub fn fetch_media_bytes(raw_url: &str, timeout: Duration) -> Result<HttpResponse, String> {
    if !is_valid_external_url(raw_url) {
        return Err("invalid media URL".to_owned());
    }
    let trimmed = raw_url.trim();
    if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        return http_request("GET", trimmed, &[], None, timeout);
    }
    if trimmed
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        let secs = timeout.as_secs().clamp(1, 30).to_string();
        let output = Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--location",
                "--max-time",
                &secs,
                "--include",
                trimmed,
            ])
            .output()
            .map_err(|e| format!("failed spawning curl: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "curl exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        return parse_possibly_redirected_http(&output.stdout);
    }
    Err("readMedia only supports http:// and https:// URLs".to_owned())
}

fn read_bounded<R: Read>(reader: &mut R, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len().saturating_add(n) > max_bytes {
                    return Err(format!("HTTP response exceeded {max_bytes} byte limit"));
                }
                out.extend_from_slice(&buf[..n]);
                if is_http_response_complete(&out) {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(format!("read response failed: {err}")),
        }
    }
    Ok(out)
}

fn is_http_response_complete(raw: &[u8]) -> bool {
    let Some(header_end) = find_header_end(raw) else {
        return false;
    };
    let headers = String::from_utf8_lossy(&raw[..header_end]);
    let mut content_length = None;
    let mut chunked = false;
    for line in headers.lines().skip(1) {
        if let Some((name, val)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = val.trim().parse::<usize>().ok();
            } else if name.trim().eq_ignore_ascii_case("transfer-encoding")
                && val.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    let body = &raw[header_end..];
    if let Some(len) = content_length {
        return body.len() >= len;
    }
    if chunked {
        return decode_chunked_body(body).is_ok()
            && (body.starts_with(b"0\r\n\r\n") || body.windows(5).any(|w| w == b"\r\n0\r\n"));
    }
    false
}

fn parse_possibly_redirected_http(raw: &[u8]) -> Result<HttpResponse, String> {
    let mut slice = raw;
    loop {
        let parsed = parse_http_response(slice)?;
        if (300..400).contains(&parsed.status)
            && (parsed.body.starts_with(b"HTTP/1.") || parsed.body.starts_with(b"HTTP/2"))
            && let Some(idx) = find_header_end(slice)
        {
            slice = &slice[idx..];
            continue;
        }
        return Ok(parsed);
    }
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let header_end = find_header_end(raw).ok_or_else(|| "incomplete HTTP headers".to_owned())?;
    if header_end > MAX_HEADER_BYTES {
        return Err("HTTP response headers exceeded 64 KiB".to_owned());
    }
    let header_text = String::from_utf8_lossy(&raw[..header_end]);
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| "missing HTTP status line".to_owned())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed HTTP status line `{status_line}`"))?;

    let mut content_type = None;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let key = name.trim().to_ascii_lowercase();
        let val = value.trim();
        if key == "content-type" {
            content_type = Some(val.to_owned());
        } else if key == "content-length" {
            content_length = val.parse::<usize>().ok();
        } else if key == "transfer-encoding" && val.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }

    let raw_body = &raw[header_end..];
    let body = if chunked {
        decode_chunked_body(raw_body)?
    } else if let Some(len) = content_length {
        raw_body.get(..len).unwrap_or(raw_body).to_vec()
    } else {
        raw_body.to_vec()
    };

    Ok(HttpResponse {
        status,
        content_type,
        body,
    })
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(pos + 4);
    }
    raw.windows(2).position(|w| w == b"\n\n").map(|pos| pos + 2)
}

fn decode_chunked_body(mut input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(pos) = input.windows(2).position(|w| w == b"\r\n") {
        let size_hex = std::str::from_utf8(&input[..pos])
            .map_err(|_| "non-UTF8 chunk size".to_owned())?
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("invalid chunk size `{size_hex}`"))?;
        input = &input[pos + 2..];
        if size == 0 {
            break;
        }
        if input.len() < size {
            return Err("truncated chunked HTTP body".to_owned());
        }
        out.extend_from_slice(&input[..size]);
        input = input.get(size + 2..).unwrap_or(&[]);
    }
    Ok(out)
}
