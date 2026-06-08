# Bundle Holler for Windows.
#
# Produces a self-contained dist\ folder and a ZIP archive ready for release.
# Optionally accepts $BinaryPath to skip the cargo build step (used by CI).
#
# Usage (local):
#   pwsh scripts\bundle-windows.ps1
# Usage (CI — binary already built):
#   pwsh scripts\bundle-windows.ps1 -BinaryPath target\x86_64-pc-windows-msvc\release\holler.exe

param(
    [string]$BinaryPath = "",
    [string]$Version = "0.1.0"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$AppName = "Holler"
$DistDir = "dist\$AppName"

# Build if no pre-built binary supplied.
if ($BinaryPath -eq "") {
    Write-Host "==> Building release binary"
    cargo build --release -p holler-app
    $BinaryPath = "target\release\holler.exe"
}

if (-not (Test-Path $BinaryPath)) {
    Write-Error "Binary not found at $BinaryPath"
    exit 1
}

Write-Host "==> Assembling $DistDir"
if (Test-Path $DistDir) { Remove-Item -Recurse -Force $DistDir }
New-Item -ItemType Directory -Path $DistDir | Out-Null

Copy-Item $BinaryPath "$DistDir\holler.exe"

# Minimal README for first-run instructions.
@"
Holler $Version — push-to-talk dictation
=========================================

Quick start:
  1. Run holler.exe — a blue dot appears in the system tray.
  2. Hold Ctrl+Alt+Space (or your configured key) and speak.
  3. Release — the transcription is pasted at your cursor and copied
     to the clipboard.

Set your API key:
  holler.exe set-key deepgram <YOUR_KEY>
  holler.exe set-key openai   <YOUR_KEY>

Edit settings:
  Right-click the tray icon → Edit Settings (config.toml)

Quit:
  Right-click the tray icon → Quit Holler
"@ | Out-File -Encoding utf8 "$DistDir\README.txt"

# ZIP the dist folder.
$ZipName = "Holler-Windows-x64-v$Version.zip"
if (Test-Path $ZipName) { Remove-Item $ZipName }
Compress-Archive -Path $DistDir -DestinationPath $ZipName

Write-Host ""
Write-Host "Packaged: $ZipName"
Write-Host "Contents:"
Get-ChildItem $DistDir | ForEach-Object { Write-Host "  $_" }
