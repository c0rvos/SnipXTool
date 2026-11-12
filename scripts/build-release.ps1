# Builds SnipXTool and produces:
#   dist\SnipXTool-<version>-Setup.exe       (installer, ffmpeg bundled)
#   dist\SnipXTool-<version>-portable.zip    (no installation needed, ffmpeg bundled)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
#   ... -NoFfmpeg         build WITHOUT bundled ffmpeg (small download)
#   ... -SkipInstaller    only build the portable package
#   ... -Clean            remove dist\ and build\ staging first
[CmdletBinding()]
param(
    [switch]$SkipInstaller,
    [switch]$NoFfmpeg,
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Write-Step([string]$Text) {
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}

# ── version from Cargo.toml ────────────────────────────────────────────────
$CargoToml = Get-Content (Join-Path $Root 'Cargo.toml') -Raw
if ($CargoToml -match '(?m)^\s*version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
} else {
    throw "Could not read the version from Cargo.toml."
}
Write-Host "SnipXTool $Version" -ForegroundColor Green

if ($Clean) {
    Write-Step "Cleaning dist\ and build\ staging"
    Remove-Item (Join-Path $Root 'dist') -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $Root 'build') -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Run scripts\setup-dev.ps1 first."
}

# ── compile ───────────────────────────────────────────────────────────────
Write-Step "Building release executable"
# Merge cargo's stderr (warnings/progress) into stdout so Windows PowerShell
# does not turn it into a terminating NativeCommandError.
$PreviousEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
& cmd /c "cargo build --release 2>&1"
$BuildExit = $LASTEXITCODE
$ErrorActionPreference = $PreviousEap
if ($BuildExit -ne 0) { throw "cargo build failed (exit code $BuildExit)." }

$Exe = Join-Path $Root 'target\release\snipxtool.exe'
if (-not (Test-Path $Exe)) { throw "Build finished but $Exe is missing." }

# ── optionally bundle ffmpeg ──────────────────────────────────────────────
$FfmpegStage = Join-Path $Root 'build\ffmpeg'
if ($NoFfmpeg) {
    Write-Step "Skipping ffmpeg bundling (-NoFfmpeg)"
    Remove-Item $FfmpegStage -Recurse -Force -ErrorAction SilentlyContinue
} else {
    Write-Step "Bundling ffmpeg (recording works right after install)"
    $staged = Join-Path $FfmpegStage 'ffmpeg.exe'
    $ffmpeg = $null
    if (Test-Path $staged) {
        # Already staged by hand (e.g. the smaller "essentials" build).
        $ffmpeg = $staged
        Write-Host "    Using pre-staged ffmpeg"
    } else {
        $ffmpeg = (Get-Command ffmpeg -ErrorAction SilentlyContinue).Source
        if (-not $ffmpeg) {
            Write-Warning ("ffmpeg was not found on PATH - building WITHOUT bundled ffmpeg. " +
                "Install it with: winget install Gyan.FFmpeg, or pass -NoFfmpeg intentionally.")
        }
    }
    if ($ffmpeg) {
        New-Item -ItemType Directory -Force -Path $FfmpegStage | Out-Null
        if ((Resolve-Path $ffmpeg).Path -ine $staged) {
            Copy-Item $ffmpeg $staged -Force
        }
    @"
This copy of ffmpeg is distributed with SnipXTool to enable screen recording
and video export.

ffmpeg is a separate project, licensed under the GNU GPL (this is the GPL
build from Gyan Doshi: https://www.gyan.dev/ffmpeg/builds/).

Source code and license: https://ffmpeg.org/
Your rights under the GPL include the right to obtain the corresponding
source code, available from https://ffmpeg.org/download.html and from the
build provider linked above.
"@ | Set-Content -Path (Join-Path $FfmpegStage 'README.txt') -Encoding UTF8
        Write-Host "    Bundled $ffmpeg"
    }
}

# ── portable package ─────────────────────────────────────────────────────
Write-Step "Creating portable package"
$Dist = Join-Path $Root 'dist'
$Stage = Join-Path $Dist "SnipXTool-$Version-portable"
Remove-Item $Stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item $Exe $Stage
Copy-Item (Join-Path $Root 'README.md') $Stage
if (Test-Path (Join-Path $FfmpegStage 'ffmpeg.exe')) {
    Copy-Item (Join-Path $FfmpegStage 'ffmpeg.exe') $Stage
    Copy-Item (Join-Path $FfmpegStage 'README.txt') $Stage
}
$Zip = Join-Path $Dist "SnipXTool-$Version-portable.zip"
Remove-Item $Zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $Zip -CompressionLevel Optimal
Write-Host "    $Zip"

if ($SkipInstaller) {
    Write-Step "Done (installer skipped)"
    exit 0
}

# ── installer ─────────────────────────────────────────────────────────────
Write-Step "Building installer"
$Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $Iscc) {
    foreach ($candidate in @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
        'C:\Program Files\Inno Setup 6\ISCC.exe'
    )) {
        if (Test-Path $candidate) { $Iscc = $candidate; break }
    }
}
if (-not $Iscc) {
    throw "Inno Setup 6 was not found. Run scripts\setup-dev.ps1, or build only the portable zip with -SkipInstaller."
}

& $Iscc "/DAppVersion=$Version" (Join-Path $Root 'installer\SnipXTool.iss')
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed." }

Write-Step "Done"
Get-ChildItem $Dist -File | ForEach-Object {
    Write-Host ("    {0}  ({1:N1} MB)" -f $_.Name, ($_.Length / 1MB))
}
exit 0


