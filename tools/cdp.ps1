# Drive a running Backtrack debug build through WebView2's DevTools protocol (for testing).
# Start the app with:  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'
# Usage:
#   powershell -File tools/cdp.ps1 -Eval "location.hash"            # evaluate JS (promises awaited), prints JSON
#   powershell -File tools/cdp.ps1 -Shot C:\path\out.png            # screenshot the page
param([string]$Eval, [string]$Shot, [int]$Port = 9222, [int]$TimeoutSec = 120, [string]$Match = '')
$ErrorActionPreference = 'Stop'
$targets = Invoke-RestMethod "http://127.0.0.1:$Port/json"
$pages = $targets | Where-Object { $_.type -eq 'page' }
if ($Match) { $t = $pages | Where-Object { $_.url -like "*$Match*" } | Select-Object -First 1 }
else { $t = $pages | Where-Object { $_.url -notlike '*toast*' } | Select-Object -First 1 }
if (-not $t) { throw "no page target on port $Port" }
$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = New-Object System.Threading.CancellationTokenSource ([TimeSpan]::FromSeconds($TimeoutSec))
$ws.ConnectAsync([Uri]$t.webSocketDebuggerUrl, $ct.Token).Wait()

function Invoke-Cdp([int]$id, [string]$method, $params) {
  $msg = @{ id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 10 -Compress
  $bytes = [Text.Encoding]::UTF8.GetBytes($msg)
  $ws.SendAsync((New-Object 'ArraySegment[byte]' (,$bytes)), 'Text', $true, $ct.Token).Wait()
  $buf = New-Object byte[] 1048576
  while ($true) {
    $ms = New-Object IO.MemoryStream
    do {
      $r = $ws.ReceiveAsync((New-Object 'ArraySegment[byte]' (,$buf)), $ct.Token).Result
      $ms.Write($buf, 0, $r.Count)
    } while (-not $r.EndOfMessage)
    $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
    if ($text -match ('^\{"id":' + $id + '[,}]')) { return ($text | ConvertFrom-Json) }
  }
}

if ($Eval) {
  $res = Invoke-Cdp 1 'Runtime.evaluate' @{ expression = $Eval; awaitPromise = $true; returnByValue = $true; replMode = $true }
  if ($res.result.exceptionDetails) { Write-Output ("EXCEPTION: " + ($res.result.exceptionDetails | ConvertTo-Json -Depth 5 -Compress)) }
  else { Write-Output ($res.result.result.value | ConvertTo-Json -Depth 8 -Compress) }
}
if ($Shot) {
  $res = Invoke-Cdp 2 'Page.captureScreenshot' @{ format = 'png' }
  [IO.File]::WriteAllBytes($Shot, [Convert]::FromBase64String($res.result.data))
  Write-Output "saved $Shot"
}
$ws.Dispose()
