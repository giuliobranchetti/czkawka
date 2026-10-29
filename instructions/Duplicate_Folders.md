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

The compact result list separates **Identical folders**, **One folder fully
present in the other**, and **Partial matches**. A fully contained result means
that all file contents from one side occur in the other, not that the paths have
a parent/child relationship. For example, an album with one photo can be fully
present in a 5,607-file archive; this does not make the two folders identical.
The result filter changes the displayed pairs without rescanning. Exact matches
come first, then fully contained pairs, then partial matches. Within each category,
pairs with more shared logical bytes come first. Result counts show visible and
total pairs; category counts describe the full scan.

Selecting a pair immediately displays both folder paths, shared file counts,
coverage and exclusive counts. The first visible pair is selected automatically.
The **Common files / Only in A / Only in B** selector shows the corresponding
files for that pair. Paths in this list are relative to the displayed folder A or B;
buttons still open the original absolute paths. Windows extended path prefixes
are hidden in folder labels. **Open A/B** opens a directory in the system file
manager. File buttons open the corresponding file in its associated application.
**Scan options** expands the coverage threshold, folder size/file limits and
cache setting. The threshold applies to the folder with fewer files; use
**100% identical only** to require complete equality on both sides.
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
  the result is 98% and 100%. Both file-count denominators and folder sizes are displayed.
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
Metadata is checked again after hashing in a separate, cancellable verification
phase, using the same bounded worker pool. On Windows, the inventory reuses
directory-entry metadata instead of requesting the same attributes again.
This is not an atomic filesystem
snapshot: scan an archive that is not being edited and do not use these reports
as authorization for future destructive operations.

Fresh streaming BLAKE3 hashing is the default. Files with repeated sizes first
receive the same 4 KiB head/tail prefilter used by the standard duplicate-file
scanner. Different samples rule out equality; matching samples still require a
full content hash. For files no larger than 8 KiB, the sample already covers the
entire file and its hash is reused. Hash buffers are retained per worker instead
of allocating a new 2 MiB buffer for every file. **Reuse hashes (trusted timestamps
only)** is an explicit opt-in. The separate versioned cache checks size,
nanosecond modification time and platform file identity; Unix also checks change
time. It retains partial signatures as well as available full hashes. Existing
full-hash cache records are migrated by reading only their samples. A cached
partial signature is never treated as evidence of full equality. Cache hits in
the JSON report can include partial signatures; `bytes_read`, `prehashes_computed`
and `hashes_computed` describe the hashing work performed in this scan.
`bytes_read` counts file payload consumed by the hashers, not SMB metadata
requests, protocol overhead or operating-system read-ahead.
Corrupt/unsupported caches are ignored with a warning and replaced. Leave
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
folder's entire file list against every other folder. Sorted subtree intervals
skip ancestor/descendant ranges together, and the comparison index retains counts
without cloning and sorting every file path. Exact-only scans group complete
multiset signatures directly instead of exploring partial-match candidates.

Hashing defaults to two workers. A nonzero thread count in the general settings
overrides that, capped at 32. Use a small count on SMB. The current limits are:
1,000,000 files, 100,000 directories, nesting depth 128, 1,000,000 content-posting
pair visits for partial searches and 20,000 result pairs. Dense partial searches can reach the pair
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

For an I/O-sensitive comparison with the standard file scanner, run:

```sh
cargo run --locked --release -p czkawka_core --example folder_scan_performance -- io
cargo run --locked --release -p czkawka_core --example folder_scan_performance -- nested
cargo run --locked --release -p czkawka_core --example folder_scan_performance -- same-prefix
```

The `io` case creates 96 equal-size 8 MiB files, with eight shared contents
between two folders. Both scanners run without caches and with two workers,
using both execution orders on fresh fixtures. The folder scanner now reads
135,004,160 bytes (128.75 MiB), including samples, instead of fully hashing all
805,306,368 bytes (768 MiB). It fully hashes 16 files instead of 96. The
`same-prefix` case puts differences in the middle of the files: all full hashes
remain necessary, and the scanner must still find only the eight actual matches.
The `nested` case compares 24-level subtrees and checks ancestor exclusion.

On the cloud Linux instance with Rust 1.94.1, two workers and caches disabled:

| Scenario | Previous development build | Previous optimized scanner | Updated optimized scanner |
| --- | --- | --- | --- |
| Equal-size 8 MiB files (`io`) | 179-230 ms | 99-121 ms | 17-19 ms |
| 24-level subtrees (`nested`) | 39 ms | 21 ms | 3 ms |

The ranges reflect the two scanner execution orders, not a statistical latency
guarantee. Sparse synthetic files and the local page cache differ substantially
from real NAS storage. The `same-prefix` control still fully hashes all 96 files
and reports only the eight actual shared contents; that work cannot be eliminated
by sample filtering.

For actual use, distribute a `--release` Windows build. The earlier `fastci`
download was a development build with optimizations disabled and is not a useful
performance baseline for an optimized standard application.

Folder and standard file scans use separate caches. A first folder scan can still
require full reads of genuinely duplicated contents even when the file scanner
has a warm cache. Folder totals also include small and empty files; the standard
scanner's minimum file-size filter can omit these. Compare the same roots,
extension/exclusion filters and cache conditions when assessing performance.

These measurements use synthetic local files and an operating-system page cache;
they do not predict throughput, file-identifier support or cancellation latency
on a particular SMB server. Actual Windows/UNC/Synology execution and Windows
file-manager associations require validation on that platform. Linux GUI testing
and Windows cross-compilation do not establish SMB runtime compatibility.

Overall progress counts the five scan phases: collection, partial hashing, full
hashing, verification, and comparison. The current-phase percentage and the
overall percentage therefore advance independently; they do not estimate network
throughput or remaining time. Phase changes are reported immediately, with
frequent throttled file updates within a phase.

No destructive workflow is implemented. If physical file removals are introduced
later, Immich external libraries may need a rescan; removed entries can enter
Immich's trash and thumbnails may persist temporarily. Neither Windows Recycle
Bin support on SMB nor the Synology recycle-bin configuration may be assumed.
