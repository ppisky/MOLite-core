# Native MOC format

MOLite uses current MOC v3: tar + zstd, `manifest.toml`, declared modules, path and SHA-256 validation, safe extraction and optional AES-256-GCM / Argon2id encryption. It does not accept image/LSB carriers or foreign character-card formats.

Characters live at `characters/spaces/00000000-0000-0000-0000-000000000001/<uuid>/character.toml` with Markdown payloads. This UUID is a wire-format namespace, not a personal Space or memory pool. An archive can carry multiple characters.

Complete MOLite state uses the explicit module `id=molite-runtime`, `path=molite`, `dependencies=[characters]`, `import_order=60`. `molite/state.json` has `version=2`, a `files_included` flag and an allowlist of resource, conversation, exchange, DMW, provenance, counter and batch tables. Attachments are stored as `molite/files/<sha256>`. Provider/model configuration, prompt overrides, credentials and local request caches are excluded.

Import validates a staging database before committing to the destination transaction. Identity conflicts reject the entire import; local sequence numbers are reassigned. `include_files=false` retains metadata and references and reports `files_included:false`; reads fail explicitly until missing attachments are restored. Content files are written before database commit: rollback may leave unreferenced content, since the filesystem and SQLite are not a single physical transaction.

Password archives retain `momo-envelope-v1`, `private/payload.enc` and the `momo-private-moc-v1` associated data. Extraction is limited to 10,000 entries and 512 MiB unpacked data; password export limits the inner archive to 512 MiB. Passwords are not saved.

Only current format versions are accepted. Original MOMO MOC characters can be imported; old Space conversation/DMW trees are not automatically migrated and are listed in `skipped_modules`. NSG is never activated. Other MOMO hosts must understand `molite-runtime` to restore MOLite state; complete bidirectional business compatibility is not claimed.
