use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use czkawka_core::common::config_cache_path::get_config_cache_path;
use czkawka_core::tools::duplicate_folders::{FolderProgress, FolderScanOptions, FolderScanPhase, scan_folders};
use log::{debug, error};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use self::presentation::{category_summary, display_path, pair_row, show_results};
use crate::common::check_if_there_are_any_included_folders;
use crate::connect_rfd::{hide_file_dialog_overlay, show_file_dialog_overlay};
use crate::settings::collect_settings;
use crate::shared_models::SharedModels;
use crate::{Callabler, FolderDetailRow, FolderPairRow, FolderUi, GuiState, MainWindow, ProgressToSend, Settings, flk};

mod presentation;

const DETAIL_LIMIT: usize = 20_000;

#[cfg(test)]
mod gui_test;

pub(crate) fn start_scan(app: &MainWindow, stop: Arc<AtomicBool>, shared: Arc<Mutex<SharedModels>>) {
    if !check_if_there_are_any_included_folders(app) {
        app.invoke_scan_ended(flk!("rust_no_included_paths").into());
        return;
    }
    let settings = app.global::<Settings>();
    if !settings.get_folders_minimum_coverage().parse::<u8>().is_ok_and(|value| value <= 100)
        || settings.get_folders_minimum_files().parse::<usize>().is_err()
        || settings.get_folders_minimum_bytes().parse::<u64>().is_err()
    {
        app.invoke_scan_ended(flk!("folders_invalid_input").into());
        return;
    }
    let custom = collect_settings(app);
    let options = FolderScanOptions {
        included_paths: custom.included_paths,
        excluded_paths: custom.excluded_paths,
        reference_paths: custom.included_paths_referenced,
        excluded_items: custom.excluded_items.split(',').map(str::to_string).collect(),
        allowed_extensions: custom.allowed_extensions.split(',').map(str::to_string).collect(),
        excluded_extensions: custom.excluded_extensions.split(',').map(str::to_string).collect(),
        recursive: custom.recursive_search,
        minimum_files: custom.folders_minimum_files,
        minimum_bytes: custom.folders_minimum_bytes,
        minimum_coverage: custom.folders_minimum_coverage,
        exact_only: custom.folders_exact_only,
        threads: if custom.thread_number <= 0 { 2 } else { custom.thread_number as usize },
        trust_cache: custom.folders_trust_cache,
        cache_path: get_config_cache_path().map(|paths| paths.cache_folder.join("folder_content_hashes_v1.json")),
        ..Default::default()
    };
    stop.store(false, Ordering::Relaxed);
    {
        let mut models = shared.lock().expect("Folder scan state mutex poisoned");
        models.duplicate_folders_generation += 1;
        models.duplicate_folders_report = None;
    }
    let ui = app.global::<FolderUi>();
    ui.set_pairs(ModelRc::default());
    ui.set_details(ModelRc::default());
    ui.set_selected_pair(-1);
    ui.set_selected(FolderPairRow::default());
    ui.set_has_report(false);
    ui.set_result_count("".into());
    ui.set_result_filter(0);
    ui.set_detail_kind(0);
    ui.set_detail_loading(false);
    ui.set_detail_status("".into());
    ui.set_warnings("".into());
    ui.set_working(true);
    ui.set_status(flk!("folders_collecting").into());
    app.set_progress_datas(ProgressToSend {
        all_progress: 0,
        current_progress: -1,
        current_progress_size: -1,
        step_name: flk!("folders_collecting").into(),
    });
    app.set_scanning(true);
    app.global::<GuiState>().set_info_text("".into());
    let weak = app.as_weak();
    thread::spawn(move || {
        let started = Instant::now();
        let last = Mutex::new((Instant::now(), FolderScanPhase::Collecting, 0));
        let report = scan_folders(&options, &stop, |progress| {
            let mut last = last.lock().expect("Folder progress clock mutex poisoned");
            if last.1 == progress.phase && (progress.completed < last.2 || last.0.elapsed() < Duration::from_millis(100)) {
                return;
            }
            *last = (Instant::now(), progress.phase, progress.completed);
            let data = progress_data(progress);
            if let Err(error) = weak.upgrade_in_event_loop(move |app| {
                app.set_progress_datas(data);
            }) {
                debug!("Folder progress receiver closed: {error}");
            }
        });
        let report = Arc::new(report);
        shared.lock().expect("Folder scan state mutex poisoned").duplicate_folders_report = Some(Arc::clone(&report));
        let seconds = format!("{:.2}", started.elapsed().as_secs_f64());
        if let Err(error) = weak.upgrade_in_event_loop(move |app| {
            let state = if report.cancelled {
                flk!("folders_cancelled")
            } else if report.truncated {
                flk!("folders_truncated")
            } else {
                flk!("folders_complete")
            };
            let summary = flk!(
                "folders_summary",
                pairs = report.pairs.len(),
                folders = report.folders.len(),
                incomplete = report.folders.iter().filter(|folder| !folder.complete).count(),
                warnings = report.warnings.len(),
                seconds = seconds,
                state = state
            );
            let ui = app.global::<FolderUi>();
            ui.set_warnings(report.warnings.iter().take(200).cloned().collect::<Vec<_>>().join("\n").into());
            ui.set_status(if report.cancelled { flk!("folders_cancelled") } else { category_summary(&report) }.into());
            ui.set_has_report(true);
            ui.set_working(false);
            app.global::<GuiState>().set_info_text(report.warnings.join("\n").into());
            app.invoke_scan_ended(summary.into());
            show_results(&app, &report, 0);
        }) {
            debug!("Folder result receiver closed: {error}");
        }
    });
}

fn progress_data(progress: FolderProgress) -> ProgressToSend {
    let (stage, name) = match progress.phase {
        FolderScanPhase::Collecting => (0, flk!("folders_collecting")),
        FolderScanPhase::PreHashing => (1, flk!("folders_prehashing")),
        FolderScanPhase::Hashing => (2, flk!("folders_hashing")),
        FolderScanPhase::Verifying => (3, flk!("folders_verifying")),
        FolderScanPhase::Comparing => (4, flk!("folders_comparing")),
    };
    let percent = if progress.total == 0 {
        -1
    } else {
        ((progress.completed as u128 * 100) / progress.total as u128).min(100) as i32
    };
    ProgressToSend {
        all_progress: stage * 20 + percent.max(0) / 5,
        current_progress: percent,
        current_progress_size: -1,
        step_name: name.into(),
    }
}

pub(crate) fn connect(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    connect_details(app, shared);
    connect_filter(app, shared);
    connect_open(app, shared);
    connect_export(app, shared);
}

fn connect_filter(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    let weak = app.as_weak();
    let shared = Arc::clone(shared);
    app.global::<Callabler>().on_folders_filter_results(move |filter| {
        let app = weak.upgrade().expect("Folder filter callback requires a live window");
        if app.get_scanning() {
            return;
        }
        let report = shared.lock().expect("Folder scan state mutex poisoned").duplicate_folders_report.clone();
        if let Some(report) = report {
            show_results(&app, &report, filter);
        }
    });
}

fn connect_details(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    let weak = app.as_weak();
    let shared = Arc::clone(shared);
    app.global::<Callabler>().on_folders_show_details(move |idx, kind| {
        let app = weak.upgrade().expect("Folder details callback requires a live window");
        if app.get_scanning() {
            return;
        }
        let models = shared.lock().expect("Folder scan state mutex poisoned");
        let Some(report) = models.duplicate_folders_report.clone() else {
            return;
        };
        let generation = models.duplicate_folders_generation;
        drop(models);
        let pair = report.pairs.get(idx as usize).expect("Folder pair index must come from the current result model").clone();
        let weak = weak.clone();
        let shared = Arc::clone(&shared);
        let ui = app.global::<FolderUi>();
        ui.set_selected_pair(idx);
        ui.set_selected(pair_row(&report, &pair, idx as usize));
        ui.set_detail_kind(kind);
        ui.set_details(ModelRc::default());
        ui.set_detail_status("".into());
        ui.set_detail_loading(true);
        thread::spawn(move || {
            let diff = report.differences(&pair);
            let paths: Vec<_> = match kind {
                1 => diff.only_a.into_iter().map(|path| (path, PathBuf::new())).collect(),
                2 => diff.only_b.into_iter().map(|path| (PathBuf::new(), path)).collect(),
                _ => diff.common,
            };
            let total = paths.len();
            let paths: Vec<_> = paths.into_iter().take(DETAIL_LIMIT).collect();
            if let Err(error) = weak.upgrade_in_event_loop(move |app| {
                if shared.lock().expect("Folder scan state mutex poisoned").duplicate_folders_generation != generation {
                    return;
                }
                let ui = app.global::<FolderUi>();
                if ui.get_selected_pair() != idx || ui.get_detail_kind() != kind {
                    return;
                }
                let relative_path = |path: &std::path::Path, folder_idx: usize, side: &str| {
                    if path.as_os_str().is_empty() {
                        String::new()
                    } else {
                        format!("{side}: {}", display_path(path.strip_prefix(&report.folders[folder_idx].path).unwrap_or(path)))
                    }
                };
                let rows = paths
                    .into_iter()
                    .map(|(a, b)| FolderDetailRow {
                        display_a: relative_path(&a, pair.folder_a, "A").into(),
                        display_b: relative_path(&b, pair.folder_b, "B").into(),
                        path_a: a.to_string_lossy().as_ref().into(),
                        path_b: b.to_string_lossy().as_ref().into(),
                    })
                    .collect::<Vec<_>>();
                let ui = app.global::<FolderUi>();
                ui.set_detail_status(flk!("folders_detail_count", shown = rows.len(), total = total).into());
                ui.set_details(ModelRc::new(VecModel::from(rows)));
                ui.set_detail_loading(false);
            }) {
                debug!("Folder details receiver closed: {error}");
            }
        });
    });
}

fn connect_open(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    let weak = app.as_weak();
    let shared = Arc::clone(shared);
    app.global::<Callabler>().on_folders_open_folder(move |idx, side_b| {
        let models = shared.lock().expect("Folder scan state mutex poisoned");
        let Some(report) = &models.duplicate_folders_report else {
            return;
        };
        let pair = report.pairs.get(idx as usize).expect("Folder open index must come from the current result model");
        let index = if side_b { pair.folder_b } else { pair.folder_a };
        let path = report.folders.get(index).expect("Folder pair refers to an inventoried directory").path.clone();
        drop(models);
        open_path(path, weak.clone());
    });
    let weak = app.as_weak();
    app.global::<Callabler>()
        .on_folders_open_file(move |path| open_path(PathBuf::from(path.as_str()), weak.clone()));
}

fn open_path(path: PathBuf, weak: Weak<MainWindow>) {
    thread::spawn(move || {
        if let Err(error) = open::that(path) {
            let message = flk!("folders_open_error", error = error.to_string());
            if let Err(error) = weak.upgrade_in_event_loop(move |app| app.invoke_show_error_popup(message.into())) {
                debug!("Folder open receiver closed: {error}");
            }
        }
    });
}

fn connect_export(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    let weak = app.as_weak();
    let shared = Arc::clone(shared);
    app.global::<Callabler>().on_folders_export(move || {
        let app = weak.upgrade().expect("Folder export callback requires a live window");
        if app.get_scanning() {
            return;
        }
        let Some(report) = shared.lock().expect("Folder scan state mutex poisoned").duplicate_folders_report.clone() else {
            app.invoke_show_error_popup(flk!("folders_no_report").into());
            return;
        };
        show_file_dialog_overlay(&app);
        let weak = weak.clone();
        thread::spawn(move || {
            let path = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name("duplicate_folders.json").save_file();
            hide_file_dialog_overlay(&weak);
            let Some(path) = path else {
                return;
            };
            let result = report.save_json(&path);
            let message = match result {
                Ok(()) => flk!("folders_export_saved", path = path.to_string_lossy()),
                Err(error) => {
                    error!("Folder report export failed: {error}");
                    flk!("folders_export_error", error = error.to_string())
                }
            };
            if let Err(error) = weak.upgrade_in_event_loop(move |app| app.set_text_summary_text(message.into())) {
                debug!("Folder export receiver closed: {error}");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use czkawka_core::tools::duplicate_folders::{FolderProgress, FolderScanPhase};

    use super::progress_data;

    #[test]
    fn overall_progress_accounts_for_completed_stages() {
        let progress = progress_data(FolderProgress {
            phase: FolderScanPhase::Hashing,
            completed: 2,
            total: 100,
        });
        assert_eq!(progress.current_progress, 2);
        assert_eq!(progress.all_progress, 40);
        let progress = progress_data(FolderProgress {
            phase: FolderScanPhase::Comparing,
            completed: 100,
            total: 100,
        });
        assert_eq!(progress.all_progress, 100);
        let progress = progress_data(FolderProgress {
            phase: FolderScanPhase::Collecting,
            completed: 10,
            total: 0,
        });
        assert_eq!(progress.current_progress, -1);
        assert_eq!(progress.all_progress, 0);
    }
}
