<#
  Stages the FFmpeg build that the Windows installer ships in src-tauri/ffmpeg/:
  ffmpeg.exe, ffprobe.exe, FFmpeg's licence, its build notes and a NOTICE.

    tools/stage-ffmpeg.ps1                 # download the pinned build and check its SHA-256
    tools/stage-ffmpeg.ps1 -From <folder>  # copy from an extracted build of the same version
                                           # (e.g. the winget install of Gyan.FFmpeg.Essentials)

  Either way it checks that the build has the encoders and filters Backtrack uses.
  To move to a newer FFmpeg, update $Version, $Sha256 and $Commit together
  (`winget show Gyan.FFmpeg.Essentials --version <v>` prints the URL, SHA-256 and source commit).
#>
param([string]$From)
$ErrorActionPreference = 'Stop'

$Version = '9.0.2'
$Url     = "https://github.com/GyanD/codexffmpeg/releases/download/$Version/ffmpeg-$Version-essentials_build.zip"
$Sha256  = '60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba'
$Commit  = '946fcce07b'

$repo = Split-Path $PSScriptRoot -Parent
$out  = Join-Path $repo 'src-tauri\ffmpeg'

if (-not $From) {
  $tmp = Join-Path ([IO.Path]::GetTempPath()) "backtrack-ffmpeg-$Version"
  New-Item -ItemType Directory -Force $tmp | Out-Null
  $zip = Join-Path $tmp 'ffmpeg.zip'
  if (-not (Test-Path $zip) -or (Get-FileHash $zip -Algorithm SHA256).Hash -ne $Sha256) {
    Write-Host "Downloading FFmpeg $Version"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest $Url -OutFile $zip -UseBasicParsing
  }
  $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
  if ($hash -ne $Sha256) { throw "FFmpeg download has the wrong SHA-256: got $hash, expected $Sha256" }
  Expand-Archive $zip -DestinationPath $tmp -Force
  $From = Join-Path $tmp "ffmpeg-$Version-essentials_build"
}

foreach ($f in 'bin\ffmpeg.exe', 'bin\ffprobe.exe', 'LICENSE', 'README.txt') {
  if (-not (Test-Path (Join-Path $From $f))) { throw "$From doesn't look like an FFmpeg build: $f is missing" }
}

# The recorder needs AMF / NVENC / Desktop Duplication capture, the exporter needs libx264.
$ffmpeg = Join-Path $From 'bin\ffmpeg.exe'
$first = & $ffmpeg -hide_banner -version | Select-Object -First 1
if ($first -notmatch "version $([regex]::Escape($Version))\b") { throw "Expected FFmpeg $Version, found: $first" }
$encoders = & $ffmpeg -hide_banner -encoders | Out-String
$filters  = & $ffmpeg -hide_banner -filters | Out-String
foreach ($e in 'h264_amf', 'h264_nvenc', 'libx264', 'aac') {
  if ($encoders -notmatch "\s$e\s") { throw "This FFmpeg build has no $e encoder" }
}
foreach ($f in 'vsrc_amf', 'vpp_amf', 'ddagrab', 'amix') {
  if ($filters -notmatch "\s$f\s") { throw "This FFmpeg build has no $f filter" }
}

if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item (Join-Path $From 'bin\ffmpeg.exe'), (Join-Path $From 'bin\ffprobe.exe') $out
Copy-Item (Join-Path $From 'LICENSE') (Join-Path $out 'LICENSE.txt')
Copy-Item (Join-Path $From 'README.txt') (Join-Path $out 'README.txt')

@"
FFmpeg in Backtrack
===================

Backtrack's installer includes ffmpeg.exe and ffprobe.exe from FFmpeg $Version
(the "essentials" build by Gyan Doshi, https://www.gyan.dev/ffmpeg/builds/).
Backtrack runs them as separate programs to record, read and export video.

FFmpeg is free software, licensed here under the GNU General Public License,
version 3 (see LICENSE.txt). This build includes GPL libraries such as x264;
README.txt lists its configuration and the libraries in it.

Source code
  FFmpeg ${Version}, commit ${Commit}:
    https://github.com/FFmpeg/FFmpeg/commit/$Commit
  This build, with the versions of every library in it:
    https://github.com/GyanD/codexffmpeg/releases/tag/$Version
  FFmpeg releases:
    https://ffmpeg.org/download.html#releases

Backtrack itself is separate from FFmpeg and is MIT-licensed:
  https://github.com/kyle-barth/backtrack
"@ | Set-Content -Encoding utf8 (Join-Path $out 'NOTICE.txt')

Write-Host "Staged FFmpeg $Version in $out"
Get-ChildItem $out | ForEach-Object { '  {0,-12} {1,8:N1} MB' -f $_.Name, ($_.Length / 1MB) }
