use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{fs, thread};

use czkawka_core::common::remove_single_folder;
use czkawka_core::tools::duplicate_folders::{FolderReport, FolderSummary};
use log::debug;
use slint::ComponentHandle;

use super::presentation::{display_path, show_results};
use crate::shared_models::SharedModels;
use crate::{Callabler, FolderUi, MainWindow, flk};

pub(super) fn prefer_b(a: &FolderSummary, b: &FolderSummary) -> bool {
    let rank = |folder: &FolderSummary| {
        (folder.file_count, folder.path.file_name().unwrap_or_default().to_string_lossy().chars().count())
    };
    rank(b) > rank(a)
}

pub(super) fn connect(app: &MainWindow, shared: &Arc<Mutex<SharedModels>>) {
    let weak = app.as_weak();
    let shared = Arc::clone(shared);
    app.global::<Callabler>().on_folders_remove_other(move || {
        let app = weak.upgrade().expect("Folder removal callback requires a live window");
        let ui = app.global::<FolderUi>();
        if app.get_scanning() || ui.get_working() {
            return;
        }
        let models = shared.lock().expect("Folder scan state mutex poisoned");
        let Some(report) = models.duplicate_folders_report.clone() else {
            return;
        };
        let Some(pair) = report.pairs.get(ui.get_selected_pair() as usize) else {
            return;
        };
        let (keep, remove) = if ui.get_keep_b() { (pair.folder_b, pair.folder_a) } else { (pair.folder_a, pair.folder_b) };
        let keep = report.folders[keep].path.clone();
        let removed = &report.folders[remove];
        if removed.reference || report.folders.iter().any(|folder| folder.reference && folder.path.starts_with(&removed.path)) {
            app.invoke_show_error_popup(flk!("folders_remove_reference").into());
            return;
        }
        let remove = removed.path.clone();
        drop(models);
        let previous_status = ui.get_status();
        ui.set_status(flk!("folders_remove_busy").into());
        ui.set_working(true);
        let weak = weak.clone();
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let result = remove_with_confirmation(&keep, &remove);
            if let Err(error) = weak.upgrade_in_event_loop(move |app| {
                app.global::<FolderUi>().set_working(false);
                app.global::<FolderUi>().set_status(previous_status);
                match result {
                    Ok(true) => {
                        // Invalidate every pair: other results may reference the removed tree.
                        let mut models = shared.lock().expect("Folder scan state mutex poisoned");
                        models.duplicate_folders_generation += 1;
                        models.duplicate_folders_report = None;
                        drop(models);
                        show_results(&app, &FolderReport::default(), 0);
                        app.global::<FolderUi>().set_has_report(false);
                        app.global::<FolderUi>().set_status(flk!("folders_remove_done").into());
                    }
                    Ok(false) => (),
                    Err(error) => app.invoke_show_error_popup(error.into()),
                }
            }) {
                debug!("Folder removal receiver closed: {error}");
            }
        });
    });
}

fn remove_with_confirmation(keep_path: &Path, remove_path: &Path) -> Result<bool, String> {
    let (keep, remove) = (keep_path, remove_path);
    let keep = fs::canonicalize(keep).map_err(|error| format!("{}: {error}", display_path(keep)))?;
    let remove = fs::canonicalize(remove).map_err(|error| format!("{}: {error}", display_path(remove)))?;
    if keep.starts_with(&remove) || remove.starts_with(&keep) {
        return Err(flk!("folders_remove_nested"));
    }
    if keep.as_path() != keep_path || remove.as_path() != remove_path {
        return Err(flk!("folders_remove_changed"));
    }
    let before = (inventory(&keep)?, inventory(&remove)?);
    let exclusive = exclusive_count(&before.0, &before.1);
    let description = flk!("folders_remove_confirm", keep = display_path(&keep), remove = display_path(&remove),
        exclusive = exclusive, files = before.1.values().flatten().count(), kept = before.0.values().flatten().count());
    let confirmed = rfd::MessageDialog::new()
        .set_title(flk!("folders_remove_other"))
        .set_description(description)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancel)
        .show();
    if confirmed != rfd::MessageDialogResult::Ok {
        return Ok(false);
    }
    let after = (inventory(&keep)?, inventory(&remove)?);
    if before != after {
        return Err(flk!("folders_remove_changed"));
    }
    remove_single_folder(remove.to_str().ok_or_else(|| flk!("folders_remove_incomplete"))?, true)?;
    Ok(true)
}

fn exclusive_count(keep: &Inventory, remove: &Inventory) -> usize {
    let mut kept = BTreeMap::new();
    for value in keep.values().flatten() {
        *kept.entry(*value).or_insert(0usize) += 1;
    }
    let mut exclusive = 0;
    for value in remove.values().flatten() {
        match kept.get_mut(value) {
            Some(count) if *count > 0 => *count -= 1,
            _ => exclusive += 1,
        }
    }
    exclusive
}

type Inventory = BTreeMap<PathBuf, Option<(u64, [u8; 32])>>;

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn inventory(root: &Path) -> Result<Inventory, String> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut entries = Inventory::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((path, depth)) = pending.pop() {
        if depth > 128 || entries.len() >= 1_000_000 {
            return Err(flk!("folders_remove_incomplete"));
        }
        let contextual = |error: std::io::Error| format!("{}: {error}", display_path(&path));
        let metadata = fs::symlink_metadata(&path).map_err(contextual)?;
        if is_link(&metadata) {
            return Err(flk!("folders_remove_incomplete"));
        }
        if metadata.is_dir() {
            entries.insert(path.clone(), None);
            for entry in fs::read_dir(&path).map_err(contextual)? {
                pending.push((entry.map_err(contextual)?.path(), depth + 1));
            }
        } else if metadata.is_file() {
            let mut file = fs::File::open(&path).map_err(contextual)?;
            let mut hasher = blake3::Hasher::new();
            let mut size = 0;
            loop {
                let read = file.read(&mut buffer).map_err(contextual)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                size += read as u64;
            }
            let after = fs::symlink_metadata(&path).map_err(contextual)?;
            if is_link(&after) || !after.is_file() || size != metadata.len() || after.len() != metadata.len()
                || after.modified().map_err(contextual)? != metadata.modified().map_err(contextual)? {
                return Err(flk!("folders_remove_changed"));
            }
            entries.insert(path, Some((size, *hasher.finalize().as_bytes())));
        } else {
            return Err(flk!("folders_remove_incomplete"));
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{Inventory, exclusive_count, inventory, prefer_b};
    use std::fs;
    use std::path::PathBuf;
    use czkawka_core::tools::duplicate_folders::FolderSummary;

    fn folder(path: &str, file_count: usize) -> FolderSummary {
        serde_json::from_value(serde_json::json!({
            "path": path, "file_count": file_count, "total_bytes": 0,
            "complete": true, "reference": false, "file_indices": []
        })).unwrap()
    }

    #[test]
    fn chooses_file_count_before_name_length() {
        assert!(!prefer_b(&folder("short", 3), &folder("longer-name", 2)));
        assert!(prefer_b(&folder("short", 2), &folder("longer-name", 2)));
        assert!(!prefer_b(&folder("abc", 2), &folder("xyz", 2)));
    }

    #[test]
    fn counts_extra_copies_as_exclusive() {
        let content = Some((12, [1; 32]));
        let keep = Inventory::from([(PathBuf::from("a/one"), content)]);
        let remove = Inventory::from([
            (PathBuf::from("b/renamed"), content),
            (PathBuf::from("b/extra-copy"), content),
            (PathBuf::from("b/exclusive"), Some((12, [2; 32]))),
            (PathBuf::from("b/empty"), None),
        ]);
        assert_eq!(exclusive_count(&keep, &remove), 2);
        assert_eq!(exclusive_count(&remove, &keep), 0);
    }

    #[test]
    fn inventory_includes_hidden_files_and_empty_subfolders_and_detects_edits() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("folder-removal-{}-{stamp}", std::process::id()));
        fs::create_dir_all(root.join("nested/empty")).unwrap();
        fs::write(root.join("nested/.hidden"), b"original").unwrap();
        let before = inventory(&root).unwrap();
        assert_eq!(before.len(), 4);
        assert_eq!(before.values().flatten().count(), 1);
        fs::write(root.join("nested/.hidden"), b"modified").unwrap();
        assert_ne!(before, inventory(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inventory_rejects_symlinks() {
        let root = std::env::temp_dir().join(format!("folder-removal-link-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        assert!(inventory(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

}
