# serial-json-repl.ps1 — line-oriented serial REPL for OBC firmware nodes.
#
# `espflash monitor` is DISPLAY-ONLY (it doesn't forward your typing to the
# device), so use this for the newline-delimited JSON command surface in
# HARDWARE-TEST-WALKTHROUGH.md Phase A / BENCH-WALKTHROUGH.md Stage 2.
#
# Usage:
#   powershell -File scripts\serial-json-repl.ps1 -Port COM7          # 115200 default
#   powershell -File scripts\serial-json-repl.ps1 -Port COM7 -Baud 115200
#
# List ports:  [System.IO.Ports.SerialPort]::GetPortNames()
# Notes: close espflash monitor first (it holds the port). Type a JSON command
# and press Enter to send; Backspace edits; paste works; Ctrl+C exits.

param(
  [Parameter(Mandatory = $true)][string]$Port,
  [int]$Baud = 115200
)

$sp = New-Object System.IO.Ports.SerialPort(
  $Port, $Baud, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
$sp.NewLine = "`n"
$sp.DtrEnable = $true   # USB-CDC devices won't transmit without DTR asserted
$sp.RtsEnable = $true

try { $sp.Open() }
catch {
  Write-Error "Cannot open ${Port}: $_"
  Write-Host "Available ports: $([System.IO.Ports.SerialPort]::GetPortNames() -join ', ')"
  exit 1
}

Write-Host "Connected to $Port @ $Baud." -ForegroundColor Green
Write-Host "Type a JSON command, press Enter to send. Ctrl+C to exit." -ForegroundColor Green
Write-Host 'e.g. {"id":"1","cmd":"capabilities"}' -ForegroundColor DarkGray

$line = ""
try {
  while ($true) {
    # Drain and print anything the node sent.
    $incoming = $sp.ReadExisting()
    if ($incoming) { Write-Host -NoNewline $incoming }

    # Non-blocking keyboard: build a local line, send on Enter.
    while ([Console]::KeyAvailable) {
      $k = [Console]::ReadKey($true)
      if ($k.Key -eq [ConsoleKey]::Enter) {
        Write-Host ""
        if ($line.Length -gt 0) { $sp.WriteLine($line); $line = "" }
      }
      elseif ($k.Key -eq [ConsoleKey]::Backspace) {
        if ($line.Length -gt 0) {
          $line = $line.Substring(0, $line.Length - 1)
          Write-Host -NoNewline "`b `b"
        }
      }
      elseif ($k.KeyChar) {
        $line += $k.KeyChar
        Write-Host -NoNewline $k.KeyChar
      }
    }
    Start-Sleep -Milliseconds 20
  }
}
finally {
  $sp.Close()
  Write-Host "`nDisconnected." -ForegroundColor Yellow
}
