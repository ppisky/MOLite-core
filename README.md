# MOLite Core

[English](README.md) | [简体中文](README.zh-CN.md)

A small Rust core for character conversations with conversation-scoped DMW v2 / MO State v2 memory. MOLite follows MOMO Core's provider → model → model parameter group → purpose → conversation workflow, with one focused configuration and current-format support.

There are no personal Spaces, character-owned or implicit shared memory pools, NSG, vector databases, foreign character-card formats, image carriers, or LSB carriers. Folders and tags classify resources; explicitly selected conversation scopes control memory injection.

## Quick start

Install the Rust version in `rust-toolchain.toml` and a native C/C++ toolchain (MSVC Build Tools on Windows; a compiler and CMake on Unix).

```sh
cargo run --release --locked --bin molite-server
```

The server listens on `127.0.0.1:3791` and stores data in `data/`. Set `MOLITE_DATA`, `MOLITE_LISTEN`, and optionally `MOLITE_HTTP_TOKEN`. A non-loopback listener requires a token. Create a conversation directly; configure your provider, models, parameter groups and purposes before generating replies. API credentials are read from named environment variables, never exported with MOC.

See the [API guide](docs/api.md), [native MOC format](docs/moc.md), and [C ABI guide](docs/c-abi.md). [Chinese guides](README.zh-CN.md) are also available.

## Behavior

- Three provider protocols: Chat Completions, Responses and Anthropic Messages. Parameter groups choose the request protocol, streaming preference, timeout and native request parameters.
- Text and vision model configuration; direct image input for vision models or image-to-text fallback for text-only models.
- Conversation-scoped DMW evidence, five-dimensional state snapshots and source revisions. Editing, deleting or regenerating an exchange invalidates affected memory and schedules repair.
- Extraction defaults to 12 completed rounds; configure 1–200 rounds or trigger early extraction. Successful early extraction restarts the cadence; repair does not add rounds.
- Four editable prompt spaces with built-in defaults, persistent overrides and reset: assistant, roleplay director, memory distillation and vision fallback.
- Native MOC v3 character import and MOLite state roundtrips, optional data files and password encryption. Only current archive formats are accepted; local databases support the documented one-to-one ownership upgrade.
- Rust API, HTTP/SSE and a standard C ABI. The core produces `rlib`, `cdylib` and `staticlib` artifacts.

## Build and validate

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --release --locked
cargo doc --locked --no-deps
cargo build --release --locked
```

CI builds Linux x64, macOS ARM64 and Windows x64, tests the C host and runtime adapters, and publishes build artifacts. Tags named `v<Cargo version>` publish release packages after all checks pass. See [contributing](CONTRIBUTING.md) for development and release instructions.

Bash entry points (Linux/macOS; Git Bash on Windows):

```bash
bash scripts/build.sh
bash scripts/check.sh
bash scripts/measure-size.sh
bash scripts/package-release.sh linux-x86_64  # use macos-aarch64 or windows-x86_64 on those hosts
```

The scripts use Python 3; set `PYTHON=python` if your executable is named `python`. `package-release.sh` packages an already-built and verified release. On Windows, `bash scripts/pack-upx.sh` invokes the pinned Windows UPX pipeline from Git Bash; native Linux/macOS UPX packaging is not validated. The `.ps1` scripts remain available for PowerShell users.

Release builds use `opt-level="z"` and full LTO, retaining rustls, AWS-LC, HTTP/2 and system proxy support. SQLite is bundled with unused extensions disabled by [.cargo/config.toml](.cargo/config.toml). **When embedding MOLite as a Rust dependency, copy `LIBSQLITE3_FLAGS` to the host workspace configuration:** Cargo does not inherit configuration from dependency directories. An existing environment variable overrides the repository default. SQLite tests verify the actual compiled options.

Only required dependency features are enabled. zstd retains the standard current wire format and fast decode paths, without legacy decoding or dictionary training. tar excludes filesystem extended attributes; model JSON/SSE uses UTF-8. On Windows, `powershell -File scripts/pack-upx.ps1` builds separate copies with UPX `--ultra-brute` and verifies them; original binaries remain in `target/release/`. Packing requires Python 3 and MSVC Build Tools. Sign binaries after packing. See [measurements](docs/size-audit-upx-2026-10-24.json).

## Current boundaries

PDF and arbitrary data files can be stored and carried in MOC; input parsing supports UTF-8 text and PNG/JPEG/WebP/GIF, without PDF text extraction. Data storage is separate from model input.

MOLite runtime data uses an explicit `molite-runtime` MOC extension. Original MOMO character payloads can be imported, but old Space conversations and memory trees are not migrated and are reported as skipped modules. Other hosts need support for the MOLite extension to restore its runtime state.

State projection rules are fixed; editable YAML contracts, DDM personality profiles, generic tools and provider-specific token counting are not implemented. History is limited to 48 rounds. One Engine serializes coordination; use one Engine owner per data directory. Provider tests use local mocks, without paid API calls. CI coverage is not a claim of compatibility with every provider or host.

## License

Apache-2.0. MOLite contains adapted MOMO Core components; attribution is retained in [NOTICE](NOTICE). Release archives include dependency license texts. Source publication does not publish the crate to crates.io.
