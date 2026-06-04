//! Splits speakable text into chunks for parallel synthesis.
//!
//! Chunking is not required by the Voxtral API (it handles long input), but it
//! gives us parallelism, per-chunk retry, and per-chunk caching. We pack whole
//! paragraphs up to a character budget, splitting on sentence boundaries only
//! when a single paragraph exceeds the budget. Never splits mid-word.

/// Default target chunk size in characters.
pub const DEFAULT_TARGET_CHARS: usize = 2500;

/// Split `text` (paragraphs separated by blank lines) into chunks near `target`
/// characters each.
pub fn chunk_text(text: &str, target: usize) -> Vec<String> {
    let target = target.max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if para.chars().count() > target {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(split_oversized(para, target));
            continue;
        }

        let joined_len =
            current.chars().count() + para.chars().count() + if current.is_empty() { 0 } else { 2 };
        if joined_len > target && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }

    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Pack the sentences of an over-long paragraph into pieces near `target`.
fn split_oversized(text: &str, target: usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();

    for sentence in split_sentences(text) {
        let joined_len = current.chars().count()
            + sentence.chars().count()
            + if current.is_empty() { 0 } else { 1 };
        if joined_len > target && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence);
    }

    if !current.trim().is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Split text into sentences, keeping terminal punctuation. A terminator counts
/// only when followed by whitespace or end-of-text (so "e.g." stays intact).
fn split_sentences(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut sentences = Vec::new();
    let mut start = 0;

    for i in 0..chars.len() {
        let (idx, ch) = chars[i];
        if matches!(ch, '.' | '!' | '?') {
            let next_is_boundary = chars.get(i + 1).is_none_or(|(_, c)| c.is_whitespace());
            if next_is_boundary {
                let end = idx + ch.len_utf8();
                let piece = text[start..end].trim();
                if !piece.is_empty() {
                    sentences.push(piece);
                }
                start = end;
            }
        }
    }
    if start < text.len() {
        let piece = text[start..].trim();
        if !piece.is_empty() {
            sentences.push(piece);
        }
    }
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_text("Hello world.", 2500);
        assert_eq!(chunks, vec!["Hello world.".to_string()]);
    }

    #[test]
    fn packs_paragraphs_up_to_target() {
        // Three ~20-char paragraphs, target 50 → packs 2 then 1.
        let text = "aaaaaaaaaaaaaaaaaa.\n\nbbbbbbbbbbbbbbbbbb.\n\ncccccccccccccccccc.";
        let chunks = chunk_text(text, 50);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("aaaa") && chunks[0].contains("bbbb"));
        assert!(chunks[1].contains("cccc"));
    }

    #[test]
    fn splits_oversized_paragraph_on_sentences() {
        let text = "First sentence here. Second sentence here. Third sentence here.";
        let chunks = chunk_text(text, 25);
        assert!(chunks.len() >= 2);
        // No chunk wildly exceeds the target (sentences are ~20 chars).
        assert!(chunks.iter().all(|c| c.chars().count() <= 45));
    }

    #[test]
    fn empty_text_is_no_chunks() {
        assert!(chunk_text("   \n\n  ", 100).is_empty());
    }
}
