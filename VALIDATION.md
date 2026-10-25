# Validation

Local validation on macOS ARM, 2026-10-25, after pinning SDK commit `9f91afcc071c3f301414dc62b33cfeac7d89a0cf`:

- 11 Rust tests passed; no ignored tests. All original domain regressions retained.
- Locked release build, formatting and strict Clippy passed.
- Packaged executable passed an extracted-ZIP protocol smoke with source absent and developer tools excluded from PATH.
- Original assets and source licenses independently matched the extraction input hashes. Raw upstream snapshots preserve their original whitespace.

Validation used one Cargo build job and sequential crates. Source migration uses manifest `migrationPaths` and scoped SDK storage. Production runtimes use two async worker threads. Tests use local fixtures; they do not mutate real hardware or live provider accounts.

Native CI builds, tests and packages on macOS ARM and Windows x64. Workflow presence alone is not platform validation evidence. Actual host installation and live OS/provider acceptance remain separate gates. The original source and dependency/license provenance is retained in `provenance/` and `NOTICE`.
