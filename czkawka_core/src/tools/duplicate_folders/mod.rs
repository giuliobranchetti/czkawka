//! Read-only folder comparison by the multiset of (BLAKE3, size) file contents.
//! Every discovered directory is a candidate. Recursive mode flattens descendants;
//! ancestor/descendant pairs and empty directories are never reported as duplicates.
mod cache;
mod compare;
mod inventory;
mod report;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use self::cache::{CachedHash, hash_file};
use self::inventory::Inventory;
use crate::flc;

/// Input filters define the compared view; exactness only applies to that view.
#[derive(Debug, Clone)]
pub struct FolderScanOptions {
    pub included_paths: Vec<PathBuf>,
    pub excluded_paths: Vec<PathBuf>,
    pub reference_paths: Vec<PathBuf>,
    pub excluded_items: Vec<String>,
    pub allowed_extensions: Vec<String>,
    pub excluded_extensions: Vec<String>,
    pub recursive: bool,
    pub minimum_files: usize,
    pub minimum_bytes: u64,
    /// A partial pair qualifies if either directional count coverage meets this threshold.
    pub minimum_coverage: u8,
    pub exact_only: bool,
    pub threads: usize,
    /// Opt in only on storage whose timestamps and file identities are reliable.
    /// Fresh hashing is the default, including for SMB.
    pub trust_cache: bool,
    pub cache_path: Option<PathBuf>,
    pub maximum_pairs: usize,
    pub maximum_candidate_visits: usize,
}

impl Default for FolderScanOptions {
    fn default() -> Self {
        Self {
            included_paths: Vec::new(),
            excluded_paths: Vec::new(),
            reference_paths: Vec::new(),
            excluded_items: Vec::new(),
            allowed_extensions: Vec::new(),
            excluded_extensions: Vec::new(),
            recursive: true,
            minimum_files: 1,
            minimum_bytes: 0,
            minimum_coverage: 90,
            exact_only: false,
            threads: 2,
            trust_cache: false,
            cache_path: None,
            maximum_pairs: 20_000,
            maximum_candidate_visits: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FolderScanPhase {
    Collecting,
    Hashing,
    Comparing,
}

#[derive(Debug, Clone, Copy)]
pub struct FolderProgress {
    pub phase: FolderScanPhase,
    pub completed: usize,
    pub total: usize,
}

/// One path occurrence, including repeated contents and hard links.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderFile {
    pub path: PathBuf,
    pub size: u64,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSummary {
    pub path: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
    pub complete: bool,
    pub reference: bool,
    #[serde(rename = "file_indices")]
    pub(crate) files: Vec<usize>,
}

/// Directional counts are exact integers; percentages are derived, never used for exactness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderPair {
    pub folder_a: usize,
    pub folder_b: usize,
    pub matched_count: usize,
    pub matched_bytes: u64,
    pub exact: bool,
    /// Logical matching bytes in one side, not a promise of physically reclaimable space.
    pub potential_duplicate_bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FolderReport {
    pub folders: Vec<FolderSummary>,
    pub pairs: Vec<FolderPair>,
    pub files: Vec<FolderFile>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
    pub truncated: bool,
    pub hashes_computed: usize,
    pub cache_hits: usize,
    pub candidate_visits: usize,
}

/// Matched path occurrences are paired deterministically; extra copies stay exclusive.
#[derive(Debug, Default)]
pub struct FolderDifferences {
    pub common: Vec<(PathBuf, PathBuf)>,
    pub only_a: Vec<PathBuf>,
    pub only_b: Vec<PathBuf>,
}

impl FolderReport {
    pub fn differences(&self, pair: &FolderPair) -> FolderDifferences {
        compare::differences(self, pair)
    }
}

/// Scan without changing source files or metadata. Cache writes are confined to cache_path.
/// Errors invalidate the affected directory and its recursive ancestors, not unrelated pairs.
pub fn scan_folders(options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: impl Fn(FolderProgress) + Sync) -> FolderReport {
    let mut inventory = Inventory::collect(options, stop, &progress);
    if stop.load(Ordering::Relaxed) {
        inventory.report.cancelled = true;
        return inventory.report;
    }
    let mut size_counts = BTreeMap::<u64, usize>::new();
    for file in &inventory.report.files {
        *size_counts.entry(file.size).or_default() += 1;
    }
    let cached = cache::load(options, &mut inventory.report.warnings);
    let processed = AtomicUsize::new(0);
    let bytes_read = Arc::new(AtomicU64::new(0));
    let total = inventory.report.files.len();
    progress(FolderProgress {
        phase: FolderScanPhase::Hashing,
        completed: 0,
        total,
    });
    let pool = match rayon::ThreadPoolBuilder::new().num_threads(options.threads.clamp(1, 32)).build() {
        Ok(pool) => pool,
        Err(error) => {
            inventory.report.warnings.push(flc!("core_df_workers", reason = error.to_string()));
            return inventory.report;
        }
    };
    let results: Vec<_> = pool.install(|| {
        inventory
            .report
            .files
            .par_iter()
            .map(|file| {
                let result = if stop.load(Ordering::Relaxed) || size_counts.get(&file.size) == Some(&1) {
                    Ok(None)
                } else {
                    hash_file(file, cached.get(&file.path), stop, &bytes_read)
                };
                let completed = processed.fetch_add(1, Ordering::Relaxed) + 1;
                if completed.is_multiple_of(32) || completed == total {
                    progress(FolderProgress {
                        phase: FolderScanPhase::Hashing,
                        completed,
                        total,
                    });
                }
                result
            })
            .collect()
    });
    let mut updated = BTreeMap::<PathBuf, CachedHash>::new();
    for (idx, result) in results.into_iter().enumerate() {
        match result {
            Ok(Some((entry, hit))) => {
                if let Some(file) = inventory.report.files.get_mut(idx) {
                    file.hash = Some(entry.hash.clone());
                    updated.insert(file.path.clone(), entry);
                }
                if hit {
                    inventory.report.cache_hits += 1;
                } else {
                    inventory.report.hashes_computed += 1;
                }
            }
            Ok(None) => {}
            Err(error) => {
                inventory.invalidate_file(idx);
                inventory.report.warnings.push(error);
            }
        }
    }
    inventory.revalidate(options);
    inventory.report.cancelled = stop.load(Ordering::Relaxed);
    if !inventory.report.cancelled {
        cache::save(options, &updated, &mut inventory.report.warnings);
        compare::find_pairs(&mut inventory.report, options, stop, &progress);
    }
    inventory.report.cancelled = stop.load(Ordering::Relaxed);
    if inventory.report.cancelled {
        inventory.report.pairs.clear();
    }
    inventory.report
}
