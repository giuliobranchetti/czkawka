use std::cmp::Reverse;
use std::path::Path;

use czkawka_core::tools::duplicate_folders::{FolderPair, FolderReport, FolderSummary};
use humansize::{BINARY, format_size};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{Callabler, FolderPairRow, FolderUi, MainWindow, flk};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchCategory {
    Exact,
    Contained,
    Partial,
}

fn category(report: &FolderReport, pair: &FolderPair) -> MatchCategory {
    if pair.exact {
        MatchCategory::Exact
    } else if pair.matched_count == report.folders[pair.folder_a].file_count || pair.matched_count == report.folders[pair.folder_b].file_count {
        MatchCategory::Contained
    } else {
        MatchCategory::Partial
    }
}

fn result_indices(report: &FolderReport, filter: i32) -> Vec<usize> {
    let selected_category = match filter {
        1 => Some(MatchCategory::Exact),
        2 => Some(MatchCategory::Contained),
        3 => Some(MatchCategory::Partial),
        _ => None,
    };
    let mut indices: Vec<_> = report
        .pairs
        .iter()
        .enumerate()
        .filter(|(_, pair)| selected_category.is_none_or(|selected| category(report, pair) == selected))
        .map(|(idx, _)| idx)
        .collect();
    indices.sort_by_key(|&idx| {
        let pair = &report.pairs[idx];
        (category(report, pair), Reverse(pair.matched_bytes), Reverse(pair.matched_count), idx)
    });
    indices
}

pub(super) fn show_results(app: &MainWindow, report: &FolderReport, filter: i32) {
    let indices = result_indices(report, filter);
    let first = indices.first().copied();
    let rows: Vec<_> = indices.into_iter().map(|idx| pair_row(report, &report.pairs[idx], idx)).collect();
    let ui = app.global::<FolderUi>();
    ui.set_result_filter(filter);
    ui.set_result_count(flk!("folders_result_count", shown = rows.len(), total = report.pairs.len()).into());
    ui.set_selected_pair(-1);
    ui.set_selected(FolderPairRow::default());
    ui.set_details(ModelRc::default());
    ui.set_detail_status("".into());
    ui.set_detail_loading(false);
    ui.set_pairs(ModelRc::new(VecModel::from(rows)));
    if let Some(first) = first {
        app.global::<Callabler>().invoke_folders_show_details(first as i32, ui.get_detail_kind());
    }
}

pub(super) fn category_summary(report: &FolderReport) -> String {
    let mut exact = 0;
    let mut contained = 0;
    let mut partial = 0;
    for pair in &report.pairs {
        match category(report, pair) {
            MatchCategory::Exact => exact += 1,
            MatchCategory::Contained => contained += 1,
            MatchCategory::Partial => partial += 1,
        }
    }
    flk!("folders_categories", exact = exact, contained = contained, partial = partial)
}

pub(super) fn coverage(matched: u64, total: u64) -> String {
    if total == 0 {
        return "0.00".into();
    }
    let hundredths = u128::from(matched) * 10_000 / u128::from(total);
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

fn metrics(folder: &FolderSummary, pair: &FolderPair) -> String {
    flk!(
        "folders_metrics",
        matched = pair.matched_count,
        files = folder.file_count,
        count_percent = coverage(pair.matched_count as u64, folder.file_count as u64),
        bytes = format_size(folder.total_bytes, BINARY),
        exclusive = folder.file_count.saturating_sub(pair.matched_count)
    )
}

pub(super) fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        local.to_string()
    } else {
        text.into_owned()
    }
}

fn folder_label(folder: &FolderSummary) -> String {
    let path = display_path(&folder.path);
    if folder.reference { format!("[{}] {path}", flk!("folders_reference")) } else { path }
}

pub(super) fn pair_row(report: &FolderReport, pair: &FolderPair, idx: usize) -> FolderPairRow {
    let (a, b) = (&report.folders[pair.folder_a], &report.folders[pair.folder_b]);
    let state = match category(report, pair) {
        MatchCategory::Exact => flk!("folders_exact_result"),
        MatchCategory::Contained if pair.matched_count == a.file_count => flk!("folders_contained_a"),
        MatchCategory::Contained => flk!("folders_contained_b"),
        MatchCategory::Partial => flk!("folders_partial_result"),
    };
    let short_name = |side: &str, folder: &FolderSummary| {
        let path = display_path(&folder.path);
        let name = path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(&path);
        format!("{side}: {name}")
    };
    FolderPairRow {
        pair_index: idx as i32,
        state: state.into(),
        name_a: short_name("A", a).into(),
        name_b: short_name("B", b).into(),
        common: flk!("folders_common_count", count = pair.matched_count, bytes = format_size(pair.matched_bytes, BINARY)).into(),
        path_a: folder_label(a).into(),
        path_b: folder_label(b).into(),
        metrics_a: metrics(a, pair).into(),
        metrics_b: metrics(b, pair).into(),
        logical_bytes: flk!("folders_logical_bytes", bytes = format_size(pair.potential_duplicate_bytes, BINARY)).into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use czkawka_core::tools::duplicate_folders::{FolderPair, FolderScanOptions, scan_folders};

    use super::{coverage, display_path, result_indices};

    #[test]
    fn windows_display_paths_preserve_the_share_and_original_paths() {
        let unc = Path::new(r"\\?\UNC\nas\photos\album");
        assert_eq!(display_path(unc), r"\\nas\photos\album");
        assert_eq!(unc.to_str().unwrap(), r"\\?\UNC\nas\photos\album");
        assert_eq!(display_path(Path::new(r"\\?\C:\photos\album")), r"C:\photos\album");
        assert_eq!(display_path(Path::new("/photos/album")), "/photos/album");
        assert_eq!(display_path(Path::new(r"\\nas\photos")), r"\\nas\photos");
    }

    #[test]
    fn partial_percentages_never_round_up_to_exact() {
        assert_eq!(coverage(999_999, 1_000_000), "99.99");
        assert_eq!(coverage(245, 250), "98.00");
        assert_eq!(coverage(245, 245), "100.00");
        assert_eq!(coverage(0, 0), "0.00");
    }

    #[test]
    fn filters_distinguish_subsets_from_exact_matches_and_keep_report_indices() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("folder-presentation-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for name in ["a", "b", "c", "d", "e"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        for name in ["a/one", "b/one", "c/one", "d/one", "e/one"] {
            std::fs::write(root.join(name), b"common").unwrap();
        }
        for name in ["c/two", "d/two", "e/two"] {
            std::fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let options = FolderScanOptions {
            included_paths: vec![root.clone()],
            minimum_coverage: 0,
            ..Default::default()
        };
        let mut report = scan_folders(&options, &Arc::new(AtomicBool::new(false)), |_| {});
        let folder = |name: &str| report.folders.iter().position(|folder| folder.path == root.join(name)).unwrap();
        let (a, b, c, d, e) = (folder("a"), folder("b"), folder("c"), folder("d"), folder("e"));
        let pair = |a, b, exact, matched_bytes| FolderPair {
            folder_a: a,
            folder_b: b,
            matched_count: 1,
            matched_bytes,
            exact,
            potential_duplicate_bytes: matched_bytes,
        };
        report.pairs = vec![pair(c, d, false, 6), pair(a, c, false, 6), pair(a, b, true, 6), pair(d, e, false, 7)];
        assert_eq!(result_indices(&report, 0), vec![2, 1, 3, 0]);
        assert_eq!(result_indices(&report, 1), vec![2]);
        assert_eq!(result_indices(&report, 2), vec![1]);
        assert_eq!(result_indices(&report, 3), vec![3, 0]);
        report.pairs.clear();
        assert!(result_indices(&report, 0).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
