# Configure a PowerShell session for the Windows MSVC LLVM build.
# Dot-source this file: . .\tools\windows-env.ps1

$ErrorActionPreference = 'Stop'

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ((Test-Path (Join-Path $cargoBin 'cargo.exe')) -and
    (($env:Path -split ';') -notcontains $cargoBin)) {
    $env:Path = "$cargoBin;$($env:Path)"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo was not found on PATH. Install Rust with rustup or add %USERPROFILE%\\.cargo\\bin to PATH, then open a fresh PowerShell session.'
}
if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    throw 'rustc was not found on PATH. Install Rust with rustup or add %USERPROFILE%\\.cargo\\bin to PATH, then open a fresh PowerShell session.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $env:LLVM_SYS_231_PREFIX) {
    throw 'Set LLVM_SYS_231_PREFIX to the LLVM 23.1 prefix before loading this script.'
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    throw 'Visual Studio Installer vswhere.exe was not found.'
}

$vsRoot = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsRoot) {
    throw 'A Visual Studio installation with the x64 MSVC tools was not found.'
}

$vcRoot = Join-Path $vsRoot 'VC\Tools\MSVC'
$vcTools = Get-ChildItem $vcRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
$programFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
$sdkRoot = Join-Path $programFilesX86 'Windows Kits\10\Lib'
$sdk = Get-ChildItem $sdkRoot -Directory |
    Where-Object { $_.Name -match '^\d+\.' } |
    Sort-Object Name -Descending |
    Select-Object -First 1
if (-not $vcTools -or -not $sdk) {
    throw 'The MSVC or Windows SDK library directories were not found.'
}
$sdkInclude = Join-Path (Join-Path $programFilesX86 'Windows Kits\10\Include') $sdk.Name

$libPaths = @(
    (Join-Path $vcTools.FullName 'lib\x64'),
    (Join-Path $sdk.FullName 'um\x64'),
    (Join-Path $sdk.FullName 'ucrt\x64')
) | Where-Object { Test-Path $_ }

$includePaths = @(
    (Join-Path $vcTools.FullName 'include'),
    (Join-Path $sdkInclude 'shared'),
    (Join-Path $sdkInclude 'um'),
    (Join-Path $sdkInclude 'ucrt')
) | Where-Object { Test-Path $_ }

$toolPaths = @(
    (Join-Path $vcTools.FullName 'bin\Hostx64\x64'),
    (Get-ChildItem (Join-Path $programFilesX86 'Windows Kits\10\bin') -Directory |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64' } |
        Where-Object { Test-Path (Join-Path $_ 'rc.exe') } |
        Select-Object -First 1)
) | Where-Object { $_ -and (Test-Path $_) }

function ConvertTo-ShortPath([string] $path) {
    $comspec = if ($env:ComSpec) { $env:ComSpec } else { Join-Path $env:SystemRoot 'System32\cmd.exe' }
    if (-not (Test-Path $comspec)) { return $path }
    (& $comspec /d /s /c "for %I in (`"$path`") do @echo %~sI").Trim()
}

$llvmPrefix = (Resolve-Path $env:LLVM_SYS_231_PREFIX).Path
$llvmConfig = Join-Path $llvmPrefix 'bin\llvm-config.exe'
if (-not (Test-Path $llvmConfig)) {
    throw "LLVM llvm-config.exe was not found under $llvmPrefix."
}

# LLVM's official Windows archives can contain an absolute zstd path from the
# build machine in `llvm-config --system-libs`. Stage a small proxy outside the
# LLVM installation so llvm-sys can ignore only missing paths safely.
$systemLibs = (& $llvmConfig --link-static --system-libs 2>$null).Trim()
$llvmLib = Join-Path $llvmPrefix 'lib'
$missingLibraries = $systemLibs -split '\s+' | Where-Object {
    $token = $_
    if ($token -match '^[A-Za-z]:[\\/].*\.lib$') {
        return -not (Test-Path $token)
    }
    if ($token -notmatch '^[^\\/]+\.lib$') {
        return $false
    }
    $inLlvm = Test-Path (Join-Path $llvmLib $token)
    $inSearchPath = @($libPaths | Where-Object { Test-Path (Join-Path $_ $token) }).Count -gt 0
    return -not ($inLlvm -or $inSearchPath)
}
if ($missingLibraries) {
    $wrapperRoot = Join-Path $repoRoot 'target\llvm-config-wrapper'
    $wrapperBin = Join-Path $wrapperRoot 'bin'
    New-Item -ItemType Directory -Force -Path $wrapperBin | Out-Null
    $wrapperExe = Join-Path $wrapperBin 'llvm-config.exe'
    $source = Join-Path $repoRoot 'tools\llvm-config-wrapper.rs'
    $env:NIA_LLVM_CONFIG_REAL = $llvmConfig
    & rustc $source --edition=2021 -C opt-level=2 -o $wrapperExe
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $wrapperExe)) {
        throw 'Failed to build the llvm-config Windows path wrapper.'
    }
    $env:LLVM_SYS_231_PREFIX = $wrapperRoot
    Write-Host "Staged llvm-config wrapper for missing LLVM library entries: $wrapperRoot"
}

$env:Path = (($toolPaths + @((Join-Path $env:LLVM_SYS_231_PREFIX 'bin')) + @($env:Path -split ';')) |
    Where-Object { $_ } | Select-Object -Unique) -join ';'
$env:LIB = (($libPaths + @($env:LIB -split ';')) | Where-Object { $_ } | Select-Object -Unique) -join ';'
$env:INCLUDE = (($includePaths + @($env:INCLUDE -split ';')) | Where-Object { $_ } | Select-Object -Unique) -join ';'
$env:CFLAGS = '/MT'
$env:CXXFLAGS = '/MT'
$nativeFlags = $libPaths | ForEach-Object { "-Lnative=$(ConvertTo-ShortPath $_)" }
$rustFlags = @()
if ($env:RUSTFLAGS) { $rustFlags += $env:RUSTFLAGS }
$rustFlags += '-Ctarget-feature=+crt-static'
$rustFlags += $nativeFlags
$env:RUSTFLAGS = $rustFlags -join ' '

Write-Host "Configured LLVM 23.1, MSVC, and Windows SDK for x64 static-CRT builds."
