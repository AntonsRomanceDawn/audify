//! Lightweight PDF extractor (pure Rust, no external service).
//!
//! PDFs store positioned glyphs, not structure, so this is heuristic: it pulls
//! the text, drops page numbers, repeated headers/footers, and everything from a
//! "References" line onward, repairs hyphenation, and rebuilds paragraphs.
//!
//! It handles single-column documents well. Two-column layouts and heavy math
//! may extract poorly — use `--preview` to check before synthesizing. A
//! structured extractor (e.g. GROBID) can replace this behind the trait later.

use std::collections::HashMap;

use async_trait::async_trait;

use audify_core::{Document, Error, Extractor, Result, Section, Source};

/// Extracts text from local PDF files.
#[derive(Default)]
pub struct PdfExtractor;

impl PdfExtractor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Extractor for PdfExtractor {
    async fn extract(&self, source: &Source) -> Result<Document> {
        let path = match source {
            Source::File(p) => p.clone(),
            _ => return Err(Error::InvalidSource("PdfExtractor only handles files".into())),
        };

        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
            .filter(|t| !t.trim().is_empty());

        // pdf-extract is blocking/CPU-bound; keep it off the async runtime.
        // Map the error to a String inside the closure so it crosses threads.
        let raw = tokio::task::spawn_blocking(move || {
            pdf_extract::extract_text(&path).map_err(|e| format!("{e:?}"))
        })
        .await
        .map_err(|e| Error::Extraction(format!("pdf task panicked: {e}")))?
        .map_err(Error::Extraction)?;

        let document = clean_pdf_text(&raw, title);
        if document.is_empty() {
            return Err(Error::Extraction(
                "no usable text extracted from PDF (scanned or image-only?)".into(),
            ));
        }
        Ok(document)
    }
}

/// A line that is just a page number, e.g. "12" or "Page 3".
fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.len() > 12 {
        return false;
    }
    let digits_only = t.chars().all(|c| c.is_ascii_digit());
    let lower = t.to_ascii_lowercase();
    // "Page 3" (en), "Seite 3" (de), "Pág 3", etc.
    let page_label = ["page ", "seite ", "pág ", "pagina ", "p. "].iter().any(|p| {
        lower
            .strip_prefix(p)
            .map(|rest| {
                let r = rest.trim();
                !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())
            })
            .unwrap_or(false)
    });
    (digits_only && t.len() <= 4) || page_label
}

/// A line marking the start of the references section (dropped onward).
fn is_reference_header(line: &str) -> bool {
    let t = line.trim().to_ascii_lowercase();
    t.len() < 24 && matches!(t.as_str(), "references" | "bibliography" | "works cited")
}

/// Turn raw PDF text into a cleaned single-section document.
fn clean_pdf_text(raw: &str, title: Option<String>) -> Document {
    let lines: Vec<&str> = raw.lines().collect();

    // Short lines that repeat across the document are likely running
    // headers/footers (proxy: we lack page boundaries from a flat extract).
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for line in &lines {
        let t = line.trim();
        if !t.is_empty() && t.len() <= 60 {
            *freq.entry(t).or_default() += 1;
        }
    }

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();

    for line in &lines {
        let t = line.trim();

        if t.is_empty() {
            if !current.trim().is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        }
        if is_reference_header(t) {
            break; // drop references and everything after
        }
        if is_page_number(t) {
            continue;
        }
        if freq.get(t).copied().unwrap_or(0) >= 3 {
            continue; // repeated header/footer
        }

        // Repair words hyphenated across line breaks; otherwise space-join.
        if current.ends_with('-') {
            current.pop();
            current.push_str(t);
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(t);
        }
    }
    if !current.trim().is_empty() {
        paragraphs.push(current);
    }

    let paragraphs = paragraphs
        .into_iter()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect();

    Document {
        title,
        sections: vec![Section {
            heading: None,
            paragraphs,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_page_numbers_and_references() {
        let raw = "Introduction text here.\n\n12\n\nMore body text.\n\nReferences\n\n[1] Foo et al.";
        let doc = clean_pdf_text(raw, Some("paper".into()));
        let joined = doc.sections[0].paragraphs.join(" ");
        assert!(joined.contains("Introduction text here."));
        assert!(joined.contains("More body text."));
        assert!(!joined.contains("12"));
        assert!(!joined.contains("Foo et al"));
    }

    #[test]
    fn repairs_hyphenation_across_lines() {
        let raw = "this is an exam-\nple of wrapping";
        let doc = clean_pdf_text(raw, None);
        assert_eq!(doc.sections[0].paragraphs[0], "this is an example of wrapping");
    }

    #[test]
    fn detects_page_number_lines() {
        assert!(is_page_number("12"));
        assert!(is_page_number("Page 3"));
        assert!(!is_page_number("2024 was a good year"));
    }
}
