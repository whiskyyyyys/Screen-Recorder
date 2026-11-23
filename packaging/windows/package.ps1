param(
    [switch]$SkipBuild = $false,
    [switch]$AllowSkipInstaller = $false
)

$ErrorActionPreference = 'Stop'
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

$CargoBin = "$env:USERPROFILE\.cargo\bin"
$WinLibsWindres = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs*" -Recurse -Filter "windres.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($WinLibsWindres) {
    $env:PATH = "$($WinLibsWindres.DirectoryName);$env:PATH"
}
$GnuSelfContained = "$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained"
if (Test-Path $GnuSelfContained) {
    $env:PATH = "$GnuSelfContained;$env:PATH"
}
if (Test-Path $CargoBin) {
    $env:PATH = "$CargoBin;$env:PATH"
}

if (-not $SkipBuild) {
    Write-Host "Building OxideStudio release binaries & plugins..."
    cargo build --release --workspace
}

$TargetDir = Join-Path $Root "target\release"
$DistDir = Join-Path $Root "dist"
$PortableDir = Join-Path $DistDir "OBS-Rust-Portable-x64"
$PortableZip = Join-Path $DistDir "OBS-Rust-Portable-x64.zip"

if (Test-Path $PortableDir) { Remove-Item -Recurse -Force $PortableDir }
New-Item -ItemType Directory -Force -Path $PortableDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PortableDir "plugins") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PortableDir "assets") | Out-Null

# Main executable & plugin DLL
Copy-Item (Join-Path $TargetDir "obs-rust.exe") (Join-Path $PortableDir "obs-rust.exe") -Force
$PluginDll = Join-Path $TargetDir "sample_vignette_plugin.dll"
if (Test-Path $PluginDll) {
    Copy-Item $PluginDll (Join-Path $PortableDir "plugins\sample_vignette_plugin.dll") -Force
}

# MinGW runtime DLLs if present beside toolchain (normally static, copy only if needed)
$FfmpegDir = Join-Path $Root ".tools\ffmpeg"
if (Test-Path (Join-Path $FfmpegDir "ffmpeg.exe")) {
    Copy-Item (Join-Path $FfmpegDir "ffmpeg.exe") (Join-Path $PortableDir "ffmpeg.exe") -Force
}
if (Test-Path (Join-Path $FfmpegDir "ffprobe.exe")) {
    Copy-Item (Join-Path $FfmpegDir "ffprobe.exe") (Join-Path $PortableDir "ffprobe.exe") -Force
}

# Assets & Legal docs
Copy-Item (Join-Path $Root "assets\*") (Join-Path $PortableDir "assets") -Recurse -Force
Copy-Item (Join-Path $Root "LICENSE") (Join-Path $PortableDir "LICENSE") -Force
Copy-Item (Join-Path $Root "THIRD_PARTY_LICENSES.md") (Join-Path $PortableDir "THIRD_PARTY_LICENSES.md") -Force
if (Test-Path (Join-Path $Root "README.md")) {
    Copy-Item (Join-Path $Root "README.md") (Join-Path $PortableDir "README.md") -Force
}

# Portable mode marker so config stays self-contained inside portable folder
"OxideStudio Portable Mode" | Out-File -FilePath (Join-Path $PortableDir "portable_mode.txt") -Encoding ascii

if (Test-Path $PortableZip) { Remove-Item -Force $PortableZip }
Write-Host "Creating $PortableZip..."
Compress-Archive -Path "$PortableDir\*" -DestinationPath $PortableZip -CompressionLevel Optimal

# Compile Windows Setup Installer via Inno Setup 6
$IsccCandidates = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
)
$Iscc = $IsccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($Iscc) {
    Write-Host "Compiling Windows Installer via $Iscc..."
    & $Iscc (Join-Path $PSScriptRoot "installer.iss")
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed with exit code $LASTEXITCODE"
    }
} elseif ($AllowSkipInstaller) {
    Write-Warning "ISCC.exe (Inno Setup 6) not found - skipped Setup EXE build (-AllowSkipInstaller set)."
} else {
    throw "ISCC.exe (Inno Setup 6) not found. Install Inno Setup 6 or pass -AllowSkipInstaller."
}

Get-ChildItem $DistDir -File | Select-Object Name, Length | Format-Table -AutoSize
