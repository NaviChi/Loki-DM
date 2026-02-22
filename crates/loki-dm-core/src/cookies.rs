use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{LokiDmError, Result};

#[must_use]
pub fn parse_cookie_pair(raw: &str) -> Option<(String, String)> {
    let (name, value) = raw.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    let value = value.trim().trim_end_matches(';').trim();
    Some((name.to_owned(), value.to_owned()))
}

pub fn load_cookie_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let raw = fs::read_to_string(path).map_err(|err| {
        LokiDmError::Message(format!(
            "failed to read cookie file {}: {err}",
            path.display()
        ))
    })?;

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let mut cookies = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((name, value, expires)) = parse_netscape_cookie_line(line) {
            if expires > 0 && expires <= now_unix {
                continue;
            }
            cookies.insert(name, value);
            continue;
        }

        if line.starts_with('#') {
            continue;
        }

        if let Some((name, value)) = parse_cookie_pair(line) {
            cookies.insert(name, value);
        }
    }

    Ok(cookies)
}

pub fn merge_cookie_sources(
    inline_cookies: &[String],
    cookie_file: Option<&Path>,
) -> Result<BTreeMap<String, String>> {
    let mut merged = BTreeMap::new();
    if let Some(path) = cookie_file {
        merged.extend(load_cookie_file(path)?);
    }

    for raw in inline_cookies {
        let Some((name, value)) = parse_cookie_pair(raw) else {
            return Err(LokiDmError::Message(format!(
                "invalid cookie format `{raw}`: expected `name=value`"
            )));
        };
        merged.insert(name, value);
    }

    Ok(merged)
}

#[must_use]
pub fn render_cookie_header(cookies: &BTreeMap<String, String>) -> Option<String> {
    if cookies.is_empty() {
        return None;
    }

    Some(
        cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn parse_netscape_cookie_line(raw: &str) -> Option<(String, String, u64)> {
    let line = raw.strip_prefix("#HttpOnly_").unwrap_or(raw);
    if line.starts_with('#') {
        return None;
    }

    let mut parts = line.split('\t');
    let _domain = parts.next()?;
    let _include_subdomains = parts.next()?;
    let _path = parts.next()?;
    let _secure = parts.next()?;
    let expires = parts
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(0);
    let name = parts.next()?.trim();
    let value = parts.next()?.trim();
    if name.is_empty() {
        return None;
    }

    Some((name.to_owned(), value.to_owned(), expires))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{load_cookie_file, merge_cookie_sources, parse_cookie_pair, render_cookie_header};

    #[test]
    fn parses_cookie_pairs() {
        assert_eq!(
            parse_cookie_pair("session=abc123"),
            Some(("session".to_owned(), "abc123".to_owned()))
        );
        assert_eq!(
            parse_cookie_pair("  auth = token ; "),
            Some(("auth".to_owned(), "token".to_owned()))
        );
        assert_eq!(parse_cookie_pair("invalid"), None);
    }

    #[test]
    fn reads_netscape_cookie_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("cookies.txt");
        fs::write(
            &path,
            concat!(
                "# Netscape HTTP Cookie File\n",
                ".example.com\tTRUE\t/\tFALSE\t4102444800\tsid\tabc\n",
                "#HttpOnly_.example.com\tTRUE\t/\tFALSE\t4102444800\tauth\txyz\n",
            ),
        )
        .expect("write");

        let cookies = load_cookie_file(&path).expect("load cookie file");
        assert_eq!(cookies.get("sid").map(String::as_str), Some("abc"));
        assert_eq!(cookies.get("auth").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn merges_inline_over_file_and_renders_header() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("cookies.txt");
        fs::write(
            &path,
            ".example.com\tTRUE\t/\tFALSE\t4102444800\tsid\tabc\n",
        )
        .expect("write");

        let merged = merge_cookie_sources(
            &["sid=override".to_owned(), "mode=fast".to_owned()],
            Some(&path),
        )
        .expect("merge");
        assert_eq!(merged.get("sid").map(String::as_str), Some("override"));
        assert_eq!(merged.get("mode").map(String::as_str), Some("fast"));

        let rendered = render_cookie_header(&merged).expect("cookie header");
        assert!(rendered.contains("sid=override"));
        assert!(rendered.contains("mode=fast"));
    }
}
