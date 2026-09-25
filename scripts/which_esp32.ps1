# which_esp32.ps1 -- name every attached ESP32-S3 before you flash one.
#
# Why this exists: on 2026-09-16 the only ESP32-S3 on the bench was the LIVE mesh
# node, and the plan said "flash the spare XIAO". Nothing on the desk or on the
# screen distinguished them -- the two boards' MACs differ only in their last
# three bytes. "Do not flash 001" was prose; this is the gate.
#
# ESP32-S3 native USB (VID_303A) puts the chip's base MAC in the composite
# device's instance path, so a board can be identified WITHOUT opening its serial
# port -- which matters, because the brain may hold the live node's port open and
# because probing a running node is not free.
#
# The table below is checked against the firmware's own roster by
# `tests/firmware_identity_roster.rs`. Source of truth is
# `firmware/obc-esp32-s3/src/identity_map.rs`; if they disagree, that file wins
# and this one is stale. Add boards there first.

$Known = @{
  '64:E8:33:7E:BB:98' = @{
    Id     = 'obc-esp32-s3-001'
    Status = 'LIVE'
    Note   = 'the mesh node. Jumpers to bridge gw-D8. DO NOT FLASH.'
  }
  '64:E8:33:7E:7E:04' = @{
    Id     = 'obc-esp32-s3-002'
    Status = 'BENCH'
    Note   = 'camera node (XIAO Sense). Bring-up board -- safe to reflash.'
  }
  '48:CA:43:4B:95:F8' = @{
    Id     = 'obc-esp32-s3-003'
    Status = 'BENCH'
    Note   = 'LILYGO T-CameraPlus-S3 V1.1 (OV5640). Camera works; safe to reflash.'
  }
  '64:E8:33:7F:84:CC' = @{
    Id     = 'obc-esp32-s3-004'
    Status = 'BENCH'
    Note   = 'spare XIAO ESP32S3. Nothing depends on it -- safe to reflash.'
  }
  'AC:27:6E:A8:4D:E4' = @{
    Id     = 'obc-esp32-s3-005'
    Status = 'BENCH'
    Note   = 'second camera node (XIAO Sense, OV3660). Captures; safe to reflash.'
  }
}

Write-Output "=== ESP32-S3 boards on native USB (VID_303A) ==="

$all = Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
  Where-Object { $_.InstanceId -match 'VID_303A' }

if (-not $all) {
  Write-Output "  (none attached)"
  exit 0
}

# Composite parents carry the MAC; the COM-port child carries the port name.
$parents = $all | Where-Object { $_.InstanceId -match '^USB\\VID_303A&PID_[0-9A-F]+\\' }

if (-not $parents) {
  Write-Output "  !! found VID_303A devices but no composite parent with a serial."
  $all | ForEach-Object { Write-Output ("     " + $_.InstanceId) }
  exit 2
}

$verdicts = @()

foreach ($p in $parents) {
  $mac = ($p.InstanceId -split '\\')[-1]

  $com = '(none)'
  foreach ($d in $all) {
    $par = (Get-PnpDeviceProperty -InstanceId $d.InstanceId -KeyName 'DEVPKEY_Device_Parent' -ErrorAction SilentlyContinue).Data
    if ($par -eq $p.InstanceId -and $d.FriendlyName -match '\((COM\d+)\)') {
      $com = $Matches[1]
    }
  }

  $hit = $null
  foreach ($k in $Known.Keys) { if ($k -ieq $mac) { $hit = $Known[$k] } }

  if ($hit) {
    $verdicts += [pscustomobject]@{ MAC=$mac; COM=$com; Id=$hit.Id; Verdict=$hit.Status; Note=$hit.Note }
  } else {
    # Matches the firmware's fallback in identity_map.rs: last three bytes.
    $tail = (($mac -split ':')[3..5] -join '').ToLower()
    $verdicts += [pscustomobject]@{ MAC=$mac; COM=$com; Id="obc-esp32-s3-$tail"; Verdict='UNKNOWN'
                                    Note='not on the roster -- it will self-name from its MAC' }
  }
}

foreach ($v in $verdicts) {
  $mark = if ($v.Verdict -eq 'LIVE') { '  [!!]' } else { '  [ok]' }
  Write-Output ("{0} {1}  {2}  {3}" -f $mark, $v.COM.PadRight(6), $v.MAC, $v.Id)
  Write-Output ("       " + $v.Note)
}

Write-Output ""
$live      = @($verdicts | Where-Object { $_.Verdict -eq 'LIVE' })
$flashable = @($verdicts | Where-Object { $_.Verdict -ne 'LIVE' })

Write-Output ("  live nodes attached:   {0}   ({1})" -f $live.Count, (($live.COM) -join ', '))
Write-Output ("  flashable candidates:  {0}   ({1})" -f $flashable.Count, (($flashable.COM) -join ', '))

if ($flashable.Count -eq 0) {
  Write-Output ""
  Write-Output "  VERDICT: nothing safe to flash. Every attached S3 is a live node."
  exit 1
}
if ($flashable.Count -gt 1) {
  Write-Output ""
  Write-Output "  VERDICT: more than one candidate. Unplug until there is one, or pass the port by hand."
  exit 1
}

Write-Output ""
Write-Output ("  VERDICT: flash {0} ({1}) only. Pass --port {0} explicitly; do NOT let espflash autodetect." -f $flashable[0].COM, $flashable[0].Id)
exit 0
