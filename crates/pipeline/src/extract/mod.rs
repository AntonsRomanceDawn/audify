//! Extractors turn a [`audify_core::Source`] into a structured document.

mod html;
mod pdf;

pub use html::ArticleExtractor;
pub use pdf::PdfExtractor;

use async_trait::async_trait;

use audify_core::{Document, Extractor, Result, Source};

/// Dispatches to the right extractor by source kind: URLs/text to the article
/// extractor, files to the PDF extractor. Keeps the pipeline source-agnostic.
pub struct RoutingExtractor {
    article: ArticleExtractor,
    pdf: PdfExtractor,
}

impl RoutingExtractor {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            article: ArticleExtractor::new(client),
            pdf: PdfExtractor::new(),
        }
    }
}

#[async_trait]
impl Extractor for RoutingExtractor {
    async fn extract(&self, source: &Source) -> Result<Document> {
        match source {
            Source::Url(_) | Source::Text(_) => self.article.extract(source).await,
            Source::File(_) => self.pdf.extract(source).await,
        }
    }
}
