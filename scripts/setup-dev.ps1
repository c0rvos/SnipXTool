# One-time developer setup for SnipXTool.
# Run from anywhere:  powershell -ExecutionPolicy Bypass -File scripts\setup-dev.ps1
[CmdletBinding()]
param(
    # Visual Studio Build Tools are large and need admin rights, so they are opt-in.
    [switch]$InstallBuildTools
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Text) {
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}

function Test-WingetPackage([string]$Id) {
    winget list --id $Id -e --accept-source-agreements *> $null
    return $LASTEXITCODE -eq 0
}

function Install-WingetPackage([string]$Id, [string]$Name, [string[]]$ExtraArgs = @()) {
    if (Test-WingetPackage $Id) {
        Write-Host "    $Name is already installed."
        return
    }
    Write-Host "    Installing $Name..."
    $wingetArgs = @('install', '--id', $Id, '-e', '--silent',
                    '--accept-source-agreements', '--accept-package-agreements') + $ExtraArgs
    & winget @wingetArgs
    if ($LASTEXITCODE -ne 0) {
        throw "winget failed to install $Name (exit code $LASTEXITCODE)."
    }
}

if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw "winget is required. Install 'App Installer' from the Microsoft Store, then re-run this script."
}

Write-Step "Checking Rust toolchain"
if (Get-Command cargo -ErrorAction SilentlyContinue) {
    Write-Host "    $(cargo --version)"
} else {
    Install-WingetPackage 'Rustlang.Rustup' 'Rust (rustup)'
    Write-Host "    Restart your terminal so 'cargo' is on PATH, then run this script again."
}

Write-Step "Checking Windows C++ build tools (linker)"
$VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$MsvcInstalled = $false
if (Test-Path $VsWhere) {
    $MsvcInstalled = & $VsWhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
}
if ($MsvcInstalled) {
    Write-Host "    MSVC build tools found."
} elseif ($InstallBuildTools) {
    Write-Host "    Installing Visual Studio 2022 Build Tools with the C++ workload (large)..." -ForegroundColor Yellow
    & winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent `
        --accept-source-agreements --accept-package-agreements `
        --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    if ($LASTEXITCODE -ne 0) { throw "Build Tools installation failed." }
} else {
    Write-Warning "MSVC build tools were not detected. If 'cargo build' fails with a linker error,"
    Write-Warning "re-run this script with -InstallBuildTools (needs admin), or install the"
    Write-Warning "'Desktop development with C++' workload in Visual Studio Build Tools."
}

Write-Step "Checking ffmpeg (needed for recording/video export)"
if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
    Write-Host "    $((Get-Command ffmpeg).Source)"
} else {
    Install-WingetPackage 'Gyan.FFmpeg' 'ffmpeg'
}

Write-Step "Checking Inno Setup (builds the installer)"
if (Get-Command ISCC.exe -ErrorAction SilentlyContinue) {
    Write-Host "    $((Get-Command ISCC.exe).Source)"
} elseif (Test-Path "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") {
    Write-Host "    $env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
} else {
    Install-WingetPackage 'JRSoftware.InnoSetup' 'Inno Setup 6' @('--scope', 'user')
}

Write-Step "Checking ImageMagick (optional, only to regenerate assets\icon.ico)"
if (Get-Command magick -ErrorAction SilentlyContinue) {
    Write-Host "    $((Get-Command magick).Source)"
} else {
    Write-Host "    Not installed - the icon is already committed, so this is optional." -ForegroundColor DarkGray
}

Write-Step "Done"
Write-Host "Build everything with:  powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1"
Write-Host "Run the app with:       cargo run --release"
exit 0

