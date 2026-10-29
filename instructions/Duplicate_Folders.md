# Duplicate Folders in Krokiet

Duplicate Folders is an analysis-only tool. It compares original file bytes and
does not connect to Immich or modify its database. Renamed photographs match;
visually similar photographs with different bytes do not.

## Use on Windows

Install Rust 1.94.1 or newer through rustup and the Visual Studio C++ build tools.
From this fork's checkout:

```powershell
cargo build --locked --release -p krokiet
.\target\release\krokiet.exe
```

Choose **Duplicate Folders** in the tool selector. Add local directories or a UNC
path such as `\\nas\photos\archive` using the existing included-path controls.
Windows must already be able to open that share with your account. Configure
excluded paths and optional extension filters, then click **Scan**. Use **Stop**
to cancel; cancelled scans publish no pairs. No share credentials belong in the
application settings.

The reference checkbox marks a directory to retain. Reference directories remain
visible in comparisons, including when every included directory is a reference.
The new tool has no deletion, trash, move, rename, hard-link or symlink actions.
Backend action handlers also reject this tool, including stale popup callbacks.

**Show differences** loads common path pairs or the files exclusive to either
side. **Open A/B** opens a directory in the system file manager. Path buttons in
the detail list open the corresponding file in its associated application.
**Export JSON** saves the report, absolute paths, per-folder totals, reference and
completeness flags, file hashes, matched counts and bytes, and any limitations.
Folder/file indices in the report refer to its `folders` and `files` arrays;
`file_indices` preserves membership for each compared directory.

## Comparison rules

- With recursion enabled, every discovered directory is a candidate and includes
  all included descendant files, independently of their names or structure.
- Without recursion, only selected roots and their direct files are compared.
- Overlapping selected roots are traversed once in recursive mode. A directory is
  never paired with itself or an ancestor/descendant. Sibling subtrees can match.
- The multiset of `(BLAKE3 hash, byte size)` defines exactness. Repeated identical
  files and hard links count as separate path occurrences. An extra copy prevents
  an exact match even when the set of hashes is the same.
- Empty files participate. Empty directories are omitted. Special files,
  symbolic links and Windows reparse points/junctions are not followed; the
  affected folder and its recursive ancestors are marked incomplete.
- Excluded paths/items and allowed/excluded extensions define the compared view.
  Exactness applies to that view, not excluded data. The global per-file size
  limits do not apply: use the dedicated minimum file count and total byte limits
  for folders instead. These folder limits apply to each side of a pair.
- A partial pair passes the coverage threshold if either directional **file
  count** coverage meets it. For 245 matches in folders with 250 and 245 files,
  the result is 98% and 100%. Both count and byte denominators are displayed.
  **100% identical only** additionally requires equal complete multisets.
- Percentages are truncated to two decimal places. A partial 99.999% match is
  never displayed as 100%; the exact/partial classification uses integer counts.

The displayed potential duplicate bytes are the logical bytes matched in one
side. They are not physically reclaimable space, and values from different pairs
must not be added together: pairs can share directories/files. Hard links,
filesystem deduplication, compression and NAS snapshots affect physical storage.

## Errors, cache and resource limits

Unreadable entries, hashing errors and observed file/directory changes invalidate
the affected folder and recursive ancestors. Incomplete folders cannot produce
exact or partial pairs. Unrelated readable folders can still produce results.
Metadata is checked again after hashing. This is not an atomic filesystem
snapshot: scan an archive that is not being edited and do not use these reports
as authorization for future destructive operations.

Fresh streaming BLAKE3 hashing is the default. **Reuse hashes (trusted timestamps
only)** is an explicit opt-in. The separate versioned cache checks size,
nanosecond modification time and platform file identity; Unix also checks change
time. Corrupt/unsupported caches are ignored with a warning and replaced. Leave
reuse disabled on SMB/NAS storage with uncertain timestamps or identifiers;
metadata-preserving changes cannot always be detected by a metadata cache.
Cache files are excluded from the compared view. Source data is never written or
explicitly retimestamped; ordinary reads remain subject to filesystem access-time
policy. Only the optional hash cache and an explicitly requested export are
written.

The tool reuses the existing streaming duplicate-file hasher and extension/item
filters. It needs its own directory inventory because the existing file-duplicate
pipeline discards singleton sizes and directory completeness information needed
for folder denominators. File-size prefiltering avoids hashing sizes that occur
only once. Recursive directories share file indices instead of copying contents.
An inverted content index accumulates partial matches without comparing every
folder's entire file list against every other folder.

Hashing defaults to two workers. A nonzero thread count in the general settings
overrides that, capped at 32. Use a small count on SMB. The current limits are:
1,000,000 files, 100,000 directories, nesting depth 128, 1,000,000 content-posting
pair visits and 20,000 result pairs. Dense partial searches can reach the pair
visit limit; their incomplete counts are discarded and a warning is shown.
Reaching an inventory limit invalidates that inventory. Reaching the result
limit keeps valid pairs and explicitly reports omitted results. The virtualized
detail list shows at most 20,000 path pairs; its shown/total label makes this
visible, and the JSON report retains the full inventory.

## Development checks

```sh
cargo test --locked -p czkawka_core duplicate_folders
cargo build --locked -p krokiet --no-default-features --features winit_software
cargo test --locked -p krokiet --no-default-features --features winit_software
```

The explicitly ignored GUI integration test requires a desktop display or Xvfb
and a larger test-thread stack for the generated Slint window:

```sh
RUST_MIN_STACK=33554432 xvfb-run -a cargo test --locked -p krokiet \
  --no-default-features --features winit_software \
  folder_scan_updates_real_slint_models_and_blocks_deletion -- --ignored
```

Run `just fix` for the repository quality gate. Its all-feature Clippy pass also
requires the native codec, GTK, audio and renderer build dependencies documented
in the CI workflows.

The reproducible synthetic benchmark creates 200 folders with 20 files each,
arranged as 100 renamed exact pairs, then scans with a cold and warm hash cache:

```sh
cargo run --locked -p czkawka_core --example duplicate_folders_benchmark
```

These measurements use synthetic local files and an operating-system page cache;
they do not predict throughput, file-identifier support or cancellation latency
on a particular SMB server. Actual Windows/UNC/Synology execution and Windows
file-manager associations require validation on that platform. Linux GUI testing
and Windows cross-compilation do not establish SMB runtime compatibility.

In the cloud Linux instance, Rust 1.94.1 with the unoptimized `fastci` profile and
two hashing workers scanned these 4,000 files in 210 ms with a cold hash cache and
160 ms with a warm hash cache. Both runs found exactly 100 exact pairs and made
6,000 content-posting pair visits. The warm run reused all 4,000 hashes. Peak
process RSS was 13,480 KiB (13.2 MiB), measured in a standalone process including
fixture creation and both scans, without counting compilation.

No destructive workflow is implemented. If physical file removals are introduced
later, Immich external libraries may need a rescan; removed entries can enter
Immich's trash and thumbnails may persist temporarily. Neither Windows Recycle
Bin support on SMB nor the Synology recycle-bin configuration may be assumed.
