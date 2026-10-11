# M0-Test (Windows): Netzwerkdaten anzeigen und Ping zum Mac messen. Ändert nichts am System.
# Aufruf (PowerShell):  powershell -ExecutionPolicy Bypass -File .\pc_netz.ps1 -Mac 192.168.178.60 [-Anzahl 120]
param([Parameter(Mandatory=$true)][string]$Mac, [int]$Anzahl = 120)
Write-Host "== Netzwerk (PC) =="
Get-NetAdapter | Where-Object Status -eq 'Up' | Format-Table -AutoSize Name, InterfaceDescription, LinkSpeed
$w = netsh wlan show interfaces 2>$null
if ($w) { $w | Select-String -Pattern 'SSID|Kanal|Channel|Funktyp|Radio type|Empfangsrate|Receive rate|Übertragungsrate|Transmit rate|Signal|Band' | ForEach-Object { "  " + $_.Line.Trim() } }
Write-Host "== Ping Mac ($Mac), $Anzahl x =="
$out = ping.exe -n $Anzahl -w 1000 $Mac
$ms = foreach ($l in $out) { if ($l -match '(?:Zeit|time)([=<])(\d+)\s*ms') { [double]$matches[2] } }
$lost = $Anzahl - @($ms).Count
if (@($ms).Count -eq 0) { Write-Host "  keine Antworten (Mac-Firewall im Tarnmodus? IP richtig?)"; exit 1 }
$s = $ms | Sort-Object
function P($p) { $s[[math]::Min($s.Count-1, [math]::Floor($s.Count*$p))] }
"  n={0}  min={1}  p50={2}  p95={3}  p99={4}  max={5} ms  verloren={6}" -f $s.Count, $s[0], (P 0.5), (P 0.95), (P 0.99), $s[-1], $lost
