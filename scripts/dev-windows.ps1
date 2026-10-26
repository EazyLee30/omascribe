[CmdletBinding()]
param(
    [string]$Python = 'python',
    [string]$Assets = (Join-Path $env:LOCALAPPDATA 'OmaScribe\native'),
    [switch]$SkipAsr,
    [switch]$Run
)
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    & $Python -c "import sys; assert sys.version_info >= (3, 11), 'Use Python 3.11+'"
    if ($LASTEXITCODE) { throw 'Python 3.11+ is required to verify runtime files.' }
    & rustup run 1.99.0 rustc --version
    if ($LASTEXITCODE) { throw 'Install the CI toolchain: rustup toolchain install 1.99.0 --profile minimal --component clippy' }
    & npm.cmd ci --ignore-scripts
    if ($LASTEXITCODE) { throw 'Frontend dependency installation failed.' }
    & npm.cmd run build
    if ($LASTEXITCODE) { throw 'Frontend build failed.' }
    & npx.cmd tauri icon assets/icon.svg
    if ($LASTEXITCODE) { throw 'Native icon preparation failed.' }
    # Separate single-instance identity and disposable data, even with a normal
    # OmaScribe installation running. Assets are reused read-only when present.
    $env:TAURI_CONFIG = '{"identifier":"io.omarchy.omascribe.dev"}'
    $env:OMASCRIBE_DATA_DIR = Join-Path $repository 'artifacts\windows-dev'
    $env:CARGO_BUILD_JOBS = '2'
    if (Test-Path -LiteralPath (Join-Path $Assets 'models')) {
        $env:OMASCRIBE_MODELS_DIR = Join-Path $Assets 'models'
        $env:OMASCRIBE_RUNTIME_DIR = Join-Path $Assets 'runtime'
    }
    & cargo +1.99.0 build -p omascribe -p omascribe-core --locked
    if ($LASTEXITCODE) { throw 'Windows desktop build failed.' }
    if (!$SkipAsr) {
        & $Python scripts/native-libraries.py --local --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE) { throw 'Pinned ASR SDK preparation failed.' }
        $env:SHERPA_ONNX_LIB_DIR = Join-Path $repository 'build-assets\native-asr\x86_64-pc-windows-msvc\lib'
        # The pinned ASR SDK uses the static CRT. Keep its separate target cache.
        $previousRustFlags = $env:RUSTFLAGS
        try {
            $env:RUSTFLAGS = '-C target-feature=+crt-static'
            & cargo +1.99.0 build -p omascribe-asr --locked --target-dir build-assets/windows-asr
            if ($LASTEXITCODE) { throw 'Windows ASR build failed.' }
        } finally {
            $env:RUSTFLAGS = $previousRustFlags
        }
        $devRuntime = Join-Path $repository 'artifacts\windows-runtime'
        New-Item -ItemType Directory -Path $devRuntime -Force | Out-Null
        Copy-Item -LiteralPath 'build-assets\windows-asr\debug\omascribe-asr.exe' -Destination $devRuntime
        Copy-Item -LiteralPath 'target\debug\omascribe-cli.exe' -Destination $devRuntime
        if (Test-Path -LiteralPath (Join-Path $Assets 'runtime\llama')) {
            Copy-Item -LiteralPath (Join-Path $Assets 'runtime\llama') -Destination $devRuntime -Recurse -Force
        }
        $env:OMASCRIBE_RUNTIME_DIR = $devRuntime
    }
    if ($Run) {
        Start-Process -FilePath (Join-Path $repository 'target\debug\omascribe.exe') -WindowStyle Hidden
    }
    Write-Output "Windows developer build ready. Data: $env:OMASCRIBE_DATA_DIR"
    Write-Output 'GPU components remain separate, detected and tested through Settings.'
} finally {
    Pop-Location
}
