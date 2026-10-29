use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{FolderDifferences, FolderFile, FolderPair, FolderProgress, FolderReport, FolderScanOptions, FolderScanPhase, FolderSummary};
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

fn content_counts<'a>(files: &'a [FolderFile], folder: &FolderSummary) -> BTreeMap<(u64, &'a str), usize> {
    let mut counts = BTreeMap::new();
    for file in folder.files.iter().filter_map(|index| files.get(*index)) {
        if let Some(hash) = &file.hash {
            *counts.entry((file.size, hash.as_str())).or_default() += 1;
        }
    }
    counts
}

fn eligible(folder: &FolderSummary, options: &FolderScanOptions) -> bool {
    folder.complete && folder.file_count >= options.minimum_files.max(1) && folder.total_bytes >= options.minimum_bytes
}

fn subtree_ends(folders: &[FolderSummary]) -> Vec<usize> {
    let mut ends = vec![folders.len(); folders.len()];
    let mut ancestors = Vec::<usize>::new();
    for (index, folder) in folders.iter().enumerate() {
        while let Some(&parent) = ancestors.last() {
            if folders.get(parent).is_some_and(|parent| folder.path.starts_with(&parent.path)) {
                break;
            }
            if let Some(end) = ends.get_mut(parent) {
                *end = index;
            }
            ancestors.pop();
        }
        ancestors.push(index);
    }
    ends
}

pub(super) fn find_pairs(report: &mut FolderReport, options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: &impl Fn(FolderProgress)) {
    progress(FolderProgress {
        phase: FolderScanPhase::Comparing,
        completed: 0,
        total: 0,
    });
    if report.truncated || stop.load(Ordering::Relaxed) {
        return;
    }
    report.folders.sort_by(|a, b| a.path.cmp(&b.path));
    let ends = subtree_ends(&report.folders);
    if options.exact_only {
        find_exact_pairs(report, options, stop, progress, &ends);
        return;
    }
    let mut postings = BTreeMap::<(u64, &str), Vec<(usize, usize)>>::new();
    for (index, folder) in report.folders.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if eligible(folder, options) {
            for (key, count) in content_counts(&report.files, folder) {
                postings.entry(key).or_default().push((index, count));
            }
        }
    }
    let total = postings.len();
    let mut candidates = BTreeMap::<(usize, usize), (usize, u64)>::new();
    for (completed, ((size, _hash), folders)) in postings.into_iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        progress(FolderProgress {
            phase: FolderScanPhase::Comparing,
            completed,
            total,
        });
        for &(left, left_count) in &folders {
            let end = ends.get(left).copied().expect("Posting indices refer to sorted folder intervals");
            let first = folders.partition_point(|&(index, _)| index < end);
            for &(right, right_count) in folders.iter().skip(first) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                report.candidate_visits += 1;
                if report.candidate_visits > options.maximum_candidate_visits {
                    report.truncated = true;
                    report.warnings.push(flc!("core_df_candidate_limit"));
                    return;
                }
                let count = left_count.min(right_count);
                let candidate = candidates.entry((left, right)).or_default();
                candidate.0 += count;
                candidate.1 += count as u64 * size;
            }
        }
    }
    for ((left, right), (matched_count, matched_bytes)) in candidates {
        let (Some(a), Some(b)) = (report.folders.get(left), report.folders.get(right)) else {
            continue;
        };
        let exact = matched_count == a.file_count && matched_count == b.file_count;
        let denominator = a.file_count.min(b.file_count) as u128;
        if matched_count as u128 * 100 < denominator * u128::from(options.minimum_coverage.min(100)) {
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

type Signature<'a> = Vec<((u64, &'a str), usize)>;

fn find_exact_pairs(report: &mut FolderReport, options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: &impl Fn(FolderProgress), ends: &[usize]) {
    let mut groups = BTreeMap::<Signature<'_>, Vec<usize>>::new();
    for (index, folder) in report.folders.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if !eligible(folder, options) {
            continue;
        }
        let counts = content_counts(&report.files, folder);
        if counts.values().sum::<usize>() != folder.file_count {
            continue;
        }
        groups.entry(counts.into_iter().collect()).or_default().push(index);
    }
    let total = groups.len();
    for (completed, (_signature, folders)) in groups.into_iter().enumerate() {
        progress(FolderProgress {
            phase: FolderScanPhase::Comparing,
            completed,
            total,
        });
        for &left in &folders {
            let end = ends.get(left).copied().expect("Signature indices refer to sorted folder intervals");
            let first = folders.partition_point(|&index| index < end);
            for &right in folders.iter().skip(first) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                if report.pairs.len() >= options.maximum_pairs {
                    report.truncated = true;
                    report.warnings.push(flc!("core_df_result_limit"));
                    return;
                }
                let a = report.folders.get(left).expect("Signature indices refer to inventoried folders");
                report.candidate_visits += 1;
                report.pairs.push(FolderPair {
                    folder_a: left,
                    folder_b: right,
                    matched_count: a.file_count,
                    matched_bytes: a.total_bytes,
                    exact: true,
                    potential_duplicate_bytes: a.total_bytes,
                });
            }
        }
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
