//! Model files: where they come from, where they're kept, and how they're
//! downloaded and checked.
//!
//! Models are never bundled or fetched in the background. They're downloaded
//! once, when the user asks (in the app, or with `aperio models download`),
//! from a pinned URL, and verified against a SHA-256 before use.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// A model file a plugin can use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFile {
    /// Short id, also the file name without `.onnx`.
    pub id: &'static str,
    pub name: &'static str,
    /// SPDX license of the weights, or what's known when there's none.
    pub license: &'static str,
    /// Project page, for LICENSES.md and the app.
    pub source: &'static str,
    /// Download URL, pinned to a revision.
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    /// Optional models aren't part of the default download; the user picks
    /// them separately, after seeing [`ModelFile::notice`].
    pub optional: bool,
    /// What the user should know before downloading, such as license gaps.
    pub notice: Option<&'static str>,
}

/// Where models are kept: `$APERIO_MODELS_DIR` if set, otherwise the
/// platform's data folder (e.g. `~/.local/share/aperio/models`).
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("APERIO_MODELS_DIR") {
        return dir.into();
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aperio")
        .join("models")
}

impl ModelFile {
    pub fn path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.onnx", self.id))
    }

    /// Whether the model is downloaded. Checks the size only; the hash is
    /// verified when downloading.
    pub fn is_installed(&self, dir: &Path) -> bool {
        std::fs::metadata(self.path(dir)).is_ok_and(|m| m.len() == self.size_bytes)
    }

    /// Download the model into `dir`, verify it, and move it into place.
    /// `progress` gets (bytes so far, total bytes).
    pub fn download(
        &self,
        dir: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        let partial = dir.join(format!("{}.onnx.part", self.id));
        let response = ureq::get(self.url)
            .call()
            .map_err(|e| format!("download failed: {e}"))?;
        let mut body = response.into_body().into_reader();
        let mut file = File::create(&partial).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = body
                .read(&mut buf)
                .map_err(|e| format!("download failed: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(done, self.size_bytes);
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        self.finish(&partial, &hex(&hasher.finalize()), dir)
    }

    /// Check a downloaded file's hash and move it into place.
    fn finish(&self, partial: &Path, sha256: &str, dir: &Path) -> Result<PathBuf, String> {
        if sha256 != self.sha256 {
            let _ = std::fs::remove_file(partial);
            return Err(format!(
                "{} didn't match its expected checksum, so it wasn't installed",
                self.name
            ));
        }
        let path = self.path(dir);
        std::fs::rename(partial, &path).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKY: ModelFile = ModelFile {
        id: "skyseg",
        name: "Sky",
        license: "MIT",
        source: "",
        url: "",
        sha256: "0",
        size_bytes: 1,
        optional: false,
        notice: None,
    };

    #[test]
    fn rejects_files_with_the_wrong_hash() {
        let dir = std::env::temp_dir().join(format!("aperio-models-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let partial = dir.join("x.part");
        std::fs::write(&partial, b"not a model").unwrap();
        let sha = hex(&Sha256::digest(b"not a model"));
        assert!(SKY.finish(&partial, &sha, &dir).is_err());
        assert!(!partial.exists());
        assert!(!SKY.is_installed(&dir));

        let ok = ModelFile {
            sha256: "abc",
            ..SKY
        };
        std::fs::write(&partial, b"x").unwrap();
        assert_eq!(
            ok.finish(&partial, "abc", &dir).unwrap(),
            dir.join("skyseg.onnx")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
