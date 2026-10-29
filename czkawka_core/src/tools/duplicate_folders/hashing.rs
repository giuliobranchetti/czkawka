use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rayon::prelude::*;

use super::cache::{self, CachedHash, PreparedHash};
use super::inventory::Inventory;
use super::{FolderProgress, FolderScanOptions, FolderScanPhase};
use crate::flc;

pub(super) fn hash_inventory(inventory: &mut Inventory, options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: &(impl Fn(FolderProgress) + Sync)) {
    let pool = match rayon::ThreadPoolBuilder::new().num_threads(options.threads.clamp(1, 32)).build() {
        Ok(pool) => pool,
        Err(error) => {
            inventory.report.warnings.push(flc!("core_df_workers", reason = error.to_string()));
            return;
        }
    };
    let bytes = Arc::new(AtomicU64::new(0));
    let mut prepared = prepare_files(inventory, options, stop, progress, &pool, &bytes);
    complete_candidates(inventory, &mut prepared, stop, progress, &pool, &bytes);
    let mut updated = BTreeMap::<PathBuf, CachedHash>::new();
    for (index, item) in prepared.into_iter().enumerate() {
        if let Some(item) = item
            && let Some(file) = inventory.report.files.get_mut(index)
        {
            file.hash = item.entry.hash.clone();
            updated.insert(file.path.clone(), item.entry);
        }
    }
    inventory.report.bytes_read = bytes.load(Ordering::Relaxed);
    inventory.revalidate(stop, &pool, progress);
    if !stop.load(Ordering::Relaxed) {
        cache::save(options, &updated, &mut inventory.report.warnings);
    }
}

fn prepare_files(
    inventory: &mut Inventory,
    options: &FolderScanOptions,
    stop: &Arc<AtomicBool>,
    progress: &(impl Fn(FolderProgress) + Sync),
    pool: &rayon::ThreadPool,
    bytes: &Arc<AtomicU64>,
) -> Vec<Option<PreparedHash>> {
    let cached = cache::load(options, &mut inventory.report.warnings);
    let mut sizes = BTreeMap::<u64, usize>::new();
    for file in &inventory.report.files {
        *sizes.entry(file.size).or_default() += 1;
    }
    let total = inventory.report.files.len();
    let processed = AtomicUsize::new(0);
    progress(FolderProgress {
        phase: FolderScanPhase::PreHashing,
        completed: 0,
        total,
    });
    let results: Vec<_> = pool.install(|| {
        inventory
            .report
            .files
            .par_iter()
            .map(|file| {
                let result = if stop.load(Ordering::Relaxed) || sizes.get(&file.size) == Some(&1) {
                    Ok(None)
                } else {
                    cache::prepare(file, inventory.file_stamp(file), cached.get(&file.path), bytes)
                };
                let completed = processed.fetch_add(1, Ordering::Relaxed) + 1;
                progress(FolderProgress {
                    phase: FolderScanPhase::PreHashing,
                    completed,
                    total,
                });
                result
            })
            .collect()
    });
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| match result {
            Ok(item) => {
                if let Some(item) = &item {
                    inventory.report.cache_hits += usize::from(item.cache_hit);
                    inventory.report.prehashes_computed += usize::from(item.sampled);
                    inventory.report.hashes_computed += usize::from(item.sampled && !item.cache_hit && item.entry.hash.is_some());
                }
                item
            }
            Err(error) => {
                inventory.invalidate_file(index);
                inventory.report.warnings.push(error);
                None
            }
        })
        .collect()
}

fn complete_candidates(
    inventory: &mut Inventory,
    prepared: &mut [Option<PreparedHash>],
    stop: &Arc<AtomicBool>,
    progress: &(impl Fn(FolderProgress) + Sync),
    pool: &rayon::ThreadPool,
    bytes: &Arc<AtomicU64>,
) {
    let mut counts = BTreeMap::<(u64, &str), usize>::new();
    for (file, item) in inventory.report.files.iter().zip(prepared.iter()) {
        if let Some(hash) = item.as_ref().and_then(|item| item.entry.prehash.as_deref()) {
            *counts.entry((file.size, hash)).or_default() += 1;
        }
    }
    let candidates: Vec<_> = inventory
        .report
        .files
        .iter()
        .zip(prepared.iter())
        .enumerate()
        .filter_map(|(index, (file, item))| {
            let entry = &item.as_ref()?.entry;
            (entry.hash.is_none() && counts.get(&(file.size, entry.prehash.as_deref()?)).is_some_and(|count| *count > 1)).then_some(index)
        })
        .collect();
    let total = candidates.len();
    let processed = AtomicUsize::new(0);
    progress(FolderProgress {
        phase: FolderScanPhase::Hashing,
        completed: 0,
        total,
    });
    let results: Vec<_> = pool.install(|| {
        candidates
            .par_iter()
            .map(|&index| {
                let file = inventory.report.files.get(index).expect("Candidate indices refer to inventoried files");
                let item = prepared.get(index).and_then(Option::as_ref).expect("Full-hash candidates have a prepared partial hash");
                let result = if stop.load(Ordering::Relaxed) {
                    Ok(None)
                } else {
                    cache::complete(file, &item.entry, stop, bytes)
                };
                let completed = processed.fetch_add(1, Ordering::Relaxed) + 1;
                progress(FolderProgress {
                    phase: FolderScanPhase::Hashing,
                    completed,
                    total,
                });
                (index, result)
            })
            .collect()
    });
    for (index, result) in results {
        match result {
            Ok(Some(entry)) => {
                if let Some(Some(item)) = prepared.get_mut(index) {
                    item.entry = entry;
                }
                inventory.report.hashes_computed += 1;
            }
            Ok(None) => {}
            Err(error) => {
                inventory.invalidate_file(index);
                inventory.report.warnings.push(error);
                if let Some(item) = prepared.get_mut(index) {
                    *item = None;
                }
            }
        }
    }
}
