use std::collections::{BTreeSet, VecDeque};

use reqwest::Client;
use url::Url;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct SpiderConfig {
    pub root: Url,
    pub max_depth: usize,
    pub allowed_extensions: BTreeSet<String>,
    pub same_host_only: bool,
    pub respect_robots: bool,
    pub allowed_schemes: BTreeSet<String>,
}

impl SpiderConfig {
    #[must_use]
    pub fn normalized(mut self) -> Self {
        if self.allowed_schemes.is_empty() {
            self.allowed_schemes.insert("http".to_owned());
            self.allowed_schemes.insert("https".to_owned());
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct SpiderHit {
    pub url: Url,
    pub depth: usize,
}

pub async fn crawl(client: &Client, config: &SpiderConfig) -> Result<Vec<SpiderHit>> {
    let config = config.clone().normalized();
    let robots = if config.respect_robots {
        fetch_robots_disallow(client, &config.root)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut queue = VecDeque::from([(config.root.clone(), 0_usize)]);
    let mut visited = BTreeSet::new();
    let mut hits = Vec::new();

    while let Some((url, depth)) = queue.pop_front() {
        if depth > config.max_depth || !visited.insert(url.as_str().to_owned()) {
            continue;
        }

        if !is_allowed_by_robots(&url, &robots) {
            continue;
        }

        let response = client.get(url.clone()).send().await?;
        if !response.status().is_success() {
            continue;
        }

        let body = response.text().await?;
        for link in extract_links(&url, &body) {
            if !config.allowed_schemes.contains(link.scheme()) {
                continue;
            }

            if config.same_host_only && link.host_str() != config.root.host_str() {
                continue;
            }

            if !is_allowed_by_robots(&link, &robots) {
                continue;
            }

            if let Some(ext) = link
                .path_segments()
                .and_then(|mut s| s.next_back())
                .and_then(|v| v.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()))
                && !config.allowed_extensions.is_empty()
                && !config.allowed_extensions.contains(&ext)
            {
                continue;
            }

            hits.push(SpiderHit {
                url: link.clone(),
                depth: depth + 1,
            });

            if depth < config.max_depth {
                queue.push_back((link, depth + 1));
            }
        }
    }

    Ok(hits)
}

#[must_use]
pub fn collect_urls(hits: &[SpiderHit]) -> Vec<Url> {
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        if !out.iter().any(|existing: &Url| existing == &hit.url) {
            out.push(hit.url.clone());
        }
    }
    out
}

async fn fetch_robots_disallow(client: &Client, root: &Url) -> Result<Vec<String>> {
    let robots_url = root.join("/robots.txt")?;
    let response = client.get(robots_url).send().await?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }

    let content = response.text().await?;
    Ok(parse_robots_disallow(&content))
}

#[must_use]
fn parse_robots_disallow(content: &str) -> Vec<String> {
    let mut disallow = Vec::new();
    let mut current_matches = false;

    for raw_line in content.lines() {
        let line = raw_line
            .split('#')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_ascii_lowercase();
        if let Some((_, value)) = lower.split_once(':')
            && lower.starts_with("user-agent:")
        {
            current_matches = value.trim() == "*";
            continue;
        }

        if !current_matches {
            continue;
        }

        if line.len() >= 9 && line[..9].eq_ignore_ascii_case("disallow:") {
            let path = line[9..].trim();
            if !path.is_empty() {
                disallow.push(path.to_owned());
            }
        }
    }

    disallow
}

#[must_use]
fn is_allowed_by_robots(url: &Url, disallow: &[String]) -> bool {
    let path = url.path();
    !disallow.iter().any(|rule| path.starts_with(rule))
}

#[must_use]
fn extract_links(base: &Url, html: &str) -> Vec<Url> {
    let mut links = Vec::new();
    let needle = "href=";
    let mut start = 0_usize;

    while let Some(offset) = html[start..].find(needle) {
        let index = start + offset + needle.len();
        if index >= html.len() {
            break;
        }

        let quote = html.as_bytes()[index];
        if quote != b'\'' && quote != b'"' {
            start = index + 1;
            continue;
        }

        let value_start = index + 1;
        if let Some(end_rel) = html[value_start..].find(quote as char) {
            let value_end = value_start + end_rel;
            let raw = html[value_start..value_end].trim();
            if let Ok(link) = base.join(raw) {
                links.push(link);
            }
            start = value_end + 1;
        } else {
            break;
        }
    }

    links
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn extracts_relative_and_absolute_links() {
        let base = Url::parse("https://example.com/root/").expect("valid url");
        let html = r#"
            <a href="/a/file.zip">file</a>
            <a href="https://cdn.example.com/asset.mp4">video</a>
        "#;

        let links = extract_links(&base, html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].as_str(), "https://example.com/a/file.zip");
        assert_eq!(links[1].as_str(), "https://cdn.example.com/asset.mp4");
    }

    #[test]
    fn parses_robots_disallow() {
        let txt = "User-agent: *\nDisallow: /private\nDisallow: /tmp\n";
        let rules = parse_robots_disallow(txt);
        assert_eq!(rules, vec!["/private", "/tmp"]);
    }
}
