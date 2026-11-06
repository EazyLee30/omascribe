[CmdletBinding()]
param(
    [ValidateSet('cuda', 'vulkan')][string]$Backend = 'cuda',
    [string]$CudaToolkit = $env:CUDA_PATH,
    [string]$CudaArchitectures = '75-real',
    [string]$VulkanSdk = $env:VULKAN_SDK,
    [switch]$LegacyCudaHost,
    [int]$Jobs = 2
)
# Developer-only build. Formal downloads use acceleration-components.py and its
# fixed SDK/matrix/signing gates. No runtime is installed or activated here.
$ErrorActionPreference = 'Stop'
if ($Jobs -lt 1 -or $Jobs -gt 4) { throw 'Use 1-4 compile jobs.' }
if ($CudaArchitectures -notmatch '^([0-9]+[a-z]?-(real|virtual))(;[0-9]+[a-z]?-(real|virtual))*$') {
    throw 'Use explicit CUDA architectures, for example 75-real for GTX 1650.'
}
$repository = Split-Path -Parent $PSScriptRoot
$spec = (Get-Content -LiteralPath (Join-Path $repository 'models.lock.json') -Raw | ConvertFrom-Json).llama_cpp
$source = Join-Path $repository 'build-assets\llama-local'
$build = Join-Path $repository "build-assets\$Backend-local-build"
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsRoot = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$vsRoot) { throw 'Visual Studio C++ Build Tools are required.' }
Import-Module (Join-Path $vsRoot 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vsRoot -SkipAutomaticLocation -Arch amd64 -HostArch amd64
$cmake = Join-Path $vsRoot 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
$ninja = Join-Path $vsRoot 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
if (!(Test-Path -LiteralPath (Join-Path $source '.git'))) {
    if ((Test-Path -LiteralPath $source) -and (Get-ChildItem -LiteralPath $source -Force | Select-Object -First 1)) {
        throw 'The source cache is not an empty directory or Git checkout.'
    }
    New-Item -ItemType Directory -Path $source -Force | Out-Null
    & git -C $source init
}
if (& git -C $source status --porcelain --untracked-files=no) { throw 'Preserve or commit local runtime source edits before building.' }
$revision = & git -C $source rev-parse HEAD 2>$null
if ($revision -ne $spec.revision) {
    & git -C $source fetch --depth 1 $spec.url $spec.revision
    if ($LASTEXITCODE) { throw 'Pinned runtime source fetch failed.' }
    & git -C $source checkout --detach FETCH_HEAD
}
if ((& git -C $source rev-parse HEAD) -ne $spec.revision) { throw 'Runtime source revision mismatch.' }
$flags = @(
    '-G', 'Ninja', "-DCMAKE_MAKE_PROGRAM=$ninja", '-DCMAKE_BUILD_TYPE=Release',
    '-DBUILD_SHARED_LIBS=OFF', '-DGGML_BACKEND_DL=OFF', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    '-DGGML_NATIVE=OFF', '-DGGML_CPU_ALL_VARIANTS=OFF', '-DGGML_AVX=ON', '-DGGML_AVX2=ON', '-DGGML_AVX512=OFF',
    '-DGGML_BLAS=OFF', '-DGGML_OPENMP=OFF', '-DGGML_METAL=OFF', '-DLLAMA_OPENSSL=OFF', '-DLLAMA_CURL=OFF',
    '-DLLAMA_BUILD_TESTS=OFF', '-DLLAMA_BUILD_EXAMPLES=OFF', '-DLLAMA_BUILD_UI=OFF', '-DLLAMA_USE_PREBUILT_UI=OFF'
)
if ($Backend -eq 'cuda') {
    if (!(Test-Path -LiteralPath (Join-Path $CudaToolkit 'bin\nvcc.exe'))) { throw 'Select an installed CUDA toolkit with -CudaToolkit.' }
    $CudaToolkit = (Resolve-Path -LiteralPath $CudaToolkit).Path.Replace('\', '/')
    $flags += @('-DGGML_CUDA=ON', '-DGGML_VULKAN=OFF', '-DGGML_CUDA_FA=OFF', '-DGGML_CUDA_NCCL=OFF',
        "-DCUDAToolkit_ROOT=$CudaToolkit", "-DCMAKE_CUDA_COMPILER=$CudaToolkit/bin/nvcc.exe", "-DCMAKE_CUDA_ARCHITECTURES=$CudaArchitectures")
    $cudaFlags = if ($LegacyCudaHost) { '-allow-unsupported-compiler -D_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH' } else { '' }
    $flags += "-DCMAKE_CUDA_FLAGS=$cudaFlags"
} else {
    if (!(Test-Path -LiteralPath (Join-Path $VulkanSdk 'Include\vulkan\vulkan.h'))) { throw 'Select a Vulkan development SDK with -VulkanSdk.' }
    $vulkanLibrary = @('Lib\vulkan-1.lib', 'Lib\VulkanLoader\lib\vulkan-1.lib') | ForEach-Object { Join-Path $VulkanSdk $_ } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (!$vulkanLibrary) { throw 'Vulkan development loader library is missing.' }
    $env:VULKAN_SDK = $VulkanSdk
    $flags += @('-DGGML_CUDA=OFF', '-DGGML_VULKAN=ON', "-DVulkan_INCLUDE_DIR=$VulkanSdk/Include", "-DVulkan_LIBRARY=$vulkanLibrary", "-DCMAKE_PREFIX_PATH=$VulkanSdk")
}
& $cmake -S $source -B $build @flags
if ($LASTEXITCODE) { throw 'Local GPU runtime configuration failed.' }
& $cmake --build $build --target llama-server --parallel $Jobs
if ($LASTEXITCODE) { throw 'Local GPU runtime build failed.' }
$binary = Join-Path $build 'bin\llama-server.exe'
$toolkitVersion = if ($Backend -eq 'cuda') { (& (Join-Path $CudaToolkit 'bin\nvcc.exe') --version) -join "`n" } else { $null }
$metadata = @{ developer_only = $true; revision = $spec.revision; backend = $Backend; target = 'windows-x86_64';
    cuda_architectures = $CudaArchitectures; legacy_cuda_host = [bool]$LegacyCudaHost; flags = $flags;
    cuda_toolkit_version = $toolkitVersion;
    binary_sha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() }
$metadata | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $build 'developer-build.json') -Encoding utf8
Write-Output "Built $binary. Validate it with tests/benchmark_windows_gpu.py before use."
