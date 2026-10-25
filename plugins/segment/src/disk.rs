//! Network outputs kept on disk, so reopening a photo in a later session
//! doesn't run the network again. Each output is one small file named by
//! its fingerprint (which covers the model and the photo). Values are
//! stored exactly, so a mask from the disk renders the same as a fresh one.
//! When the folder grows past its limit, the files used least recently are
//! deleted.

use std::path::PathBuf;
use std::time::SystemTime;

/// Identifies the file format, so an older or damaged file is ignored.
const MAGIC: &[u8; 8] = b"APMASK01";

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl DiskCache {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            dir: dir.into(),
            max_bytes,
        }
    }

    fn path(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}.mask"))
    }

    /// The stored output for `key`, if there is one holding `len` values.
    pub fn load(&self, key: u64, len: usize) -> Option<Vec<f32>> {
        let path = self.path(key);
        let bytes = std::fs::read(&path).ok()?;
        let body = bytes.strip_prefix(MAGIC.as_slice())?;
        let (count, values) = body.split_first_chunk::<8>()?;
        if u64::from_le_bytes(*count) != len as u64 || values.len() != len * 4 {
            return None;
        }
        // Mark it as recently used, so pruning keeps it.
        if let Ok(file) = std::fs::File::options().append(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(
            values
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    }

    /// Store `values` for `key`. Best effort: an output that
    /// can't be stored is just computed again next time.
    pub fn store(&self, key: u64, values: &[f32]) {
        let mut bytes = Vec::with_capacity(MAGIC.len() + 8 + values.len() * 4);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        // Write then rename, so a reader never sees half a file.
        let path = self.path(key);
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        self.prune();
    }

    /// Delete the least recently used files until the folder fits.
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "mask"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

/// Where the app keeps network outputs.
pub fn default_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aperio")
        .join("masks")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aperio-masks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn file_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |d| d.count())
    }

    #[test]
    fn round_trips_outputs_exactly() {
        let dir = temp_dir("round");
        let cache = DiskCache::new(&dir, u64::MAX);
        let values: Vec<f32> = (0..1000).map(|i| i as f32 / 999.0).collect();
        assert!(cache.load(1, values.len()).is_none());
        cache.store(1, &values);
        let back = cache.load(1, values.len()).unwrap();
        assert_eq!(back, values);
        // A different length means a different network: not a match.
        assert!(cache.load(1, 999).is_none());
        // A damaged file is ignored.
        std::fs::write(cache.path(2), b"APMASK01junk").unwrap();
        assert!(cache.load(2, 4).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prunes_the_least_recently_used() {
        let dir = temp_dir("prune");
        let values = vec![0.5f32; 100];
        // Room for two outputs (416 bytes each).
        let cache = DiskCache::new(&dir, 850);
        cache.store(1, &values);
        cache.store(2, &values);
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        for key in [1, 2] {
            let file = std::fs::File::options()
                .append(true)
                .open(cache.path(key))
                .unwrap();
            file.set_modified(old - std::time::Duration::from_secs(key))
                .unwrap();
        }
        // Using 2 makes 1 the oldest, so storing a third deletes 1.
        assert!(cache.load(2, values.len()).is_some());
        cache.store(3, &values);
        assert_eq!(file_count(&dir), 2);
        assert!(cache.load(1, values.len()).is_none());
        assert!(cache.load(2, values.len()).is_some());
        assert!(cache.load(3, values.len()).is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
