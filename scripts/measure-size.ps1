# Measure the single supported release configuration without altering sources.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$run = Join-Path $repo ('target\size-audit-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $run | Out-Null
Push-Location $repo
try {
    & cargo build --release --locked --lib --bin molite-server
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $results = [ordered]@{
        measured_at = (Get-Date -Format o)
        rustc = (& rustc -V)
        source_commit = (& git rev-parse HEAD)
        working_tree_changes = @(& git status --porcelain)
        manifest_sha256 = (Get-FileHash Cargo.toml).Hash
        lock_sha256 = (Get-FileHash Cargo.lock).Hash
        config_sha256 = (Get-FileHash .cargo/config.toml).Hash
        sqlite_flags_environment_override = $env:LIBSQLITE3_FLAGS
        dll_bytes = (Get-Item target/release/molite_core.dll).Length
        exe_bytes = (Get-Item target/release/molite-server.exe).Length
    }
    Copy-Item -LiteralPath target/release/molite_core.dll -Destination $run
    Copy-Item -LiteralPath target/release/molite-server.exe -Destination $run
    $results | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $run 'sizes.json') -Encoding utf8
    $results
    Write-Output "Results: $run"
} finally {
    Pop-Location
}
