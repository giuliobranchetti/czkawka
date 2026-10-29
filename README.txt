Krokiet Duplicate Folders - optimized Windows x64 release

Source commit: 3f9d6bb875e2bd26fae76990d145cd7377f9ea12
https://github.com/giuliobranchetti/czkawka/commit/3f9d6bb875e2bd26fae76990d145cd7377f9ea12

Stop the previous scan and close the old application. Extract this ZIP and run
its krokiet.exe. Existing language and path preferences are retained.
Select Cartelle duplicate when using Italian.

This is an optimized release build, replacing the previous unoptimized fastci test
build. It samples equal-size files before full hashing, reuses worker buffers,
uses Windows directory-entry metadata, verifies changes in parallel, and avoids
redundant ancestor comparisons. Matching samples still require complete content
verification. Read-only behavior and cache opt-in are preserved.

Current and overall progress now represent five distinct phases, including
partial hashing and final verification. They are not estimates of remaining time.

On the local 96-file fixture, hashing payload falls from 768 MiB to 128.75 MiB.
This does not establish a particular throughput on your SMB/NAS share. Cold folder
scans still require full reads of actual duplicates and use a separate cache from
the standard file scanner.

Validation: full core/Krokiet suites, 23 folder tests, Italian GUI integration,
Fluent validation, just fix and Windows x64 release compilation passed.
Actual Windows/SMB execution remains to be checked on your machine.

Built with Rust 1.94.1, the software renderer and release optimizations.
