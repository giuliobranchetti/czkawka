//! Read-only folder comparison by the multiset of (BLAKE3, size) file contents.
//! Every discovered directory is a candidate. Recursive mode flattens descendants;
//! ancestor/descendant pairs and empty directories are never reported as duplicates.
mod cache;
mod compare;
mod hashing;
mod inventory;
mod report;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use self::inventory::Inventory;

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
    /// Limit partial-search posting visits; exact signatures are bounded by maximum_pairs.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderScanPhase {
    Collecting,
    PreHashing,
    Hashing,
    Verifying,
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
    #[serde(default)]
    pub prehashes_computed: usize,
    #[serde(default)]
    pub bytes_read: u64,
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
    hashing::hash_inventory(&mut inventory, options, stop, &progress);
    inventory.report.cancelled = stop.load(Ordering::Relaxed);
    if !inventory.report.cancelled {
        compare::find_pairs(&mut inventory.report, options, stop, &progress);
    }
    inventory.report.cancelled = stop.load(Ordering::Relaxed);
    if inventory.report.cancelled {
        inventory.report.pairs.clear();
    }
    inventory.report
}
