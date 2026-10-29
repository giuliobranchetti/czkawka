use std::cell::Cell;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, Timer, TimerMode};

use super::{connect, start_scan};
use crate::settings::model::{BasicSettings, SettingsCustom};
use crate::shared_models::SharedModels;
use crate::{ActiveTab, Callabler, FolderUi, GuiState, MainWindow, Settings};

#[test]
#[ignore = "Requires a display; run explicitly under Xvfb or a desktop session"]
fn folder_scan_updates_real_slint_models_and_blocks_deletion() {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("krokiet-folder-ui-{}-{stamp}", std::process::id()));
    for dir in ["a", "b"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for name in ["a/one.jpg", "b/renamed.jpg"] {
        fs::write(root.join(name), b"first photo").unwrap();
    }
    for name in ["a/two.jpg", "b/other.jpg"] {
        fs::write(root.join(name), b"second photo").unwrap();
    }
    fs::write(root.join("a/extra.jpg"), b"exclusive photo").unwrap();
    let app = MainWindow::new().unwrap();
    crate::set_initial_gui_info::set_initial_gui_infos(&app);
    let custom = SettingsCustom {
        included_paths: vec![root.join("a"), root.join("b")],
        included_paths_referenced: vec![root.join("a")],
        excluded_paths: Vec::new(),
        excluded_items: String::new(),
        folders_minimum_coverage: 90,
        ..Default::default()
    };
    crate::settings::set_initial_settings_to_gui(&app, &BasicSettings::default(), &custom, None, 0);
    crate::connect_translation::connect_translations(&app);
    app.global::<GuiState>().set_active_tab(ActiveTab::DuplicateFolders);
    let shared = SharedModels::new_shared();
    connect(&app, &shared);
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, _receiver) = crossbeam_channel::unbounded();
    crate::file_actions::connect_delete::connect_delete_button(&app, sender.clone(), Arc::clone(&stop));
    crate::file_actions::connect_delete::connect_trash_button(&app, sender, Arc::clone(&stop));
    czkawka_core::common::config_cache_path::set_config_cache_path("Krokiet", "Krokiet");
    app.window().set_size(slint::LogicalSize::new(1400., 1000.));
    start_scan(&app, stop, shared);
    let phase = Rc::new(Cell::new(0));
    let phase_copy = Rc::clone(&phase);
    let weak = app.as_weak();
    let started = Instant::now();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let app = weak.upgrade().unwrap();
        let ui = app.global::<FolderUi>();
        if started.elapsed() > Duration::from_secs(20) {
            slint::quit_event_loop().unwrap();
            return;
        }
        match phase_copy.get() {
            0 if !app.get_scanning() => {
                assert_eq!(ui.get_pairs().row_count(), 1);
                let row = ui.get_pairs().row_data(0).unwrap();
                let metrics_a = row.metrics_a.replace(['\u{2068}', '\u{2069}'], "");
                assert!(metrics_a.contains("2 / 2") || metrics_a.contains("2 / 3"), "{metrics_a:?}");
                assert!(row.metrics_a.contains("100.00") || row.metrics_b.contains("100.00"));
                assert!(row.metrics_a.contains("66.66") || row.metrics_b.contains("66.66"));
                assert!(row.path_a.contains("Reference") || row.path_b.contains("Reference"));
                ui.set_selected_pair(0);
                app.global::<Callabler>().invoke_folders_show_details(0, 0);
                phase_copy.set(1);
            }
            1 if !ui.get_detail_loading() => {
                assert_eq!(ui.get_details().row_count(), 2);
                app.global::<Callabler>().invoke_delete_selected_items();
                app.global::<Callabler>().invoke_trash_selected_items();
                let collected = crate::settings::collect_settings(&app);
                assert_eq!(collected.folders_minimum_coverage, 90);
                assert_eq!(collected.folders_minimum_files, 1);
                assert!(!collected.folders_trust_cache);
                phase_copy.set(2);
            }
            2 => {
                if let Ok(path) = std::env::var("CZKAWKA_FOLDER_SCREENSHOT") {
                    let snapshot = app.window().take_snapshot().unwrap();
                    image::save_buffer(path, snapshot.as_bytes(), snapshot.width(), snapshot.height(), image::ColorType::Rgba8).unwrap();
                }
                phase_copy.set(3);
                slint::quit_event_loop().unwrap();
            }
            _ => {}
        }
    });
    app.run().unwrap();
    assert_eq!(phase.get(), 3, "Folder UI scan timed out");
    assert_eq!(fs::read(root.join("a/one.jpg")).unwrap(), b"first photo");
    assert_eq!(fs::read_dir(root.join("a")).unwrap().count(), 3);
    assert_eq!(fs::read_dir(root.join("b")).unwrap().count(), 2);
    let serialized = serde_json::to_string(&crate::settings::collect_settings(&app)).unwrap();
    let restored: SettingsCustom = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored.folders_minimum_coverage, 90);
    app.global::<Settings>().set_folders_minimum_coverage("101".into());
    fs::remove_dir_all(root).unwrap();
}
