param(
    [string] $OutputRoot = $(if ($env:NIA_RELEASE_OUTPUT) { $env:NIA_RELEASE_OUTPUT } else { Join-Path $PSScriptRoot '..\..\target\release-package' }),
    [string] $Platform = $(if ($env:NIA_RELEASE_PLATFORM) { $env:NIA_RELEASE_PLATFORM } else { 'windows-x86_64' }),
    [string] $RepositoryRoot = $(if ($env:NIA_REPO_ROOT) { $env:NIA_REPO_ROOT } else { Join-Path $PSScriptRoot '..\..' })
)

$ErrorActionPreference = 'Stop'
if ($Platform -ne 'windows-x86_64') {
    throw "unsupported release platform: $Platform"
}

$repoRoot = (Resolve-Path $RepositoryRoot).Path
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
Push-Location $repoRoot
$pkgid = (& cargo pkgid -p nia-cli | Select-Object -Last 1).Trim()
$version = ($pkgid -split '#')[-1]
Pop-Location
if (-not $version) { throw 'failed to determine nia-cli version' }

$llvmPrefix = if ($env:LLVM_SYS_231_PREFIX) { $env:LLVM_SYS_231_PREFIX } else { Join-Path $repoRoot 'target\llvm-static\install' }
$binary = Join-Path $repoRoot 'target\x86_64-pc-windows-msvc\release\nia.exe'
$lld = Join-Path $llvmPrefix 'bin\ld.lld.exe'
$resourceRoot = Join-Path $repoRoot 'lib'
if (-not (Test-Path $llvmPrefix -PathType Container)) { throw "release input is missing: $llvmPrefix" }
foreach ($required in @($binary, $lld, (Join-Path $resourceRoot 'toolchain.meta'), (Join-Path $resourceRoot 'std\pkg.nia'), (Join-Path $resourceRoot 'runtime\start.nia'))) {
    if (-not (Test-Path $required -PathType Leaf)) { throw "release input is missing: $required" }
}

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
$stageRoot = Join-Path $OutputRoot "nia-$version-$Platform"
$archive = Join-Path $OutputRoot "nia-$version-$Platform.zip"
$checksums = Join-Path $OutputRoot "SHA256SUMS-$Platform"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $stageRoot
Remove-Item -Force -ErrorAction SilentlyContinue $archive, $checksums
New-Item -ItemType Directory -Force -Path (Join-Path $stageRoot 'bin'), (Join-Path $stageRoot 'lib'), (Join-Path $stageRoot 'libexec') | Out-Null

Copy-Item $binary (Join-Path $stageRoot 'bin\nia.exe')
Copy-Item (Join-Path $resourceRoot '*') (Join-Path $stageRoot 'lib') -Recurse
Copy-Item $lld (Join-Path $stageRoot 'libexec\ld.lld.exe')
Copy-Item (Join-Path $repoRoot 'README.md'), (Join-Path $repoRoot 'LICENSE.md') $stageRoot

$gitRevision = (& git -C $repoRoot rev-parse HEAD).Trim()
$llvmVersion = (& (Join-Path $llvmPrefix 'bin\llvm-config.exe') --version).Trim()
@(
    "nia-version=$version"
    "git-revision=$gitRevision"
    "platform=$Platform"
    "llvm-version=$llvmVersion"
) | Set-Content -Encoding ascii (Join-Path $stageRoot 'release-manifest.txt')

Compress-Archive -Path $stageRoot -DestinationPath $archive -CompressionLevel Optimal
$digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
"$digest  $(Split-Path $archive -Leaf)" | Set-Content -Encoding ascii $checksums
Write-Host "release archive: $archive"
Write-Host "checksums: $checksums"
