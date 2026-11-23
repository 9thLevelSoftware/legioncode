//! Strict `SemVer` 2.0 parsing and ordering matching `serviceProtocol.ts:164-244`.

use std::cmp::Ordering;

/// Returns `true` when `version` matches strict `SemVer` 2.0 `MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]`
/// (`serviceProtocol.ts:164-171`).
pub fn is_exact_service_version(version: &str) -> bool {
    let without_build = match version.split_once('+') {
        Some((prefix, build)) => {
            if build.is_empty()
                || build.split('.').any(|part| {
                    part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return false;
            }
            prefix
        }
        None => version,
    };

    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };

    let mut core_parts = core.split('.');
    let Some(major) = core_parts.next() else {
        return false;
    };
    let Some(minor) = core_parts.next() else {
        return false;
    };
    let Some(patch) = core_parts.next() else {
        return false;
    };
    if core_parts.next().is_some()
        || !is_semver_number(major)
        || !is_semver_number(minor)
        || !is_semver_number(patch)
    {
        return false;
    }

    if let Some(pre) = prerelease {
        if pre.is_empty() {
            return false;
        }
        for ident in pre.split('.') {
            if ident.is_empty()
                || !ident
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return false;
            }
            if ident.bytes().all(|b| b.is_ascii_digit()) && !is_semver_number(ident) {
                return false;
            }
        }
    }
    true
}

/// Compares two strict `SemVer` 2.0 versions per `serviceProtocol.ts:209-244`.
pub fn compare_exact_service_versions(left: &str, right: &str) -> Ordering {
    let (a_core, a_pre) = parse_semver(left);
    let (b_core, b_pre) = parse_semver(right);
    for idx in 0..3 {
        match a_core[idx].cmp(&b_core[idx]) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }
    }
    if a_pre.is_empty() || b_pre.is_empty() {
        return match (a_pre.is_empty(), b_pre.is_empty()) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (true, true) | (false, false) => Ordering::Equal,
        };
    }

    for idx in 0..a_pre.len().max(b_pre.len()) {
        match (a_pre.get(idx).copied(), b_pre.get(idx).copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) => {
                let x_num = x.bytes().all(|b| b.is_ascii_digit());
                let y_num = y.bytes().all(|b| b.is_ascii_digit());
                return match (x_num, y_num) {
                    (true, true) => x
                        .parse::<u128>()
                        .unwrap_or(0)
                        .cmp(&y.parse::<u128>().unwrap_or(0)),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => x.cmp(y),
                };
            }
        }
    }
    Ordering::Equal
}

fn parse_semver(version: &str) -> ([u128; 3], Vec<&str>) {
    let without_build = version.split('+').next().unwrap_or(version);
    let (core, pre) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let parse_part = |p: Option<&str>| p.and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
    let core_nums = [
        parse_part(parts.next()),
        parse_part(parts.next()),
        parse_part(parts.next()),
    ];
    let prerelease: Vec<&str> = pre.map_or_else(Vec::new, |s| s.split('.').collect());
    (core_nums, prerelease)
}

fn is_semver_number(segment: &str) -> bool {
    if segment == "0" {
        return true;
    }
    let bytes = segment.as_bytes();
    !bytes.is_empty()
        && bytes[0] >= b'1'
        && bytes[0] <= b'9'
        && bytes.iter().all(u8::is_ascii_digit)
}
