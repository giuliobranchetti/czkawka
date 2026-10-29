Krokiet Duplicate Folders - clearer comparison view - Windows x64 release

Source commit: 59a5819a1ac42739221add86f737c1be65e191fe
https://github.com/giuliobranchetti/czkawka/commit/59a5819a1ac42739221add86f737c1be65e191fe

Close the old application, extract this ZIP and run its krokiet.exe.
Existing language and included/excluded path preferences are retained.
Select Cartelle duplicate when using Italian.

The left panel lists pairs. Select one to see its comparison and files on the
right. The first visible pair is selected automatically. The result filter
shows all matches, identical folders, fully contained contents or partial
matches without starting a new scan. Exact matches are listed first, then
contained pairs, then partial pairs, with the largest shared contents first
within each category.

A result such as "Tutti i file di B sono presenti in A" means all contents from
B occur in A. It does not mean the two folders are identical: A may contain
many additional files. Both sides show shared counts, percentages and exclusive
file counts. Use File comuni / Solo in A / Solo in B to inspect the selected
pair's files. A and B identify the displayed paths; file rows use paths relative
to those folders. Open buttons still use the original absolute paths.

Opzioni di scansione expands the coverage threshold, folder file/byte limits
and optional cache. Coverage is measured against the folder with fewer files;
use Solo identiche al 100% to require complete equality on both sides.
The tool does not delete or move files. Common logical bytes are not an estimate
of physically recoverable space. Full comparison rules are in Duplicate_Folders.md.

This is an optimized release build with the previous NAS I/O improvements.
Validation: Krokiet tests, Italian GUI integration, translation validation,
just fix and Windows x64 release compilation. Runtime checks on Windows/SMB
still require that platform.
