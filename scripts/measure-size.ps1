# Non-destructive Windows/MSVC size experiments. Production sources are not edited.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$run = Join-Path $repo ('target\size-audit-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$build = Join-Path $run 'build'
New-Item -ItemType Directory -Path $run | Out-Null
$originalFlags = $env:LIBSQLITE3_FLAGS
$results = @()
try {
    foreach ($case in @('current', 'baseline', 'lto-speed', 'lto-size', 'lean-rustls', 'lean-native', 'sqlite-trim')) {
        $source = Join-Path $run $case
        New-Item -ItemType Directory -Path $source | Out-Null
        foreach ($directory in @('src', 'tests')) {
            Copy-Item -LiteralPath (Join-Path $repo $directory) -Destination $source -Recurse
        }
        foreach ($file in @('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml')) {
            Copy-Item -LiteralPath (Join-Path $repo $file) -Destination $source
        }
        $manifest = Get-Content (Join-Path $source 'Cargo.toml') -Raw
        if ($case -ne 'current') {
            # Historical variants retain their original profile, independently of production.
            $manifest = [regex]::Replace($manifest, '(?ms)^\[profile\.release\]\r?\n.*?(?=^\[|\z)', '')
        }
        if ($case -notin @('current', 'baseline')) {
            $level = if ($case -eq 'lto-speed') { '3' } else { '"s"' }
            $manifest += "`n[profile.release]`nlto = `"thin`"`ncodegen-units = 1`nopt-level = $level`n"
        }
        if ($case -in @('lean-rustls', 'lean-native', 'sqlite-trim')) {
            $tls = if ($case -eq 'lean-rustls') { 'rustls' } else { 'native-tls' }
            $old = 'reqwest = { version = "0.13", features = ["json", "stream"] }'
            if (-not $manifest.Contains($old)) { throw 'reqwest declaration changed; update the experiment.' }
            $new = 'reqwest = { version = "0.13", default-features = false, features = ["json", "stream", "' + $tls + '"] }'
            $manifest = $manifest.Replace($old, $new)
        }
        [IO.File]::WriteAllText((Join-Path $source 'Cargo.toml'), $manifest)
        $env:LIBSQLITE3_FLAGS = if ($case -eq 'sqlite-trim') {
            '-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_FTS3_PARENTHESIS -USQLITE_ENABLE_FTS5 -USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -USQLITE_SOUNDEX'
        } else { $null }
        & cargo build --manifest-path (Join-Path $source 'Cargo.toml') --target-dir $build --release --lib --bin molite-server
        if ($LASTEXITCODE -ne 0) { throw "Build failed: $case" }
        Copy-Item -LiteralPath (Join-Path $build 'release\molite_core.dll') -Destination (Join-Path $run "$case.dll")
        Copy-Item -LiteralPath (Join-Path $build 'release\molite-server.exe') -Destination (Join-Path $run "$case.exe")
        $results += [pscustomobject]@{
            case = $case
            dll_bytes = (Get-Item (Join-Path $run "$case.dll")).Length
            exe_bytes = (Get-Item (Join-Path $run "$case.exe")).Length
            lock_sha256 = (Get-FileHash (Join-Path $source 'Cargo.lock')).Hash
        }
        $results | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $run 'sizes.json') -Encoding utf8
    }
} finally {
    $env:LIBSQLITE3_FLAGS = $originalFlags
}
$results | Format-Table
Write-Output "Results: $run"
