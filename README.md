# Amphetamine — standalone Rust extension

Independent stdio executable using `tezbar-extension-sdk`; no Tezbar app crate, Tauri, JavaScript runtime, or user-machine Cargo build is needed for a packaged installation. Source manifest identity is `com.tezbar.native.amphetamine`, schema 3, protocol 1.

Five commands preserve timed/unlimited sessions, duration rounding, until-time validation, remaining time, and display-sleep policy. macOS invokes `/usr/bin/caffeinate -i [-d] -w <extension PID>`; Windows owns all `SetThreadExecutionState` calls on one actor thread. An acknowledged **non-durable** `keep-awake` background lease precedes assertion start. It survives view disposal and releases after expiry/end; EOF/shutdown stops the actor and helper. Active assertions are never restored after a restart. The host's generic idle policy can stop an inactive process; the port does not retain it while inactive.

Tests inject an assertion driver and capture real generic host RPCs. They never change the machine's power policy. Live timed/unlimited/crash behavior still needs native platform acceptance.

## Build and test

Cargo.toml pins the standalone SDK to immutable commit `9f91afcc071c3f301414dc62b33cfeac7d89a0cf`. The app source is not a build input. Keep `Cargo.lock` and use one crate at a time:

```sh
cargo test --locked -j1
cargo build --locked -j1 --bin tezbar-amphetamine
./target/debug/tezbar-amphetamine --protocol-info
./target/debug/tezbar-amphetamine --self-test
```

The executable without flags speaks UTF-8 LF JSON-lines on stdin/stdout; logs go to stderr. It emits `hello` first and waits for `initialize` containing protocol/identity, absolute scoped package/data/temp directories, granted generic capabilities and explicit migration paths. Sessions use `session.open`, `session.call` with the original `extension:*` operation names, and `session.dispose`. Call IDs are `h:<UUID>` and `e:<UUID>`; reverse host responses must be serviced while an operation is pending. `shutdown` or stdin EOF triggers lifecycle cleanup. `--protocol-info` and `--self-test` do not access hardware, launch apps, or contact providers.

Integration tests run the real executable through pipes for protocol-info, hello, initialize, open, dispose and EOF. The Google/CleanShot tests also answer real reverse RPCs; Speedtest uses a separate fixture process for subprocess behavior. No tests are ignored to obtain a passing run.

## Scoped data and migration

Storage is `dataDir/amphetamine/storage.json`, using SDK atomic transactional replacement and corruption rejection. Existing private keys win over imported native keys; unknown keys are retained. The host must resolve these **manifest-declared relative legacy sources** into explicit trusted absolute `initialize.migrationPaths` entries:

- `nativeStore`: `native-extensions-v2/amphetamine/storage.json`

Only named grants are read. No app-global config, installed extension directory discovery or credential copying occurs. Migration sources are never modified. `packageDir` is the validated immutable package root; `dataDir` and `tempDir` are extension-private.

## Package releases

`extension.json.artifacts` is empty in source; published catalog metadata supplies verified target downloads. To update the SDK pin, review the immutable revision and run:

```sh
python3 scripts/pin-sdk.py --rev 9f91afcc071c3f301414dc62b33cfeac7d89a0cf
cargo test --locked -j1
```

The script rejects placeholders/non-SHA revisions. Inspect and commit the final dependency and lockfile as part of parent publication. Release packaging and CI reject a local/unpinned SDK dependency. The native CI matrix uses GitHub-hosted `macos-15` ARM64 and `windows-2022` x64 MSVC runners. Each job tests, builds, packages and uploads review artifacts, with no release publishing step.

Build with `--target aarch64-apple-darwin` or `--target x86_64-pc-windows-msvc`, then use:

```sh
python3 scripts/package.py --target aarch64-apple-darwin --binary target/aarch64-apple-darwin/release/tezbar-amphetamine
```

For a review-only local development ZIP, add `--allow-local-sdk`. `python3 scripts/smoke-package.py dist/amphetamine-aarch64-apple-darwin.zip` validates its full inventory and runs the extracted executable offline over real stdio without installing it. CI runs the same check. The ZIP has `extension.json`, `bin/tezbar-amphetamine[.exe]`, `resources/**`, licenses, LICENSE and NOTICE at its root. The embedded `files` map inventories every packaged file except the manifest. Sidecars contain the real ZIP SHA-256 and executable SHA-256; parent supplies the actual release URL and pins those values in the trusted catalog after release. Never publish development ZIPs or invent artifact URLs/digests. End-user installation executes the verified packaged binary; it never runs Cargo or a helper provisioning script.

## Command coverage

| Original command ID | Implemented platform coverage |
|---|---|
| `amp-start` | macOS, Windows |
| `quick-start` | macOS, Windows |
| `amp-end` | macOS, Windows |
| `remaining` | macOS, Windows |
| `amp-duration` | macOS, Windows |

macOS ARM unit/process tests are recorded in the task handoff. Windows code/CI is present but has not been natively executed by this implementation task. Tests do not establish full hardware/provider/host-install acceptance.

## Licensing and provenance

Keep `LICENSE` (original Tezbar Apache-2.0), `licenses/raycast-extensions-MIT.txt`, `NOTICE`, original artwork and author/contributor metadata. `provenance/source.json` describes the input snapshot, not the port's validation status; `provenance/upstream` distinguishes installed metadata from later upstream context. The exact installed upstream commit is unknown. Original icons/resources were copied unchanged; no new trademark rights or universal asset license are asserted. Separate helper/provider notices apply where present.
