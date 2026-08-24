//! Small string and version helpers shared across modules.

use std::cmp::Ordering;

/// Lowercase and strip everything outside [a-z0-9] (mirrors Python's re.sub).
pub(crate) fn norm_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

pub(crate) fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        s.chars().take(cap).collect()
    }
}

/// Numeric version comparison on dot-separated integer components.
pub(crate) fn parse_version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .filter_map(|p| p.parse::<u64>().ok())
        .collect()
}

pub(crate) fn lt_versions(a: &[u64], b: &[u64]) -> bool {
    cmp_versions_slices(a, b) == Ordering::Less
}

pub(crate) fn le_versions(a: &[u64], b: &[u64]) -> bool {
    cmp_versions_slices(a, b) != Ordering::Greater
}

fn cmp_versions_slices(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

pub(crate) fn cmp_versions(a: &str, b: &str) -> Ordering {
    cmp_versions_slices(&parse_version_key(a), &parse_version_key(b))
}

pub(crate) fn last_n(items: &[String], n: usize) -> Vec<String> {
    let start = items.len().saturating_sub(n);
    items[start..].to_vec()
}
