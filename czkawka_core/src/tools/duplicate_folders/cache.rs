use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::{FolderFile, FolderScanOptions};
use crate::common::model::HashType;
use crate::flc;
use crate::tools::duplicate::{DuplicateEntry, PREHASHING_BUFFER_SIZE, THREAD_BUFFER, hash_calculation, hash_calculation_limit};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Stamp {
    size: u64,
    modified: (u64, u32),
    identity: String,
}

pub(super) fn stamp(path: &Path, metadata: &Metadata) -> Result<Stamp, String> {
    let modified = metadata
        .modified()
        .and_then(|time| time.duration_since(UNIX_EPOCH).map_err(std::io::Error::other))
        .map_err(|error| flc!("core_df_timestamp", path = path.to_string_lossy(), reason = error.to_string()))?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}:{}:{}", metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
    };
    #[cfg(windows)]
    let identity = file_id::get_file_id(path).map(|id| format!("{id:?}")).unwrap_or_default();
    #[cfg(not(any(unix, windows)))]
    let identity = format!("{:?}", metadata.created().ok());
    Ok(Stamp {
        size: metadata.len(),
        modified: (modified.as_secs(), modified.subsec_nanos()),
        identity,
    })
}

pub(super) fn regular_metadata(path: &Path) -> Result<Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| flc!("core_df_inspect", path = path.to_string_lossy(), reason = error.to_string()))?;
    if is_link(&metadata) {
        return Err(flc!("core_df_link", path = path.to_string_lossy()));
    }
    Ok(metadata)
}

pub(super) fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CachedHash {
    stamp: Stamp,
    pub hash: Option<String>,
    #[serde(default)]
    pub prehash: Option<String>,
}

pub(super) struct PreparedHash {
    pub entry: CachedHash,
    pub cache_hit: bool,
    pub sampled: bool,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    entries: BTreeMap<PathBuf, CachedHash>,
}

pub(super) fn load(options: &FolderScanOptions, warnings: &mut Vec<String>) -> BTreeMap<PathBuf, CachedHash> {
    let Some(path) = options.cache_path.as_ref().filter(|_| options.trust_cache) else {
        return BTreeMap::new();
    };
    let result = (|| -> Result<Cache, String> {
        let file = fs::File::open(path).map_err(|error| error.to_string())?;
        if file.metadata().map_err(|error| error.to_string())?.len() > 64 * 1024 * 1024 {
            return Err(flc!("core_df_cache_too_large"));
        }
        serde_json::from_reader(BufReader::new(file)).map_err(|error| error.to_string())
    })();
    match result {
        Ok(cache) if cache.version == 1 => cache.entries,
        Ok(_) => {
            warnings.push(flc!("core_df_cache_version", path = path.to_string_lossy()));
            BTreeMap::new()
        }
        Err(error) => {
            if path.exists() {
                warnings.push(flc!("core_df_cache_load", path = path.to_string_lossy(), reason = error));
            }
            BTreeMap::new()
        }
    }
}

pub(super) fn save(options: &FolderScanOptions, entries: &BTreeMap<PathBuf, CachedHash>, warnings: &mut Vec<String>) {
    let Some(path) = options.cache_path.as_ref().filter(|_| options.trust_cache) else {
        return;
    };
    let result = (|| -> Result<(), std::io::Error> {
        let parent = path.parent().ok_or_else(|| std::io::Error::other(flc!("core_df_cache_no_parent")))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            serde_json::to_writer(
                &mut writer,
                &Cache {
                    version: 1,
                    entries: entries.clone(),
                },
            )?;
            writer.flush()?;
        }
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    })();
    if let Err(error) = result {
        warnings.push(flc!("core_df_cache_save", path = path.to_string_lossy(), reason = error.to_string()));
    }
}

fn valid_digest(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn prepare(file: &FolderFile, original: &Stamp, cached: Option<&CachedHash>, bytes: &Arc<AtomicU64>) -> Result<Option<PreparedHash>, String> {
    let cached = cached.filter(|entry| {
        !original.identity.is_empty() && &entry.stamp == original && entry.hash.as_deref().is_none_or(valid_digest) && entry.prehash.as_deref().is_none_or(valid_digest)
    });
    let mut entry = cached.cloned().unwrap_or_else(|| CachedHash {
        stamp: original.clone(),
        hash: None,
        prehash: None,
    });
    if entry.prehash.is_some() {
        return Ok(Some(PreparedHash {
            entry,
            cache_hit: true,
            sampled: false,
        }));
    }
    let input = DuplicateEntry {
        path: file.path.clone(),
        size: file.size,
        ..Default::default()
    };
    let prehash = THREAD_BUFFER.with_borrow_mut(|buffer| hash_calculation_limit(buffer, &input, HashType::Blake3, PREHASHING_BUFFER_SIZE, bytes))?;
    if file.size <= 2 * PREHASHING_BUFFER_SIZE {
        let after = regular_metadata(&file.path)?;
        if !after.is_file() || &stamp(&file.path, &after)? != original {
            return Err(flc!("core_df_hash_changed", path = file.path.to_string_lossy()));
        }
        entry.hash = Some(prehash.clone());
    }
    entry.prehash = Some(prehash);
    Ok(Some(PreparedHash {
        entry,
        cache_hit: cached.is_some(),
        sampled: true,
    }))
}

pub(super) fn complete(file: &FolderFile, prepared: &CachedHash, stop: &Arc<AtomicBool>, bytes: &Arc<AtomicU64>) -> Result<Option<CachedHash>, String> {
    let before = regular_metadata(&file.path)?;
    if !before.is_file() || before.len() != file.size {
        return Err(flc!("core_df_changed", path = file.path.to_string_lossy()));
    }
    let before_stamp = stamp(&file.path, &before)?;
    if before_stamp != prepared.stamp {
        return Err(flc!("core_df_changed", path = file.path.to_string_lossy()));
    }
    let entry = DuplicateEntry {
        path: file.path.clone(),
        size: file.size,
        ..Default::default()
    };
    let Some(hash) = THREAD_BUFFER.with_borrow_mut(|buffer| hash_calculation(buffer, &entry, HashType::Blake3, bytes, stop))? else {
        return Ok(None);
    };
    let after = regular_metadata(&file.path)?;
    if !after.is_file() || stamp(&file.path, &after)? != before_stamp {
        return Err(flc!("core_df_hash_changed", path = file.path.to_string_lossy()));
    }
    Ok(Some(CachedHash {
        stamp: before_stamp,
        hash: Some(hash),
        prehash: prepared.prehash.clone(),
    }))
}
