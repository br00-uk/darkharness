//! The `sitemap` adapter: a sitemap and the HTML pages it lists.
//!
//! This is the adapter that most needs the seam documented in
//! `crate::ingest::fetch`: it fetches a sitemap, then fetches every page
//! the sitemap lists, all through a caller-supplied [`Fetcher`] so
//! `dark-lexicon` never builds a socket itself. It obeys `robots.txt`
//! (G2's Do 3) by checking every page URL against the host's policy before
//! fetching it, and paces requests with [`RateLimiter`] at G2's rate: 2
//! requests each second for one host.

use crate::ingest::document::Document;
use crate::ingest::fetch::{Fetcher, RateLimiter, fetch_capped, host_of};
use crate::ingest::html;
use crate::ingest::robots::RobotsPolicy;
use dark_contract::{ErrCode, Error, Result};

/// Extracts every `<loc>` URL from sitemap XML, in document order.
///
/// This is a small, tolerant scan rather than a full XML parser, for the
/// same dependency reason `crate::ingest::html` hand-scans HTML: Rule 16
/// leaves no room for an XML crate. A sitemap's `<loc>` elements hold plain
/// URLs with no nested markup, so a substring scan is enough.
fn extract_locations(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<loc>") {
        let after_tag = &rest[start + "<loc>".len()..];
        let Some(end) = after_tag.find("</loc>") else {
            break;
        };
        let url = after_tag[..end].trim();
        if !url.is_empty() {
            out.push(decode_xml_entities(url));
        }
        rest = &after_tag[end + "</loc>".len()..];
    }
    out
}

/// Decodes the five predefined XML entities. A sitemap URL can carry
/// `&amp;` for a literal `&` in a query string; nothing else needs
/// decoding here.
fn decode_xml_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Fetches the sitemap at `sitemap_url` and every page it lists, returning
/// one [`Document`] per page.
///
/// Fetch order matches the sitemap's own `<loc>` order, which keeps a run
/// deterministic given a deterministic sitemap and a deterministic
/// `fetcher`. Requests to each page's host obey that host's `robots.txt`
/// and are paced through `limiter` at G2's rate.
///
/// A page that `robots.txt` disallows, or that fails to fetch, is skipped
/// rather than aborting the whole run: one broken page in a large sitemap
/// should not lose every other page.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when the sitemap itself cannot be fetched.
pub fn ingest(
    fetcher: &dyn Fetcher,
    limiter: &mut RateLimiter,
    sitemap_url: &str,
) -> Result<Vec<Document>> {
    let sitemap_host = host_of(sitemap_url)?;
    limiter.wait(&sitemap_host);
    let sitemap_bytes = fetch_capped(fetcher, sitemap_url)?;
    let sitemap_xml = String::from_utf8(sitemap_bytes).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("{sitemap_url} is not valid UTF-8: {source}"),
        )
    })?;

    let mut documents = Vec::new();
    let mut robots_by_host: std::collections::HashMap<String, RobotsPolicy> =
        std::collections::HashMap::new();

    for page_url in extract_locations(&sitemap_xml) {
        let Ok(host) = host_of(&page_url) else {
            continue;
        };
        let policy = robots_by_host
            .entry(host.clone())
            .or_insert_with(|| RobotsPolicy::fetch(fetcher, &page_url).unwrap_or_default());
        let path = page_url
            .split_once("://")
            .map_or(page_url.as_str(), |(_, rest)| rest);
        let path = path.split_once('/').map_or("/", |(_, rest)| rest);
        let path = format!("/{path}");
        if !policy.is_allowed(&path) {
            continue;
        }

        limiter.wait(&host);
        let Ok(bytes) = fetch_capped(fetcher, &page_url) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let extracted = html::extract(&text);
        let title = extracted.title.clone().unwrap_or_else(|| page_url.clone());
        let doc_path = page_url
            .split_once("://")
            .map_or(page_url.as_str(), |(_, rest)| rest)
            .replace(['/', '?'], "_");
        let document = Document::new(doc_path, title, extracted.body)
            .with_headings(extracted.headings)
            .with_url(page_url);
        documents.push(document);
    }

    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MapFetcher(Mutex<HashMap<String, Vec<u8>>>);

    impl Fetcher for MapFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.0
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| Error::new(ErrCode::ToolFailed, format!("no fixture for {url}")))
        }
    }

    fn fetcher_with(pages: &[(&str, &str)]) -> MapFetcher {
        let mut map = HashMap::new();
        for (url, body) in pages {
            map.insert((*url).to_owned(), body.as_bytes().to_vec());
        }
        MapFetcher(Mutex::new(map))
    }

    #[test]
    fn extracts_loc_urls_in_order() {
        let xml = "<urlset><url><loc>https://example.com/a</loc></url>\
                    <url><loc>https://example.com/b?x=1&amp;y=2</loc></url></urlset>";
        assert_eq!(
            extract_locations(xml),
            vec!["https://example.com/a", "https://example.com/b?x=1&y=2"]
        );
    }

    #[test]
    fn ingests_every_page_the_sitemap_lists() {
        let fetcher = fetcher_with(&[
            (
                "https://example.com/sitemap.xml",
                "<urlset><url><loc>https://example.com/a</loc></url>\
                  <url><loc>https://example.com/b</loc></url></urlset>",
            ),
            ("https://example.com/robots.txt", ""),
            (
                "https://example.com/a",
                "<title>A</title><h1>Page A</h1><p>text a</p>",
            ),
            (
                "https://example.com/b",
                "<title>B</title><h1>Page B</h1><p>text b</p>",
            ),
        ]);
        let mut limiter = RateLimiter::new(1000);
        let docs = ingest(&fetcher, &mut limiter, "https://example.com/sitemap.xml").unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].title, "A");
        assert_eq!(docs[1].title, "B");
        assert!(docs[0].body.contains("text a"));
    }

    #[test]
    fn skips_a_page_that_robots_txt_disallows() {
        let fetcher = fetcher_with(&[
            (
                "https://example.com/sitemap.xml",
                "<urlset><url><loc>https://example.com/private/a</loc></url>\
                  <url><loc>https://example.com/public/b</loc></url></urlset>",
            ),
            (
                "https://example.com/robots.txt",
                "User-agent: *\nDisallow: /private\n",
            ),
            ("https://example.com/public/b", "<title>B</title>"),
        ]);
        let mut limiter = RateLimiter::new(1000);
        let docs = ingest(&fetcher, &mut limiter, "https://example.com/sitemap.xml").unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "B");
    }

    #[test]
    fn ingest_fails_when_the_sitemap_itself_cannot_be_fetched() {
        let fetcher = fetcher_with(&[]);
        let mut limiter = RateLimiter::new(1000);
        let err = ingest(&fetcher, &mut limiter, "https://example.com/sitemap.xml").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
