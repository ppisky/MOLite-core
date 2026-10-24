# Windows x64: build, maximum UPX search, integrity checks and actual host tests.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    $version = '5.2.1'
    $archive = Join-Path $repo "target/tools/upx-$version-win64.zip"
    $upx = Join-Path $repo "target/tools/upx-$version-win64/upx.exe"
    $expectedHash = 'EABC6792A347D45E945BE7748423E7868FD01B0D2BCAA2F4B1031FD71FF69BDA'
    New-Item -ItemType Directory -Force target/tools | Out-Null
    if (-not (Test-Path -LiteralPath $archive)) {
        Invoke-WebRequest "https://github.com/upx/upx/releases/download/v$version/upx-$version-win64.zip" -OutFile $archive
    }
    if ((Get-FileHash -LiteralPath $archive).Hash -ne $expectedHash) { throw 'UPX archive checksum mismatch' }
    Expand-Archive -LiteralPath $archive -DestinationPath target/tools -Force
    & cargo build --release --locked --lib --bin molite-server
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $output = Join-Path $repo ('target/upx-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    New-Item -ItemType Directory -Path $output | Out-Null
    Copy-Item target/release/molite_core.dll,target/release/molite-server.exe,target/release/molite_core.dll.lib -Destination $output
    & $upx --ultra-brute --no-progress "$output/molite_core.dll" "$output/molite-server.exe"
    if ($LASTEXITCODE -ne 0) { throw 'UPX packing failed; original release files are unchanged' }
    & $upx -t "$output/molite_core.dll" "$output/molite-server.exe"
    if ($LASTEXITCODE -ne 0) { throw 'UPX integrity verification failed' }
    & python scripts/verify-packed.py $output target/release
    if ($LASTEXITCODE -ne 0) { throw 'Packed runtime verification failed' }
    & cmd /c scripts\test-c-abi.cmd $output
    if ($LASTEXITCODE -ne 0) { throw 'Packed C host verification failed' }
    $sizes = foreach ($name in @('molite_core.dll', 'molite-server.exe')) {
        [pscustomobject]@{
            file = $name
            original_bytes = (Get-Item "target/release/$name").Length
            packed_bytes = (Get-Item "$output/$name").Length
            packed_sha256 = (Get-FileHash "$output/$name").Hash
        }
    }
    $sizes | ConvertTo-Json | Set-Content "$output/sizes.json" -Encoding utf8
    $sizes | Format-Table
    Write-Output "Verified packed artifacts: $output"
} finally {
    Pop-Location
}
