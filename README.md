# Krokiet Duplicate Folders - Windows x64 test build

Download `krokiet-duplicate-folders-windows-x64.zip`, extract it, and run `krokiet.exe`.
Choose **Duplicate Folders** in the tool selector.

Source commit: [1802e0c2eae072768460a5037b045bb20d0b563f](https://github.com/giuliobranchetti/czkawka/commit/1802e0c2eae072768460a5037b045bb20d0b563f).

This development build uses Rust 1.94.1 and the software renderer. It supports read-only
folder analysis, exact and partial comparisons, file differences, and JSON export.
The ZIP also includes the full implementation patch and usage documentation.

Validation: Windows x64 cross-compilation succeeded, 344 distinct tests passed,
and `just fix` passed. The GUI integration test ran on Linux under Xvfb.
Windows execution and access to a real SMB/NAS share still require runtime verification.

Use `SHA256SUMS.txt` to verify the ZIP checksum.
