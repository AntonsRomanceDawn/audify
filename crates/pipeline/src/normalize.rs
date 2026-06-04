//! Rule-based normalizer: turns a structured [`Document`] into speakable text.
//!
//! Per the locked plan it strips references/bibliography sections, figure and
//! table captions, and inline citation markers, repairs PDF-style hyphenation,
//! and collapses whitespace. An optional LLM "speakify" stage can be layered on
//! later behind its own trait; this rule pass is the always-on baseline.

use regex::Regex;

use audify_core::{Document, Error, Normalizer, Result};

/// Deterministic, dependency-free text normalizer.
pub struct RuleNormalizer {
    citation: Regex,
    caption: Regex,
    hyphenation: Regex,
}

impl RuleNormalizer {
    /// Compile the cleaning patterns.
    pub fn new() -> Result<Self> {
        let mk = |p: &str| Regex::new(p).map_err(|e| Error::Config(format!("bad regex {p:?}: {e}")));
        Ok(Self {
            // [12], [3-5], [2, 7] style inline citation markers.
            citation: mk(r"\[\s*\d+(?:\s*[-–,]\s*\d+)*\s*\]")?,
            // Lines that begin a figure/table caption.
            caption: mk(r"(?i)^\s*(figure|fig\.?|table)\s+\d+")?,
            // A word hyphenated across a line break: "exam-\nple" -> "example".
            hyphenation: mk(r"(\w)-\s*\n\s*(\w)")?,
        })
    }

    /// Headings that mark a section we drop entirely.
    fn is_dropped_heading(heading: &str) -> bool {
        let h = heading.trim().to_ascii_lowercase();
        matches!(
            h.as_str(),
            "references" | "bibliography" | "works cited" | "citations" | "notes" | "footnotes"
        )
    }

    /// Clean a single text fragment.
    fn clean(&self, text: &str) -> String {
        let dehyphenated = self.hyphenation.replace_all(text, "$1$2");
        let decited = self.citation.replace_all(&dehyphenated, "");
        decited.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

impl Normalizer for RuleNormalizer {
    fn normalize(&self, document: &Document) -> Result<String> {
        let mut out = String::new();

        if let Some(title) = &document.title {
            let title = self.clean(title);
            if !title.is_empty() {
                out.push_str(&title);
                out.push_str(".\n\n");
            }
        }

        for section in &document.sections {
            if let Some(heading) = &section.heading {
                if Self::is_dropped_heading(heading) {
                    continue;
                }
                let heading = self.clean(heading);
                if !heading.is_empty() {
                    out.push_str(&heading);
                    out.push_str(".\n\n");
                }
            }

            for paragraph in &section.paragraphs {
                // Drop figure/table captions wholesale.
                if self.caption.is_match(paragraph) {
                    continue;
                }
                let cleaned = self.clean(paragraph);
                if cleaned.is_empty() {
                    continue;
                }
                out.push_str(&cleaned);
                out.push_str("\n\n");
            }
        }

        let trimmed = out.trim();
        if trimmed.is_empty() {
            return Err(Error::Extraction(
                "no speakable text remained after normalization".into(),
            ));
        }
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audify_core::{Document, Section};

    fn norm() -> RuleNormalizer {
        RuleNormalizer::new().expect("regexes compile")
    }

    fn doc(sections: Vec<Section>) -> Document {
        Document {
            title: None,
            sections,
        }
    }

    #[test]
    fn strips_citation_markers() {
        let out = norm()
            .normalize(&doc(vec![Section {
                heading: None,
                paragraphs: vec!["As shown earlier [12], this holds [3-5].".into()],
            }]))
            .unwrap();
        assert_eq!(out, "As shown earlier , this holds .");
    }

    #[test]
    fn drops_figure_captions() {
        let out = norm()
            .normalize(&doc(vec![Section {
                heading: None,
                paragraphs: vec![
                    "Figure 2: the architecture overview.".into(),
                    "Real body text.".into(),
                ],
            }]))
            .unwrap();
        assert_eq!(out, "Real body text.");
    }

    #[test]
    fn drops_reference_sections() {
        let out = norm()
            .normalize(&doc(vec![
                Section {
                    heading: Some("Introduction".into()),
                    paragraphs: vec!["Hello.".into()],
                },
                Section {
                    heading: Some("References".into()),
                    paragraphs: vec!["[1] Some Author, 2020.".into()],
                },
            ]))
            .unwrap();
        assert_eq!(out, "Introduction.\n\nHello.");
    }

    #[test]
    fn repairs_hyphenation() {
        let out = norm()
            .normalize(&doc(vec![Section {
                heading: None,
                paragraphs: vec!["This is one exam-\nple of wrapping.".into()],
            }]))
            .unwrap();
        assert_eq!(out, "This is one example of wrapping.");
    }

    #[test]
    fn empty_document_errors() {
        let err = norm().normalize(&doc(vec![Section {
            heading: None,
            paragraphs: vec!["   ".into()],
        }]));
        assert!(err.is_err());
    }
}
