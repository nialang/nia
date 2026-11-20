# SPDX-License-Identifier: GPL-3.0-or-later
# Tests an installed Nia release and VSIX through a real VS Code extension host.
#
#   pwsh tools/lsp-vscode-smoke/smoke.ps1 -Code <Code executable> -Cli <code CLI> `
#       -Server <unpacked bin/nia-lsp> -Vsix <nia.vsix>
#
# The extension host runs run.js with VS Code's own runtime; no separate Node
# toolchain is involved.

param(
    [Parameter(Mandatory)] [string] $Code,
    [string] $Cli = 'code',
    [Parameter(Mandatory)] [string] $Server,
    [Parameter(Mandatory)] [string] $Vsix
)

$ErrorActionPreference = 'Stop'

$executable = (Resolve-Path -LiteralPath $Code).Path
$serverPath = (Resolve-Path -LiteralPath $Server).Path
$vsixPath = (Resolve-Path -LiteralPath $Vsix).Path
$cliCommand = Get-Command $Cli -ErrorAction SilentlyContinue
if (-not $cliCommand) { throw "cannot find VS Code CLI $Cli" }
$cliPath = $cliCommand.Source

# Start-Process joins its arguments with spaces, so quote each one; Code.exe is
# a GUI program, which the call operator would not wait for.
function Invoke-Waited([string] $FilePath, [string[]] $Arguments, [string] $What) {
    $line = ($Arguments | ForEach-Object { '"' + ($_ -replace '"', '\"') + '"' }) -join ' '
    $process = Start-Process -FilePath $FilePath -ArgumentList $line -NoNewWindow -PassThru
    if (-not $process.WaitForExit(60000)) {
        $process.Kill($true)
        throw "$What timed out"
    }
    $process.ExitCode
}

$harness = $PSScriptRoot
$repository = Split-Path -Parent (Split-Path -Parent $harness)
$root = Join-Path ([IO.Path]::GetTempPath()) ("nia-vscode-smoke-" + [guid]::NewGuid().ToString('N'))
try {
    $user = Join-Path $root 'user'
    $settings = Join-Path $user 'User/settings.json'
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $settings) | Out-Null
    @{
        'nia.server.path' = $serverPath
        'workbench.iconTheme' = 'nia'
        'telemetry.telemetryLevel' = 'off'
        'extensions.autoUpdate' = $false
        'extensions.autoCheckUpdates' = $false
    } | ConvertTo-Json | Set-Content -LiteralPath $settings -Encoding utf8NoBOM

    $workspace = Join-Path $root 'workspace'
    New-Item -ItemType Directory -Force -Path (Join-Path $workspace 'source') | Out-Null
    $sources = [ordered]@{
        'nia.toml' = "[package]`nname = `"file_move`"`nversion = `"1.0.0`"`n[library]`npath = `"root.nia`"`n"
        'root.nia' = "module source;`nfun value() i32 {`n    source::nested::read()`n}`n"
        'source.nia' = "pub module nested;`n"
        'source/nested.nia' = "pub fun read() i32 { 42 }`n"
    }
    foreach ($entry in $sources.GetEnumerator()) {
        $path = Join-Path $workspace $entry.Key
        [IO.File]::WriteAllText($path, $entry.Value, [Text.UTF8Encoding]::new($false))
    }

    $profileArguments = @("--user-data-dir=$user", "--extensions-dir=$(Join-Path $root 'extensions')")
    $status = Invoke-Waited $cliPath ($profileArguments + @('--install-extension', $vsixPath)) 'VSIX installation'
    if ($status -ne 0) { throw "VSIX installation failed with status $status" }

    $result = Join-Path $root 'result.txt'
    $env:NIA_SMOKE_RESULT = $result
    $env:NIA_SMOKE_ICON = Join-Path $repository 'assets/brand/nia-icon.svg'
    $status = Invoke-Waited $executable ($profileArguments + @(
            "--extensionDevelopmentPath=$harness",
            "--extensionTestsPath=$(Join-Path $harness 'run.js')",
            '--skip-welcome', '--skip-release-notes', '--disable-workspace-trust', $workspace)) 'VS Code smoke test'
    if (Test-Path -LiteralPath $result) { (Get-Content -LiteralPath $result -Raw).Trim() }
    if ($status -ne 0) { throw "VS Code exited with status $status" }
    if (-not (Test-Path -LiteralPath $result)) { throw 'VS Code exited before reporting the smoke test result' }
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
