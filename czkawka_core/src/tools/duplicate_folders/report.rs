use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::FolderReport;
use crate::flc;

impl FolderReport {
    /// Atomically export JSON, rejecting any inventoried source-file target.
    pub fn save_json(&self, path: &Path) -> std::io::Result<()> {
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let parent = fs::canonicalize(parent)?;
        let name = path.file_name().ok_or_else(|| std::io::Error::other(flc!("core_df_report_no_name")))?;
        let target = parent.join(name);
        let resolved = if target.exists() { fs::canonicalize(&target)? } else { target.clone() };
        if self.files.iter().any(|file| file.path == resolved) {
            return Err(std::io::Error::other(flc!("core_df_report_source")));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            serde_json::to_writer_pretty(&mut writer, self)?;
            writer.flush()?;
        }
        temporary.persist(target).map_err(|error| error.error)?;
        Ok(())
    }
}
