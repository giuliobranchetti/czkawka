use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use czkawka_core::tools::duplicate_folders::{FolderScanOptions, scan_folders};

#[expect(clippy::print_stdout, reason = "This example reports reproducible benchmark measurements")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for folder in 0..200 {
        let path = root.path().join(format!("folder-{folder:03}"));
        fs::create_dir(&path)?;
        for file in 0..20 {
            let content = format!("photo:{}:{file}:{}", folder / 2, "x".repeat(4096));
            fs::write(path.join(format!("file-{folder}-{file}")), content)?;
        }
    }
    let options = FolderScanOptions {
        included_paths: vec![root.path().to_path_buf()],
        trust_cache: true,
        cache_path: Some(root.path().join("hashes.json")),
        ..Default::default()
    };
    for run in ["cold", "cached"] {
        let started = Instant::now();
        let report = scan_folders(&options, &Arc::new(AtomicBool::new(false)), |_| {});
        assert_eq!(report.pairs.len(), 100);
        assert!(report.pairs.iter().all(|pair| pair.exact));
        assert!(!report.truncated);
        println!(
            "{run}: folders={} files={} pairs={} hashes={} cache_hits={} candidate_visits={} elapsed_ms={}",
            report.folders.len(),
            report.files.len(),
            report.pairs.len(),
            report.hashes_computed,
            report.cache_hits,
            report.candidate_visits,
            started.elapsed().as_millis()
        );
    }
    Ok(())
}
