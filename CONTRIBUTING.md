# Contributing

Use the pinned toolchain in `rust-toolchain.toml`. Install a C/C++ compiler and CMake; on Windows use Visual Studio Build Tools with the Desktop C++ workload. Python 3 is needed for packaged runtime verification and dependency-license collection.

Keep changes focused on current DMW / MO State and native MOC. Avoid adding alternate backends or historical compatibility modes. Preserve source evidence and data on edits, retries and cancellation. Never commit API keys, private data, databases or build outputs. Keep both language entry points and current API guides consistent.

Before submitting, run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --release --locked`, and `cargo doc --locked --no-deps`. CI also builds release artifacts, runs C/HTTP/SSE/MOC smoke checks and audits dependencies. Tests use local model mocks; no credentials are required.

The release profile and SQLite compile flags are part of the supported build. Do not change them only in CI. Keep Cargo.lock tracked. Windows UPX packing is a post-build step that preserves original files.

To release, update Cargo.toml and Cargo.lock, merge with passing CI, then push a matching `v<VERSION>` tag (for example `v0.1.0`). The workflow checks the tag, builds and verifies three platform archives, includes license texts, creates SHA256SUMS and publishes a GitHub Release. Windows includes a separate verified UPX archive. This does not publish to crates.io. Workflow runs can also be started manually without creating a release.
