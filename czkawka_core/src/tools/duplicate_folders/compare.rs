use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{FolderDifferences, FolderPair, FolderProgress, FolderReport, FolderScanOptions, FolderScanPhase, FolderSummary};
use crate::flc;

type Content = (u64, String);
type Bag = BTreeMap<Content, Vec<PathBuf>>;

fn bag(report: &FolderReport, folder: &FolderSummary) -> Bag {
    let mut contents = Bag::new();
    for file in folder.files.iter().filter_map(|idx| report.files.get(*idx)) {
        if let Some(hash) = &file.hash {
            contents.entry((file.size, hash.clone())).or_default().push(file.path.clone());
        }
    }
    for paths in contents.values_mut() {
        paths.sort();
    }
    contents
}

pub(super) fn find_pairs(report: &mut FolderReport, options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: &impl Fn(FolderProgress)) {
    let mut postings = BTreeMap::<Content, Vec<(usize, usize)>>::new();
    for (idx, folder) in report.folders.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if !folder.complete || folder.file_count < options.minimum_files.max(1) || folder.total_bytes < options.minimum_bytes {
            continue;
        }
        for (key, paths) in bag(report, folder) {
            postings.entry(key).or_default().push((idx, paths.len()));
        }
    }
    let total = postings.len();
    let mut candidates = BTreeMap::<(usize, usize), (usize, u64)>::new();
    'contents: for (completed, ((size, _hash), folders)) in postings.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        progress(FolderProgress {
            phase: FolderScanPhase::Comparing,
            completed,
            total,
        });
        for (offset, &(left, left_count)) in folders.iter().enumerate() {
            for &(right, right_count) in folders.iter().skip(offset + 1) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                report.candidate_visits += 1;
                if report.candidate_visits > options.maximum_candidate_visits {
                    report.truncated = true;
                    report.warnings.push(flc!("core_df_candidate_limit"));
                    break 'contents;
                }
                let (Some(a), Some(b)) = (report.folders.get(left), report.folders.get(right)) else {
                    continue;
                };
                if a.path.starts_with(&b.path) || b.path.starts_with(&a.path) {
                    continue;
                }
                let count = left_count.min(right_count);
                let candidate = candidates.entry((left, right)).or_default();
                candidate.0 += count;
                candidate.1 += count as u64 * size;
            }
        }
    }
    // A truncated inverted-index pass has incomplete counts: never present those as exact.
    if report.truncated {
        return;
    }
    for ((left, right), (matched_count, matched_bytes)) in candidates {
        let (Some(a), Some(b)) = (report.folders.get(left), report.folders.get(right)) else {
            continue;
        };
        let exact = matched_count == a.file_count && matched_count == b.file_count;
        let denominator = a.file_count.min(b.file_count) as u128;
        if (options.exact_only && !exact) || (matched_count as u128 * 100 < denominator * u128::from(options.minimum_coverage.min(100))) {
            continue;
        }
        if report.pairs.len() >= options.maximum_pairs {
            report.truncated = true;
            report.warnings.push(flc!("core_df_result_limit"));
            break;
        }
        report.pairs.push(FolderPair {
            folder_a: left,
            folder_b: right,
            matched_count,
            matched_bytes,
            exact,
            potential_duplicate_bytes: matched_bytes,
        });
    }
}

pub(super) fn differences(report: &FolderReport, pair: &FolderPair) -> FolderDifferences {
    let mut result = FolderDifferences::default();
    let (Some(a), Some(b)) = (report.folders.get(pair.folder_a), report.folders.get(pair.folder_b)) else {
        return result;
    };
    let a_bag = bag(report, a);
    let mut b_bag = bag(report, b);
    for (key, paths_a) in a_bag {
        let paths_b = b_bag.remove(&key).unwrap_or_default();
        let common = paths_a.len().min(paths_b.len());
        result.common.extend(paths_a.iter().zip(&paths_b).map(|(a, b)| (a.clone(), b.clone())));
        result.only_a.extend(paths_a.into_iter().skip(common));
        result.only_b.extend(paths_b.into_iter().skip(common));
    }
    result.only_b.extend(b_bag.into_values().flatten());
    for (folder, exclusive) in [(a, &mut result.only_a), (b, &mut result.only_b)] {
        exclusive.extend(
            folder
                .files
                .iter()
                .filter_map(|idx| report.files.get(*idx))
                .filter(|file| file.hash.is_none())
                .map(|file| file.path.clone()),
        );
        exclusive.sort();
    }
    result
}
