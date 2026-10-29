use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use czkawka_core::common::config_cache_path::set_config_cache_path;
use czkawka_core::common::model::{CheckingMethod, HashType};
use czkawka_core::common::threads::set_number_of_threads;
use czkawka_core::common::tool_data::CommonData;
use czkawka_core::common::traits::Search;
use czkawka_core::tools::duplicate::{DuplicateFinder, DuplicateFinderParameters};
use czkawka_core::tools::duplicate_folders::{FolderScanOptions, scan_folders};

fn create_fixture(root: &Path, scenario: &str) -> std::io::Result<u64> {
    let file_size = if matches!(scenario, "small" | "nested") { 4096 } else { 8 * 1024 * 1024 };
    for side in 0..2 {
        let mut path = root.join(format!("side-{side}"));
        if scenario == "nested" {
            for _ in 0..24 {
                path.push("nested");
            }
        }
        fs::create_dir_all(&path)?;
        for index in 0..48_u64 {
            let identity = if index < 8 { index } else { side * 48 + index };
            let mut file = File::create(path.join(format!("photo-{index:03}.jpg")))?;
            file.set_len(file_size)?;
            if scenario == "same-prefix" {
                file.seek(SeekFrom::Start(file_size / 2))?;
            }
            file.write_all(&identity.to_le_bytes())?;
        }
    }
    Ok(file_size * 96)
}

#[expect(clippy::print_stdout, reason = "This example compares real scanner time and I/O counters")]
fn measure(root: &Path, scanner: &str) {
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    if scanner == "folders" {
        let options = FolderScanOptions {
            included_paths: vec![root.to_path_buf()],
            minimum_coverage: 0,
            ..Default::default()
        };
        let report = scan_folders(&options, &stop, |_| {});
        assert!(!report.cancelled && !report.truncated);
        assert!(report.pairs.iter().all(|pair| !pair.exact && pair.matched_count == 8));
        assert!(!report.pairs.is_empty());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let metrics = serde_json::to_value(&report).expect("Fixture paths are valid UTF-8");
        println!(
            "scanner=folders elapsed_ms={} files={} pairs={} full_hashes={} bytes_read={} prehashes={}",
            started.elapsed().as_millis(),
            report.files.len(),
            report.pairs.len(),
            report.hashes_computed,
            metrics.get("bytes_read").map_or_else(|| "unavailable".into(), ToString::to_string),
            metrics.get("prehashes_computed").map_or_else(|| "unavailable".into(), ToString::to_string)
        );
    } else {
        let params = DuplicateFinderParameters::new(CheckingMethod::Hash, HashType::Blake3, false, 0, 0, true);
        let mut finder = DuplicateFinder::new(params);
        finder.set_included_paths(vec![root.to_path_buf()]);
        finder.set_minimal_file_size(0);
        finder.set_recursive_search(true);
        finder.set_use_cache(false);
        finder.set_hide_hard_links(false);
        finder.search(&stop, None);
        let info = finder.get_information();
        assert_eq!(info.number_of_groups_by_hash, 8);
        println!("scanner=files elapsed_ms={} groups={}", started.elapsed().as_millis(), info.number_of_groups_by_hash);
    }
}

#[expect(clippy::print_stdout, reason = "This example reports fixture and scanner parameters")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "io".into());
    assert!(["io", "small", "same-prefix", "nested"].contains(&scenario.as_str()));
    set_config_cache_path("Czkawka", "Czkawka");
    set_number_of_threads(2);
    for order in [["folders", "files"], ["files", "folders"]] {
        let root = tempfile::tempdir()?;
        let logical_bytes = create_fixture(root.path(), &scenario)?;
        println!(
            "scenario={scenario} order={order:?} logical_bytes={logical_bytes} threads=2 cache=false debug_assertions={}",
            cfg!(debug_assertions)
        );
        for scanner in order {
            measure(root.path(), scanner);
        }
    }
    Ok(())
}
