# install.ps1 — shipped inside the Windows release .zip as `install.ps1`.
# Installs the archive's our.exe/ourc.exe to -BinDir (default
# $HOME\.local\bin) as BOTH the current copy and a versioned sibling
# (CHANNELS.md §2):
#
#   <bin>\our.exe, ourc.exe           — last installed wins
#   <bin>\our-<ver>.exe, ourc-<ver>.exe — stays; the dispatch pool a
#                                       [project] toolchain req selects
#                                       from
#
# The binaries are self-contained (stdlib + runtime are embedded,
# INSTALL.md §1) — nothing else is installed, nothing else is needed.
# Safe to re-run: installing the same version refreshes the copies in
# place; old versioned siblings are never removed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File .\install.ps1 [-BinDir <dir>]
param(
    [string]$BinDir = (Join-Path $HOME '.local\bin')
)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$ver = (& (Join-Path $here 'our.exe') --version) -replace '^our ',''
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
foreach ($b in 'our','ourc') {
    Copy-Item (Join-Path $here "$b.exe") (Join-Path $BinDir "$b-$ver.exe") -Force
    Copy-Item (Join-Path $here "$b.exe") (Join-Path $BinDir "$b.exe") -Force
}
Write-Host "installed $ver -> $BinDir\{our,ourc}.exe (+ versioned siblings)"

# INSTALL.md §5 — warn, don't fail: builds need clang (LLVM or VS
# Build Tools) on PATH; remote registry fetches need curl.
if (-not (Get-Command clang -ErrorAction SilentlyContinue)) {
    Write-Host 'note: clang not found on PATH — required for `our build`'
}
if (-not (Get-Command curl -ErrorAction SilentlyContinue)) {
    Write-Host 'note: curl not found on PATH — required for remote registries'
}
$pathDirs = ($env:Path -split ';') | ForEach-Object { $_.TrimEnd('\') }
if ($pathDirs -notcontains $BinDir.TrimEnd('\')) {
    Write-Host "note: $BinDir is not on PATH"
}
