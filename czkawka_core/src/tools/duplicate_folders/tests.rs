use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tempfile::TempDir;

use super::*;

fn fixtures() -> (TempDir, FolderScanOptions) {
    let temp = TempDir::new().unwrap();
    for name in ["a", "b"] {
        fs::create_dir(temp.path().join(name)).unwrap();
    }
    let options = FolderScanOptions {
        included_paths: vec![temp.path().join("a"), temp.path().join("b")],
        minimum_coverage: 0,
        ..Default::default()
    };
    (temp, options)
}

fn write(root: &Path, name: &str, value: &[u8]) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn scan(options: &FolderScanOptions) -> FolderReport {
    scan_folders(options, &Arc::new(AtomicBool::new(false)), |_| {})
}

#[test]
fn renamed_files_and_flattened_structure_are_exact() {
    let (temp, options) = fixtures();
    write(temp.path(), "a/photo.jpg", b"original");
    write(temp.path(), "a/sub/second.jpg", b"second");
    write(temp.path(), "b/renamed.jpg", b"original");
    write(temp.path(), "b/other.jpg", b"second");
    let report = scan(&options);
    let pair = report.pairs.iter().find(|pair| pair.exact).unwrap();
    assert_eq!(pair.matched_count, 2);
    assert_eq!(pair.matched_bytes, 14);
    assert_eq!(report.differences(pair).common.len(), 2);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(fs::read(temp.path().join("a/photo.jpg")).unwrap(), b"original");
}

#[test]
fn subset_counts_and_bytes_are_directional() {
    let (temp, options) = fixtures();
    for (name, bytes) in [("a/1", b"same".as_slice()), ("a/2", b"extra".as_slice()), ("b/3", b"same".as_slice())] {
        write(temp.path(), name, bytes);
    }
    let report = scan(&options);
    assert_eq!(report.pairs.len(), 1);
    let pair = report.pairs.first().unwrap();
    assert!(!pair.exact);
    assert_eq!(pair.matched_count, 1);
    assert_eq!(pair.matched_bytes, 4);
    let counts: Vec<_> = report.folders.iter().map(|folder| folder.file_count).collect();
    assert!(counts.contains(&1) && counts.contains(&2));
    let differences = report.differences(pair);
    assert_eq!(differences.only_a.len() + differences.only_b.len(), 1);
    assert_eq!(differences.common.len(), 1);
    assert!(scan(&FolderScanOptions { exact_only: true, ..options }).pairs.is_empty());
}

#[test]
fn repeated_content_is_a_multiset_including_empty_files() {
    let (temp, options) = fixtures();
    for name in ["a/1", "a/2", "b/3"] {
        write(temp.path(), name, b"");
    }
    let report = scan(&options);
    let pair = report.pairs.first().unwrap();
    assert!(!pair.exact);
    assert_eq!(pair.matched_count, 1);
    assert_eq!(pair.matched_bytes, 0);
    let differences = report.differences(pair);
    assert_eq!(differences.only_a.len() + differences.only_b.len(), 1);
}

#[test]
fn empty_directories_and_different_bytes_are_not_duplicates() {
    let (temp, options) = fixtures();
    assert!(scan(&options).pairs.is_empty());
    write(temp.path(), "a/1", b"1234");
    write(temp.path(), "b/1", b"4321");
    assert!(scan(&options).pairs.is_empty());
}

#[test]
fn overlapping_roots_are_scanned_once_and_never_paired() {
    let (temp, mut options) = fixtures();
    write(temp.path(), "a/sub/1", b"same");
    options.included_paths = vec![temp.path().join("a"), temp.path().join("a/sub"), temp.path().join("a")];
    let report = scan(&options);
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.folders.len(), 2);
    assert!(report.pairs.is_empty());
}

#[test]
fn nonrecursive_mode_compares_only_direct_files_of_selected_roots() {
    let (temp, mut options) = fixtures();
    write(temp.path(), "a/sub/1", b"same");
    write(temp.path(), "b/sub/2", b"same");
    options.recursive = false;
    let report = scan(&options);
    assert!(report.pairs.is_empty());
    assert_eq!(report.folders.len(), 2);
    assert!(report.files.is_empty());
}

#[test]
fn directory_and_extension_filters_are_reused() {
    let (temp, mut options) = fixtures();
    for name in ["a/1.jpg", "b/2.jpg", "a/skip/3.jpg", "a/unique.txt"] {
        write(temp.path(), name, name.as_bytes());
    }
    write(temp.path(), "b/2.jpg", b"a/1.jpg");
    options.allowed_extensions = vec!["jpg".into()];
    options.excluded_paths = vec![temp.path().join("a/skip")];
    assert!(scan(&options).pairs.first().unwrap().exact);
}

#[test]
fn cancellation_during_hashing_and_comparing_discards_pairs() {
    let (temp, options) = fixtures();
    for name in ["a/1", "b/2"] {
        write(temp.path(), name, b"same");
    }
    for phase in [FolderScanPhase::Hashing, FolderScanPhase::Comparing] {
        let stop = Arc::new(AtomicBool::new(false));
        let report = scan_folders(&options, &stop, |progress| {
            if std::mem::discriminant(&progress.phase) == std::mem::discriminant(&phase) {
                stop.store(true, Ordering::Relaxed);
            }
        });
        assert!(report.cancelled);
        assert!(report.pairs.is_empty());
    }
}

#[test]
fn changed_file_or_directory_invalidates_ancestors() {
    for add_file in [true, false] {
        let (temp, options) = fixtures();
        for name in ["a/1", "b/2"] {
            write(temp.path(), name, b"same");
        }
        let changed = AtomicBool::new(false);
        let report = scan_folders(&options, &Arc::new(AtomicBool::new(false)), |progress| {
            if matches!(progress.phase, FolderScanPhase::Hashing) && !changed.swap(true, Ordering::Relaxed) {
                if add_file {
                    write(temp.path(), "a/new", b"new");
                } else {
                    write(temp.path(), "a/1", b"diff");
                }
            }
        });
        assert!(report.pairs.is_empty());
        assert!(report.folders.iter().any(|folder| !folder.complete));
        assert!(!report.warnings.is_empty());
    }
}

#[test]
fn cache_reuses_verified_stamps_and_invalidates_same_size_changes() {
    let (temp, mut options) = fixtures();
    for name in ["a/1", "b/2"] {
        write(temp.path(), name, b"same");
    }
    options.trust_cache = true;
    options.cache_path = Some(temp.path().join("cache/hashes.json"));
    let first = scan(&options);
    assert_eq!(first.hashes_computed, 2);
    let second = scan(&options);
    assert_eq!(second.cache_hits, 2);
    let path = temp.path().join("a/1");
    let before = fs::metadata(&path).unwrap();
    write(temp.path(), "a/1", b"diff");
    let modified = filetime::FileTime::from_last_modification_time(&before);
    filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(modified.unix_seconds() + 1, modified.nanoseconds())).unwrap();
    assert!(scan(&options).pairs.is_empty());
    options.trust_cache = false;
    assert_eq!(scan(&options).cache_hits, 0);
}

#[test]
fn reference_folders_remain_in_results_without_any_actions() {
    let (temp, mut options) = fixtures();
    for name in ["a/1", "b/2"] {
        write(temp.path(), name, b"same");
    }
    options.reference_paths = options.included_paths.clone();
    let report = scan(&options);
    assert!(report.pairs.first().unwrap().exact);
    assert!(report.folders.iter().all(|folder| folder.reference));
}

#[cfg(unix)]
#[test]
fn unreadable_files_and_symlink_cycles_prevent_exact_claims() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (temp, options) = fixtures();
    for name in ["a/1", "b/2"] {
        write(temp.path(), name, b"same");
    }
    fs::set_permissions(temp.path().join("a/1"), fs::Permissions::from_mode(0o0)).unwrap();
    let report = scan(&options);
    assert!(report.pairs.is_empty());
    assert!(report.folders.iter().any(|folder| !folder.complete));
    fs::set_permissions(temp.path().join("a/1"), fs::Permissions::from_mode(0o600)).unwrap();
    symlink(temp.path().join("a"), temp.path().join("a/cycle")).unwrap();
    assert!(scan(&options).pairs.is_empty());
}

#[test]
fn dense_partial_search_is_bounded_and_reports_truncation() {
    let temp = TempDir::new().unwrap();
    for idx in 0..50 {
        write(temp.path(), &format!("{idx}/file"), b"same");
    }
    let options = FolderScanOptions {
        included_paths: vec![temp.path().to_path_buf()],
        maximum_candidate_visits: 100,
        ..Default::default()
    };
    let report = scan(&options);
    assert!(report.truncated);
    assert!(report.pairs.is_empty());
    assert_eq!(report.candidate_visits, 101);
}

#[test]
fn reports_preserve_membership_and_cannot_overwrite_scanned_files() {
    let (temp, options) = fixtures();
    for name in ["a/one", "b/two"] {
        write(temp.path(), name, b"same");
    }
    let report = scan(&options);
    assert!(report.save_json(&temp.path().join("a/one")).is_err());
    assert_eq!(fs::read(temp.path().join("a/one")).unwrap(), b"same");
    let output = temp.path().join("report.json");
    report.save_json(&output).unwrap();
    let restored: FolderReport = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    let pair = restored.pairs.first().unwrap();
    assert!(pair.exact);
    assert_eq!(restored.differences(pair).common.len(), 1);
}

#[test]
fn partial_coverage_and_folder_limits_are_applied_to_both_sides() {
    let (temp, mut options) = fixtures();
    for directory in ["a", "b"] {
        for idx in 0..3 {
            write(temp.path(), &format!("{directory}/{idx}"), &[idx, 0]);
        }
    }
    write(temp.path(), "a/extra", b"aa");
    write(temp.path(), "b/extra", b"bb");
    options.minimum_coverage = 76;
    assert!(scan(&options).pairs.is_empty());
    options.minimum_coverage = 75;
    assert_eq!(scan(&options).pairs.first().unwrap().matched_count, 3);
    options.minimum_files = 5;
    assert!(scan(&options).pairs.is_empty());
    options.minimum_files = 4;
    options.minimum_bytes = 9;
    assert!(scan(&options).pairs.is_empty());
    options.minimum_bytes = 8;
    assert_eq!(scan(&options).pairs.len(), 1);
}

#[test]
fn hard_links_count_as_path_occurrences_without_claiming_physical_savings() {
    let (temp, options) = fixtures();
    write(temp.path(), "a/one", b"same");
    fs::hard_link(temp.path().join("a/one"), temp.path().join("a/two")).unwrap();
    fs::hard_link(temp.path().join("a/one"), temp.path().join("b/one")).unwrap();
    let report = scan(&options);
    let pair = report.pairs.first().unwrap();
    assert!(!pair.exact);
    assert_eq!(pair.matched_count, 1);
    assert_eq!(pair.potential_duplicate_bytes, 4);
}
