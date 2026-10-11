# Laedt einen fest gepinnten FFmpeg-8.1-Build (LGPL, shared) von BtbN/FFmpeg-Builds nach third_party/ffmpeg
# und prueft die SHA-256-Pruefsumme; bei Abweichung bricht das Skript ab.
# Bewusst der LGPL-Build: dynamisch gelinkt, austauschbar, ohne die zusaetzlichen GPL-Bibliotheken des
# "gpl"-Builds. Gepinnt ist ein Monatsend-Build: BtbN raeumt Tages-Builds nach einigen Wochen ab, die
# Builds vom Monatsletzten bleiben. Siehe docs/teleport/DEPENDENCIES.md.
param(
    [string]$Tag = "autobuild-2026-08-31-13-27",
    [string]$Asset = "ffmpeg-n8.1.2-50-g1a748fe2cd-win64-lgpl-shared-8.1.zip",
    [string]$Sha256 = "e9712ffbdb03ef71bbab660c75b835bfe698ef6fad0247c76d8d394a39a3db63",
    [switch]$Force
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root "third_party\ffmpeg"
$stamp = Join-Path $dest ".source-sha256"
if ((Test-Path $stamp) -and -not $Force) {
    if ((Get-Content $stamp -Raw).Trim() -eq $Sha256) {
        Write-Host "FFmpeg ($Asset) liegt schon unter $dest"
        exit 0
    }
    Write-Host "FFmpeg unter $dest stammt aus einem anderen Build, lade neu"
}
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$Tag/$Asset"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ffmpeg-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    $zip = Join-Path $tmp "ffmpeg.zip"
    Write-Host "Lade $url"
    Invoke-WebRequest $url -OutFile $zip
    $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($got -ne $Sha256.ToLowerInvariant()) {
        throw "SHA-256 stimmt nicht: erwartet $Sha256, erhalten $got - Abbruch"
    }
    Write-Host "SHA-256 ok ($got)"
    Expand-Archive $zip $tmp
    $src = Get-ChildItem $tmp -Directory | Where-Object { Test-Path (Join-Path $_.FullName "bin\avcodec-62.dll") } | Select-Object -First 1
    if (-not $src) { throw "unerwarteter Inhalt: avcodec-62.dll (FFmpeg 8.x) fehlt" }
    if (-not (Select-String -Path (Join-Path $src.FullName "LICENSE.txt") -Pattern "LESSER GENERAL PUBLIC" -Quiet)) {
        throw "kein LGPL-Build"
    }
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Move-Item $src.FullName $dest
    Set-Content -Path $stamp -Value $Sha256 -NoNewline
    Write-Host "FFmpeg nach $dest entpackt"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
