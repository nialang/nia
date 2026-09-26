@echo off
if /I not "%MODE%"=="fixture" exit /b 1
if not exist "%~5" exit /b 1
if not exist "%~6" exit /b 1
if not exist "%~7" exit /b 1
if not exist "%~8" exit /b 1
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "$contents = [IO.File]::ReadAllText('%~2').Replace([string][char]13, [string]::Empty); [IO.File]::WriteAllText('%~3', $contents.ToUpperInvariant()); [IO.File]::WriteAllText('%~4', ('source=tool-input' + [char]10))"
