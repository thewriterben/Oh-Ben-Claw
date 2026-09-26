<#
.SYNOPSIS
  Build (and optionally flash) the camera firmware, from main.

.DESCRIPTION
  The camera build is firmware/obc-esp32-s3-camera: the node's own sources plus
  the esp32-camera IDF component, which cannot be added to firmware/obc-esp32-s3
  without reaching the live node's build (see that crate's Cargo.toml). This
  script sets the three things a camera build has got wrong on this bench:

    1. The sdkconfig overlay       -> both files named, absolute, per board.
                                      Naming only the overlay drops the main
                                      stack size; naming none means no PSRAM.
                                      A leftover value from another shell is
                                      overwritten, not trusted.
    2. A target dir of its own      -> C:\ec-cam. Sharing one with the node
                                      build shares one esp-idf build
                                      (2026-09-16); build.rs refuses it.
    3. The board being flashed      -> with -Port, the chip's MAC is read first
                                      and obc-esp32-s3-001, the live mesh node,
                                      is refused.

  The PSRAM mode is also checked at compile time (camera.rs), so a wrong
  overlay is a compile error rather than a boot loop.

.PARAMETER Board
  xiao-sense (OCT PSRAM) or lilygo-tcam-v11 (QUAD PSRAM, V1.0/V1.1 only).

.PARAMETER Port
  Flash to this port and open the monitor. Omit to build only. Find the port
  with scripts/which_esp32.ps1.

.PARAMETER TargetDir
  Defaults to C:\ec-cam. Must not be the node build's target dir.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('xiao-sense', 'lilygo-tcam-v11')][string]$Board,
    [string]$Port,
    [string]$TargetDir = 'C:\ec-cam'
)

$ErrorActionPreference = 'Stop'

$repo  = Split-Path -Parent $PSScriptRoot
$node  = Join-Path $repo 'firmware\obc-esp32-s3'
$crate = Join-Path $repo 'firmware\obc-esp32-s3-camera'
if (-not (Test-Path (Join-Path $crate 'Cargo.toml'))) {
    throw "no crate at $crate -- is this script still inside the repo?"
}

$feature, $overlay = switch ($Board) {
    'xiao-sense'      { 'board-xiao-sense',         'sdkconfig.defaults.camera-xiao-sense' }
    'lilygo-tcam-v11' { 'board-lilygo-tcam-s3-v11', 'sdkconfig.defaults.camera-lilygo-tcam-v11' }
}

# The live mesh node. Same value as ROSTER in identity_map.rs and $Known in
# which_esp32.ps1 (tests/firmware_identity_roster.rs keeps those in step).
$LiveMac = '64:E8:33:7E:BB:98'

# --- the environment ----------------------------------------------------------
$exportEsp = Join-Path $env:USERPROFILE 'export-esp.ps1'
if (-not (Test-Path $exportEsp)) {
    throw "$exportEsp not found. Run `espup install` first (BRINGUP.md 0.3)."
}
. $exportEsp

$env:CARGO_TARGET_DIR = $TargetDir
$env:ESP_IDF_SDKCONFIG_DEFAULTS = @(
    (Join-Path $node 'sdkconfig.defaults'),
    (Join-Path $node $overlay)
) -join ';'

Write-Host "board    $Board  (--features $feature)"
Write-Host "overlay  $env:ESP_IDF_SDKCONFIG_DEFAULTS"
Write-Host "target   $env:CARGO_TARGET_DIR"

# --- which chip is on the port ------------------------------------------------
if ($Port) {
    # espflash logs to stderr (including a "new version available" notice), and
    # Windows PowerShell turns redirected stderr into error records, which
    # $ErrorActionPreference = 'Stop' makes fatal. Relax it for this one call and
    # keep the text; the MAC regex below is the real check. (2026-09-26: the
    # first run died here on the version notice, before anything was built.)
    $eap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $info = & espflash board-info --port $Port 2>&1 | ForEach-Object { "$_" } | Out-String
    } finally {
        $ErrorActionPreference = $eap
    }
    $mac = [regex]::Match($info, '(?i)MAC address:\s*([0-9a-f]{2}(:[0-9a-f]{2}){5})')
    if (-not $mac.Success) {
        Write-Host $info
        throw "could not read a MAC from $Port. Nothing was built or written."
    }
    $found = $mac.Groups[1].Value.ToUpper()
    if ($found -eq $LiveMac) {
        throw ("$Port is obc-esp32-s3-001 ($found), the live mesh node. Nothing was written. " +
               "Run scripts/which_esp32.ps1 and pass the camera board's port.")
    }
    Write-Host "port     $Port  MAC $found" -ForegroundColor Green
}

# --- build, and flash if asked --------------------------------------------------
Push-Location $crate
try {
    $cargoArgs = if ($Port) { @('run') } else { @('build') }
    $cargoArgs += @('--release', '--features', $feature)
    if ($Port) { $cargoArgs += @('--', '--port', $Port) }
    Write-Host ""
    Write-Host "cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    Write-Host ""
    & cargo @cargoArgs
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
