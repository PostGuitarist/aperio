//! The models this plugin uses: where they come from, where they're kept,
//! and how they're downloaded and checked.
//!
//! Models are never bundled or fetched in the background. They're downloaded
//! once, when the user asks (in the app, or with `aperio models download`),
//! from a pinned revision, and verified against a SHA-256 before use.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// A model file this plugin can use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
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
    /// Square input size the network expects.
    pub input_size: u32,
    /// Per-channel mean and standard deviation the network was trained
    /// with, for sRGB values in 0..1.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Whether the output is a logit (needs a sigmoid) or already 0..1.
    pub logits: bool,
    /// Optional models aren't part of the default download; the user picks
    /// them separately, after seeing [`Model::notice`].
    pub optional: bool,
    /// What the user should know before downloading, such as license gaps.
    pub notice: Option<&'static str>,
}

/// ImageNet statistics, which most segmentation models are trained with.
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// U²-Net sky segmentation by xiongzhu666, MIT.
pub const SKY: Model = Model {
    id: "skyseg",
    name: "Sky segmentation (U²-Net)",
    license: "MIT",
    source: "https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing",
    url: "https://huggingface.co/JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx",
    sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
    size_bytes: 175_997_079,
    input_size: 320,
    mean: IMAGENET_MEAN,
    std: IMAGENET_STD,
    logits: false,
    optional: false,
    notice: None,
};

/// BiRefNet lite (salient object / subject segmentation), MIT.
pub const SUBJECT: Model = Model {
    id: "birefnet_lite",
    name: "Subject segmentation (BiRefNet lite)",
    license: "MIT",
    source: "https://github.com/ZhengPeng7/BiRefNet",
    url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
    sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
    size_bytes: 224_005_088,
    input_size: 1024,
    mean: IMAGENET_MEAN,
    std: IMAGENET_STD,
    logits: true,
    optional: false,
    notice: None,
};

/// ISNet general use (DIS), a much faster subject model with softer edges.
/// Optional: the weights state no license and were trained on DIS5K, whose
/// terms are non-commercial (see LICENSES.md). This is rembg's export of the
/// authors' released weights.
pub const FAST_SUBJECT: Model = Model {
    id: "isnet_general_use",
    name: "Fast subject segmentation (ISNet)",
    license: "No license stated",
    source: "https://github.com/xuebinqin/DIS",
    url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
    sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
    size_bytes: 178_648_008,
    input_size: 1024,
    mean: [0.5; 3],
    std: [1.0; 3],
    logits: false,
    optional: true,
    notice: Some(
        "About 15 times faster than BiRefNet, with softer edges. Its authors \
         haven't stated a license for the weights, and they were trained on \
         DIS5K, whose terms allow only non-commercial research and education. \
         Download it only if that's acceptable for how you use your photos.",
    ),
};

/// The models this build offers. ISNet is only offered with the
/// `fast-subject` feature, which is off by default.
#[cfg(feature = "fast-subject")]
pub const ALL: [Model; 3] = [SKY, SUBJECT, FAST_SUBJECT];
#[cfg(not(feature = "fast-subject"))]
pub const ALL: [Model; 2] = [SKY, SUBJECT];

/// Whether this build offers [`FAST_SUBJECT`].
pub const FAST_SUBJECT_ENABLED: bool = cfg!(feature = "fast-subject");

/// Look a model up by id.
pub fn find(id: &str) -> Option<Model> {
    ALL.into_iter().find(|m| m.id == id)
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

impl Model {
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

        let ok = Model {
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
