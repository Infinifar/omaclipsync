# Rebuilding the committed native executables

The 0.2.3 release workflow packaged committed files without rebuilding them. Its checksums established asset identity, **not** source-to-binary equivalence. Version 0.2.4 replaces that workflow with a clean source rebuild and byte comparison of **both** executables before packaging.

## Fixed build inputs

- Target: `x86_64-unknown-linux-gnu` (Linux amd64).
- Builder: `mirror.gcr.io/library/rust@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b`.
- Rust/Cargo 1.99.1, Debian GCC 12.2.0 and GNU ld 2.40 are inside that immutable image.
- `core/Cargo.lock`, the committed vendored `rustclipsync-core` 0.3.0 snapshot and release profile in `core/Cargo.toml` are build inputs.
- Source mounts read-only at `/src`; each invocation gets a fresh `/build` directory. `RUSTFLAGS` remaps `/src` to `/omaclipsync`; incremental builds are disabled. `SOURCE_DATE_EPOCH=0`, `TZ=UTC`, `LC_ALL=C`.
- Cargo fetch may access crates.io to populate the cache. Compilation uses `--locked --offline` with container networking disabled. No desktop sockets, credentials, or application state are mounted. The builder runs source-built Rust regression tests, including isolated daemon/CLI fixture tests, before compiling the release outputs. It never executes the committed executables.

The container image and locked dependency sources remain part of the trusted toolchain. Equality does not establish that this toolchain or the source is free of vulnerabilities.

## Independent verification

Requires Git, Docker with access to its daemon, and x86_64 Linux. Start with a clean checkout at the exact review commit; do not enable the plugin to do this:

```sh
git clone https://github.com/Infinifar/omaclipsync.git
cd omaclipsync
git checkout <exact-review-commit>
./scripts/reproducible-build.sh
./scripts/package.sh --use-verified
```

`reproducible-build.sh` rejects a dirty checkout in its default comparison mode. It uses a fresh target directory and `cmp` against `bin/syncclipd` and `bin/syncclipctl`, failing if either differs. `dist/BUILD-EVIDENCE.txt` records the exact source commit, clean-tree status, immutable builder, compiler/linker versions and SHA-256 values. `dist/native/` contains the independent build. `--offline` skips dependency fetching if the Cargo cache is already complete. `NATIVE_CARGO_REGISTRY` selects a registry-only cache (default: `~/.cache/omaclipsync-native/registry`). No Cargo credential/config files are mounted; offline mode mounts the registry cache read-only.

Maintainers intentionally updating the committed binaries use `--update-bundled`, inspect and commit the result, then run default comparison mode from that clean commit. Update mode is preparation, not independent verification.

The GitHub workflow runs on pushes to `main`, manual dispatch, and published releases. GitLab has an equivalent `reproduce` stage. Both release paths must pass the comparison before `package.sh --use-verified` packages the rebuilt files. The archive includes `BUILD-EVIDENCE.txt`; releases also attach that report, both executables, and `SHA256SUMS`. The release tag must match the manifest version.

A generic `./scripts/package.sh` still builds from source using the local compiler; those bytes need not match the fixed toolchain. `--use-bundled` only packages existing files and supplies no source comparison. Neither is the release CI path.

## Review scope

See [NATIVE-REVIEW.md](NATIVE-REVIEW.md) for the substantial native change since the listed marketplace snapshot. Successful reproduction is evidence of provenance, not marketplace approval or an independent security review. Existing release tags and the pending marketplace ticket are preserved.
