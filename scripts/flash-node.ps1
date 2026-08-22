<#
.SYNOPSIS
  Flash the ESP32-S3 node firmware, with the four things that are easy to get
  wrong already done.

.DESCRIPTION
  Every one of these has cost a bench cycle:

    1. `export-esp.ps1` not sourced      -> LIBCLANG_PATH missing, build fails.
    2. CARGO_TARGET_DIR not set          -> "Too long output directory", after
                                            several minutes of compiling.
    3. The wrong working directory       -> export-esp.ps1 does not cd, so a
                                            relative `cd firmware\...` from
                                            $HOME fails and cargo runs nowhere.
    4. The wrong board                   -> on 2026-08-22 the node firmware was
                                            flashed onto the LoRa base station,
                                            because espflash picks a port when
                                            it is not told and the base station
                                            was the only one plugged in.

  The port is chosen by USB vendor id, not by position: the node is an ESP32-S3
  speaking over native USB-Serial-JTAG (VID 303A). The Heltecs are CP210x
  bridges (VID 10C4) and are refused, not preferred-against.

.PARAMETER Port
  Override the detected port. Use only if you know why.

.PARAMETER PortOnly
  Print what would be flashed and to where, then stop. Nothing is built and
  nothing is written.

.PARAMETER Features
  Passed through to cargo, e.g. -Features board-waveshare-21
#>
[CmdletBinding()]
param(
    [string]$Port,
    [switch]$PortOnly,
    [string]$Features
)

$ErrorActionPreference = 'Stop'

$repo  = Split-Path -Parent $PSScriptRoot
$crate = Join-Path $repo 'firmware\obc-esp32-s3'
if (-not (Test-Path (Join-Path $crate 'Cargo.toml'))) {
    throw "no crate at $crate -- is this script still inside the repo?"
}

# --- the port, by what the chip is, not by which one is present ---------------
function Get-NodePorts {
    Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '\(COM(\d+)\)' } |
        ForEach-Object {
            [pscustomobject]@{
                Port = ([regex]::Match($_.Name, '\(COM(\d+)\)').Groups[0].Value -replace '[()]', '')
                Name = $_.Name
                Id   = $_.PNPDeviceID
                Kind = if ($_.PNPDeviceID -match 'VID_303A') { 'native' }
                       elseif ($_.PNPDeviceID -match 'VID_(10C4|1A86|0403|067B)') { 'bridge' }
                       else { 'unknown' }
            }
        }
}

$ports = @(Get-NodePorts)
foreach ($p in $ports) { Write-Host ("  {0,-6} [{1}] {2}" -f $p.Port, $p.Kind, $p.Name) }

if (-not $Port) {
    $native = @($ports | Where-Object { $_.Kind -eq 'native' })
    if ($native.Count -eq 1) {
        $Port = $native[0].Port
        Write-Host "using $Port : the only native USB-Serial-JTAG port." -ForegroundColor Green
    } elseif ($native.Count -eq 0) {
        throw ("no native USB-Serial-JTAG port (VID 303A) found. The node is a XIAO " +
               "ESP32-S3 and has no bridge chip, so none of the ports above can be it. " +
               "Plug it in with a data cable. Do NOT flash a bridge port: on this bench " +
               "those are the Heltecs, and BENCH-PINOUT-CARDS.md Card 0 records the base " +
               "station gw-D8 among them.")
    } else {
        throw "more than one native USB port present. Pass -Port."
    }
}

if ($PortOnly) {
    Write-Host ""
    Write-Host "would flash: $crate"
    Write-Host "to:          $Port"
    Write-Host "-PortOnly: nothing was built and nothing was written."
    exit 0
}

# --- the environment ----------------------------------------------------------
$exportEsp = Join-Path $env:USERPROFILE 'export-esp.ps1'
if (-not (Test-Path $exportEsp)) {
    throw "$exportEsp not found. Run `espup install` first (BRINGUP.md 0.3)."
}
. $exportEsp
$env:CARGO_TARGET_DIR = 'C:\e'   # see BRINGUP.md 0.3: the path-length workaround

# --- build and flash ----------------------------------------------------------
Push-Location $crate
try {
    $cargoArgs = @('run')
    if ($Features) { $cargoArgs += @('--features', $Features) }
    $cargoArgs += @('--', '--port', $Port)
    Write-Host ""
    Write-Host "cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    Write-Host "(if the first build dies with an xtensa gcc 'internal compiler error'"
    Write-Host " on esp_lcd_panel_rgb.c, run this again once -- known flake.)"
    Write-Host ""
    & cargo @cargoArgs
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
