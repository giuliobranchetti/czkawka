use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::cache::{Stamp, is_link, regular_metadata, stamp};
use super::{FolderFile, FolderProgress, FolderReport, FolderScanOptions, FolderScanPhase, FolderSummary};
use crate::common::extensions::Extensions;
use crate::common::items::ExcludedItems;
use crate::flc;

pub(super) struct Inventory {
    pub report: FolderReport,
    stamps: BTreeMap<PathBuf, Stamp>,
    recursive: bool,
}

struct Filters {
    paths: Vec<PathBuf>,
    items: ExcludedItems,
    extensions: Extensions,
}

impl Inventory {
    pub fn collect(options: &FolderScanOptions, stop: &Arc<AtomicBool>, progress: &impl Fn(FolderProgress)) -> Self {
        let mut scan = Self {
            report: FolderReport::default(),
            stamps: BTreeMap::new(),
            recursive: options.recursive,
        };
        let filters = scan.filters(options);
        let mut roots = BTreeSet::new();
        for path in &options.included_paths {
            match regular_metadata(path).and_then(|metadata| {
                if !metadata.is_dir() {
                    return Err(flc!("core_df_not_directory", path = path.to_string_lossy()));
                }
                fs::canonicalize(path).map_err(|error| flc!("core_df_resolve", path = path.to_string_lossy(), reason = error.to_string()))
            }) {
                Ok(path) => {
                    roots.insert(path);
                }
                Err(error) => scan.report.warnings.push(error),
            }
        }
        if roots.is_empty() {
            scan.report.warnings.push(flc!("core_df_no_roots"));
        }
        let references: Vec<_> = options.reference_paths.iter().filter_map(|path| fs::canonicalize(path).ok()).collect();
        let mut pending: Vec<_> = roots
            .iter()
            .filter(|path| !options.recursive || !roots.iter().any(|other| *path != other && path.starts_with(other)))
            .map(|path| (path.clone(), None::<usize>, 0usize))
            .collect();
        let mut children = Vec::<Vec<usize>>::new();
        while let Some((path, parent, depth)) = pending.pop() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if filters.excluded(&path) {
                continue;
            }
            let idx = scan.report.folders.len();
            children.push(Vec::new());
            if let Some(list) = parent.and_then(|index| children.get_mut(index)) {
                list.push(idx);
            }
            let reference = references.iter().any(|root| path.starts_with(root));
            scan.report.folders.push(FolderSummary {
                path: path.clone(),
                file_count: 0,
                total_bytes: 0,
                complete: true,
                reference,
                files: Vec::new(),
            });
            if depth > 128 || idx >= 100_000 {
                scan.invalidate(&path);
                scan.report.warnings.push(flc!("core_df_directory_limit", path = path.to_string_lossy()));
                scan.report.truncated = true;
                break;
            }
            scan.read_directory(&path, idx, depth, &filters, &mut pending, stop);
            progress(FolderProgress {
                phase: FolderScanPhase::Collecting,
                completed: idx + 1,
                total: 0,
            });
        }
        for (idx, descendants) in children.into_iter().enumerate().rev() {
            for child in descendants {
                let Some(folder) = scan.report.folders.get(child) else {
                    continue;
                };
                let files = folder.files.clone();
                let complete = folder.complete;
                if let Some(folder) = scan.report.folders.get_mut(idx) {
                    folder.files.extend(files);
                    folder.complete &= complete;
                }
            }
        }
        for folder in &mut scan.report.folders {
            folder.file_count = folder.files.len();
            folder.total_bytes = folder.files.iter().filter_map(|idx| scan.report.files.get(*idx)).map(|file| file.size).sum();
        }
        scan
    }

    fn filters(&mut self, options: &FolderScanOptions) -> Filters {
        let mut extensions = Extensions::new();
        self.report.warnings.extend(extensions.set_allowed_extensions(options.allowed_extensions.clone()).warnings);
        self.report
            .warnings
            .extend(extensions.set_excluded_extensions(options.excluded_extensions.clone()).warnings);
        let mut items = ExcludedItems::new();
        self.report.warnings.extend(items.set_excluded_items(options.excluded_items.clone()).warnings);
        let mut paths: Vec<_> = options.excluded_paths.iter().map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone())).collect();
        if let Some(path) = &options.cache_path {
            paths.push(fs::canonicalize(path).unwrap_or_else(|_| path.clone()));
        }
        Filters { paths, items, extensions }
    }

    fn read_directory(&mut self, path: &Path, idx: usize, depth: usize, filters: &Filters, pending: &mut Vec<(PathBuf, Option<usize>, usize)>, stop: &Arc<AtomicBool>) {
        let result = regular_metadata(path).and_then(|metadata| stamp(path, &metadata));
        match result {
            Ok(value) => {
                self.stamps.insert(path.to_path_buf(), value);
            }
            Err(error) => {
                self.fail(path, error);
                return;
            }
        }
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                self.fail(path, flc!("core_df_list", path = path.to_string_lossy(), reason = error.to_string()));
                return;
            }
        };
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.fail(path, flc!("core_df_entry", path = path.to_string_lossy(), reason = error.to_string()));
                    continue;
                }
            };
            let entry_path = entry.path();
            if filters.excluded(&entry_path) {
                continue;
            }
            let metadata = match fs::symlink_metadata(&entry_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.fail(path, flc!("core_df_inspect", path = entry_path.to_string_lossy(), reason = error.to_string()));
                    continue;
                }
            };
            if is_link(&metadata) {
                self.fail(path, flc!("core_df_link", path = entry_path.to_string_lossy()));
            } else if metadata.is_dir() && self.recursive {
                pending.push((entry_path, Some(idx), depth + 1));
            } else if metadata.is_file() && filters.extensions.check_if_entry_have_valid_extension(&entry.file_name()) {
                if self.report.files.len() >= 1_000_000 {
                    self.report.truncated = true;
                    self.fail(path, flc!("core_df_file_limit"));
                    return;
                }
                match stamp(&entry_path, &metadata) {
                    Ok(value) => {
                        self.stamps.insert(entry_path.clone(), value);
                    }
                    Err(error) => {
                        self.fail(path, error);
                        continue;
                    }
                }
                if let Some(folder) = self.report.folders.get_mut(idx) {
                    folder.files.push(self.report.files.len());
                }
                self.report.files.push(FolderFile {
                    path: entry_path,
                    size: metadata.len(),
                    hash: None,
                });
            } else if !metadata.is_dir() && !metadata.is_file() {
                self.fail(path, flc!("core_df_special", path = entry_path.to_string_lossy()));
            }
        }
    }

    fn fail(&mut self, path: &Path, error: String) {
        self.invalidate(path);
        self.report.warnings.push(error);
    }

    fn invalidate(&mut self, path: &Path) {
        for folder in &mut self.report.folders {
            if path == folder.path || (self.recursive && path.starts_with(&folder.path)) || path.parent() == Some(folder.path.as_path()) {
                folder.complete = false;
            }
        }
    }

    pub fn invalidate_file(&mut self, idx: usize) {
        if let Some(path) = self.report.files.get(idx).map(|file| file.path.clone()) {
            self.invalidate(&path);
        }
    }

    pub fn revalidate(&mut self, _options: &FolderScanOptions) {
        let changed: Vec<_> = self
            .stamps
            .iter()
            .filter_map(|(path, before)| {
                let unchanged = regular_metadata(path).and_then(|metadata| stamp(path, &metadata)).is_ok_and(|after| &after == before);
                if unchanged { None } else { Some(path.clone()) }
            })
            .collect();
        for path in changed {
            self.fail(&path, flc!("core_df_changed", path = path.to_string_lossy()));
        }
        if self.report.truncated {
            for folder in &mut self.report.folders {
                folder.complete = false;
            }
        }
    }
}

impl Filters {
    fn excluded(&self, path: &Path) -> bool {
        self.paths.iter().any(|root| path.starts_with(root)) || self.items.is_excluded(path)
    }
}
