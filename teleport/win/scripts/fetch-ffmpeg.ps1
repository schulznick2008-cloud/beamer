# Laedt den FFmpeg-8.1-Build (LGPL, shared) von BtbN/FFmpeg-Builds nach third_party/ffmpeg.
# Bewusst der LGPL-Build: dynamisch gelinkt, austauschbar, ohne die zusaetzlichen GPL-Bibliotheken des
# "gpl"-Builds. Siehe docs/teleport/DEPENDENCIES.md.
param(
    [string]$Url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip",
    [switch]$Force
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root "third_party\ffmpeg"
if ((Test-Path (Join-Path $dest "bin\avcodec-62.dll")) -and -not $Force) {
    Write-Host "FFmpeg liegt schon unter $dest"
    exit 0
}
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ffmpeg-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    $zip = Join-Path $tmp "ffmpeg.zip"
    Write-Host "Lade $Url"
    Invoke-WebRequest $Url -OutFile $zip
    Expand-Archive $zip $tmp
    $src = Get-ChildItem $tmp -Directory | Where-Object { Test-Path (Join-Path $_.FullName "bin\avcodec-62.dll") } | Select-Object -First 1
    if (-not $src) { throw "unerwarteter Inhalt: avcodec-62.dll (FFmpeg 8.x) fehlt" }
    if (-not (Select-String -Path (Join-Path $src.FullName "LICENSE.txt") -Pattern "LESSER GENERAL PUBLIC" -Quiet)) {
        throw "kein LGPL-Build"
    }
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Move-Item $src.FullName $dest
    Write-Host "FFmpeg nach $dest entpackt"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
