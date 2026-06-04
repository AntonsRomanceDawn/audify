//! HTML / raw-text extractor.
//!
//! For Phase 1 this is a pragmatic heuristic extractor: it prefers an
//! `<article>` or `<main>` container and walks headings and paragraphs in
//! document order to build a structured [`Document`]. A readability-grade
//! implementation (or GROBID for PDFs) can replace it behind the
//! [`Extractor`] trait without touching the rest of the pipeline.

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};

use audify_core::{Document, Error, Extractor, Result, Section, Source};

/// Fetches and extracts article text from a URL, or wraps raw text directly.
pub struct ArticleExtractor {
    client: reqwest::Client,
}

impl ArticleExtractor {
    /// Build an extractor over a preconfigured HTTP client (timeout, user-agent).
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn fetch(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?
            .error_for_status()
            .map_err(|e| Error::Extraction(format!("fetching {url}: {e}")))?;
        resp.text().await.map_err(|e| Error::Http(e.to_string()))
    }
}

#[async_trait]
impl Extractor for ArticleExtractor {
    async fn extract(&self, source: &Source) -> Result<Document> {
        match source {
            Source::Text(text) => {
                if text.trim().is_empty() {
                    return Err(Error::InvalidSource("empty text".into()));
                }
                Ok(text_document(text))
            }
            Source::Url(url) => {
                let body = self.fetch(url).await?;
                // No `await` past this point, so the non-Send `Html` value is
                // never held across a suspension and the future stays `Send`.
                parse_html(&body)
            }
            Source::File(_) => Err(Error::InvalidSource(
                "ArticleExtractor does not handle files".into(),
            )),
        }
    }
}

/// Wrap raw text as a single-section document, splitting on blank lines.
fn text_document(text: &str) -> Document {
    let paragraphs = text
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect();
    Document {
        title: None,
        sections: vec![Section {
            heading: None,
            paragraphs,
        }],
    }
}

/// Parse an HTML page into a structured [`Document`].
fn parse_html(body: &str) -> Result<Document> {
    let doc = Html::parse_document(body);

    let title = extract_title(&doc);

    // Prefer a content container; fall back to the whole document.
    let container = selector("article")
        .and_then(|sel| doc.select(&sel).next())
        .or_else(|| selector("main").and_then(|sel| doc.select(&sel).next()));

    let block_sel = selector("h1, h2, h3, p")
        .ok_or_else(|| Error::Extraction("invalid block selector".into()))?;

    let sections = match container {
        Some(root) => sections_from(root.select(&block_sel)),
        None => sections_from(doc.select(&block_sel)),
    };

    Ok(Document { title, sections })
}

/// Walk heading/paragraph elements in document order, starting a new section at
/// each heading and accumulating paragraphs in between.
fn sections_from<'a>(elements: impl Iterator<Item = ElementRef<'a>>) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current = Section::default();

    for el in elements {
        let text = collapse_ws(&el.text().collect::<String>());
        if text.is_empty() {
            continue;
        }
        match el.value().name() {
            "h1" | "h2" | "h3" => {
                if current.heading.is_some() || !current.paragraphs.is_empty() {
                    sections.push(std::mem::take(&mut current));
                }
                current.heading = Some(text);
            }
            _ => current.paragraphs.push(text),
        }
    }
    if current.heading.is_some() || !current.paragraphs.is_empty() {
        sections.push(current);
    }
    sections
}

/// First `<h1>`, else the `<title>` tag.
fn extract_title(doc: &Html) -> Option<String> {
    selector("h1")
        .and_then(|sel| doc.select(&sel).next())
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            selector("title")
                .and_then(|sel| doc.select(&sel).next())
                .map(|el| collapse_ws(&el.text().collect::<String>()))
                .filter(|t| !t.is_empty())
        })
}

/// Parse a CSS selector, returning `None` on an invalid pattern.
fn selector(pattern: &str) -> Option<Selector> {
    Selector::parse(pattern).ok()
}

/// Collapse all runs of whitespace to single spaces and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
