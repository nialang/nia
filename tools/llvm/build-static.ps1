param(
    [string] $SourceRoot = $(if ($env:LLVM_SOURCE_ROOT) { $env:LLVM_SOURCE_ROOT } else { Join-Path $PSScriptRoot '..\..\target\llvm-source' }),
    [string] $BuildRoot = $(if ($env:LLVM_BUILD_ROOT) { $env:LLVM_BUILD_ROOT } else { Join-Path $PSScriptRoot '..\..\target\llvm-static\build' }),
    [string] $InstallRoot = $(if ($env:LLVM_INSTALL_ROOT) { $env:LLVM_INSTALL_ROOT } else { Join-Path $PSScriptRoot '..\..\target\llvm-static\install' })
)

$ErrorActionPreference = 'Stop'
$releaseTag = if ($env:LLVM_RELEASE_TAG) { $env:LLVM_RELEASE_TAG } else { 'llvmorg-23.1.2' }
$llvmRepository = if ($env:LLVM_REPOSITORY) { $env:LLVM_REPOSITORY } else { 'https://github.com/llvm/llvm-project.git' }

if (-not (Test-Path (Join-Path $SourceRoot 'llvm\CMakeLists.txt'))) {
    New-Item -ItemType Directory -Force -Path (Split-Path $SourceRoot) | Out-Null
    & git clone --depth 1 --branch $releaseTag $llvmRepository $SourceRoot
    if ($LASTEXITCODE -ne 0) { throw 'Failed to clone the pinned LLVM source tree.' }
}

$savedErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$actualTag = & git -C $SourceRoot describe --tags --exact-match 2>$null
$actualTag = if ($null -eq $actualTag) { '' } else { ([string]$actualTag).Trim() }
$ErrorActionPreference = $savedErrorActionPreference
if ($actualTag -ne $releaseTag) {
    throw "LLVM source tree is not checked out at $releaseTag (found $actualTag)."
}

$programFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
$vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Visual Studio Installer vswhere.exe was not found.' }
$vsRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
if (-not $vsRoot) { throw 'Visual Studio with the x64 MSVC tools was not found.' }

$vcRoot = Join-Path $vsRoot 'VC\Tools\MSVC'
$vcTools = Get-ChildItem $vcRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
$sdkRoot = Join-Path $programFilesX86 'Windows Kits\10\Lib'
$sdk = Get-ChildItem $sdkRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
if (-not $vcTools -or -not $sdk) { throw 'The MSVC or Windows SDK library directories were not found.' }
$sdkInclude = Join-Path (Join-Path $programFilesX86 'Windows Kits\10\Include') $sdk.Name

$cmake = Join-Path $vsRoot 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
$ninja = Join-Path $vsRoot 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'
$cl = Join-Path $vcTools.FullName 'bin\Hostx64\x64'
$sdkBin = Get-ChildItem (Join-Path $programFilesX86 'Windows Kits\10\bin') -Directory |
    Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName 'x64' } |
    Where-Object { Test-Path (Join-Path $_ 'rc.exe') } |
    Select-Object -First 1
if (-not (Test-Path $cmake) -or -not (Test-Path (Join-Path $ninja 'ninja.exe'))) {
    throw 'CMake and Ninja from the Visual Studio C++ workload are required.'
}
if (-not $sdkBin) { throw 'A Windows SDK bin directory with rc.exe was not found.' }
$resourceCompiler = Join-Path $sdkBin 'rc.exe'
$manifestTool = Join-Path $sdkBin 'mt.exe'
if (-not (Test-Path $manifestTool)) { throw 'A Windows SDK bin directory with mt.exe was not found.' }

if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
    $python = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Programs\Python') -Recurse -Filter python.exe -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
    if ($python) { $env:Path = "$($python.Directory.FullName);$env:Path" }
}
if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
    throw 'Python 3 is required by the LLVM CMake configuration.'
}

$env:Path = (($cl, $sdkBin, $ninja, (Split-Path $cmake), $env:Path -split ';') |
    Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique) -join ';'
$env:LIB = (@(
    (Join-Path $vcTools.FullName 'lib\x64'),
    (Join-Path $sdk.FullName 'um\x64'),
    (Join-Path $sdk.FullName 'ucrt\x64'),
    $env:LIB -split ';'
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique) -join ';'
$env:INCLUDE = (@(
    (Join-Path $vcTools.FullName 'include'),
    (Join-Path $sdkInclude 'shared'),
    (Join-Path $sdkInclude 'um'),
    (Join-Path $sdkInclude 'ucrt'),
    $env:INCLUDE -split ';'
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique) -join ';'

$jobs = if ($env:LLVM_BUILD_JOBS) { $env:LLVM_BUILD_JOBS } else { [Environment]::ProcessorCount }
$cmakeArgs = @(
    '-S', (Join-Path $SourceRoot 'llvm'),
    '-B', $BuildRoot,
    '-G', 'Ninja',
    '-DCMAKE_BUILD_TYPE=Release',
    "-DCMAKE_INSTALL_PREFIX=$InstallRoot",
    '-DCMAKE_C_COMPILER=cl.exe',
    '-DCMAKE_CXX_COMPILER=cl.exe',
    "-DCMAKE_RC_COMPILER=$resourceCompiler",
    "-DCMAKE_MT=$manifestTool",
    '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    '-DLLVM_ENABLE_PROJECTS=lld',
    '-DLLVM_TARGETS_TO_BUILD=X86',
    '-DLLVM_BUILD_LLVM_DYLIB=OFF',
    '-DLLVM_LINK_LLVM_DYLIB=OFF',
    '-DLLVM_ENABLE_ZLIB=OFF',
    '-DLLVM_ENABLE_ZSTD=OFF',
    '-DLLVM_ENABLE_LIBXML2=OFF',
    '-DLLVM_ENABLE_LIBEDIT=OFF',
    '-DLLVM_ENABLE_TERMINFO=OFF',
    '-DLLVM_ENABLE_LIBPFM=OFF',
    '-DLLVM_INCLUDE_BENCHMARKS=OFF',
    '-DLLVM_INCLUDE_EXAMPLES=OFF',
    '-DLLVM_INCLUDE_TESTS=OFF',
    '-DLLVM_BUILD_TESTS=OFF',
    '-DLLVM_BUILD_BENCHMARKS=OFF',
    '-DLLVM_BUILD_EXAMPLES=OFF'
)
& $cmake @cmakeArgs
if ($LASTEXITCODE -ne 0) { throw 'LLVM CMake configuration failed.' }
& $cmake '--build' $BuildRoot '--parallel' $jobs
if ($LASTEXITCODE -ne 0) { throw 'LLVM build failed.' }
& $cmake '--install' $BuildRoot
if ($LASTEXITCODE -ne 0) { throw 'LLVM install failed.' }

$config = Join-Path $InstallRoot 'bin\llvm-config.exe'
if (-not (Test-Path $config)) { throw "LLVM installation did not produce $config." }
Write-Host "LLVM prefix: $InstallRoot"
& $config '--version'
