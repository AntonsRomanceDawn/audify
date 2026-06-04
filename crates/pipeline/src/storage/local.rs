//! Local-disk storage. The R2/object-store backend (Phase 5) implements the
//! same [`Storage`] trait and swaps in via config — nothing else changes.

use std::path::PathBuf;

use async_trait::async_trait;

use audify_core::{Error, Result, Storage};

/// Writes objects under a base directory on the local filesystem.
pub struct LocalStorage {
    base_dir: PathBuf,
}

impl LocalStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }
}

#[async_trait]
impl Storage for LocalStorage {
    async fn store(&self, key: &str, bytes: &[u8]) -> Result<String> {
        let path = self.base_dir.join(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Storage(format!("creating {}: {e}", parent.display())))?;
        }
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| Error::Storage(format!("writing {}: {e}", path.display())))?;
        Ok(path.to_string_lossy().into_owned())
    }
}
