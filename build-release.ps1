param([string]$TargetDir = (Join-Path $PSScriptRoot 'target'))

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath($PSScriptRoot)
$TargetDir = [IO.Path]::GetFullPath($TargetDir)
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
} else {
    $cargoPath = $cargo.Source
}

$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
    # Encoded flags also support paths containing spaces without shell quoting.
    $flags = @(
        "--remap-path-prefix=$env:USERPROFILE=/user",
        "--remap-path-prefix=$TargetDir=/build",
        "--remap-path-prefix=$projectRoot=/src/mabipng-studio"
    )
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
    & $cargoPath build --release --locked --manifest-path (Join-Path $projectRoot 'Cargo.toml') --target-dir $TargetDir
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    Write-Output (Join-Path $TargetDir 'release\mabipng-studio.exe')
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
}
