//! Audio assembly via ffmpeg.
//!
//! Per the plan: synthesize chunks to lossless WAV, concatenate, and encode to
//! MP3 **once** with loudness normalization — never concatenate already-encoded
//! MP3s (that causes gaps/clicks and compounds loss). Requires `ffmpeg` on PATH.

use tokio::process::Command;

use audify_core::{Audio, Error, Result};

/// Concatenate WAV chunks and encode a single loudness-normalized MP3.
pub async fn to_mp3(chunks: &[Audio]) -> Result<Vec<u8>> {
    if chunks.is_empty() {
        return Err(Error::Synthesis("no audio chunks to assemble".into()));
    }

    let dir = tempfile::tempdir().map_err(|e| Error::Storage(format!("temp dir: {e}")))?;

    // Write each chunk and build an ffmpeg concat-demuxer list file.
    let mut list = String::new();
    for (i, audio) in chunks.iter().enumerate() {
        let path = dir.path().join(format!("chunk_{i}.{}", audio.format.extension()));
        tokio::fs::write(&path, &audio.bytes)
            .await
            .map_err(|e| Error::Storage(format!("writing chunk {i}: {e}")))?;
        // ffmpeg accepts forward slashes on Windows; escape single quotes.
        let p = path.to_string_lossy().replace('\\', "/").replace('\'', "'\\''");
        list.push_str(&format!("file '{p}'\n"));
    }

    let list_path = dir.path().join("list.txt");
    tokio::fs::write(&list_path, list)
        .await
        .map_err(|e| Error::Storage(format!("writing concat list: {e}")))?;
    let out_path = dir.path().join("out.mp3");

    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        .args(["-af", "loudnorm", "-c:a", "libmp3lame", "-q:a", "4"])
        .arg(&out_path)
        .output()
        .await
        .map_err(|e| Error::Synthesis(format!("running ffmpeg (is it on PATH?): {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Synthesis(format!("ffmpeg failed: {stderr}")));
    }

    tokio::fs::read(&out_path)
        .await
        .map_err(|e| Error::Storage(format!("reading assembled mp3: {e}")))
}
