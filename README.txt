Krokiet Duplicate Folders - Windows x64 test build with Italian translation

Source commit: f1dab361fbf264b377e5ca0d1868cabd133f9662
https://github.com/giuliobranchetti/czkawka/commit/f1dab361fbf264b377e5ca0d1868cabd133f9662

Extract the ZIP and run krokiet.exe. Choose Italiano in Settings, then select
Cartelle duplicate. Existing Italian language preferences are reused.

The new section includes 56 Italian messages covering settings, results, progress,
warnings and errors. Folder analysis remains read-only.

Validation: Italian Fluent syntax and placeholders, the Italian GUI integration
test under Xvfb, 16 folder scanner tests, just fix, and Windows x64 compilation passed.
Actual Windows and SMB/NAS execution still require runtime verification.

This is an unoptimized fastci development build with the software renderer.
The ZIP includes usage documentation and the complete implementation patch.
