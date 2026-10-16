param([switch]$RestartDesktop,[string]$Output='desktop-blocking.json')
$ErrorActionPreference='Stop'
$cli=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\cli\claude-guard.exe'
$exe=(Get-AppxPackage -Name Claude).InstallLocation+'\app\claude.exe'
$crlf=[string][char]13+[char]10
function State { (& $cli --status | ConvertFrom-Json).state }
function Desktop { @(Get-Process -Name claude -ErrorAction SilentlyContinue | Where-Object Path -eq $exe) }
function Connections {
 $ids=@((Desktop).Id)
 @(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {$_.OwningProcess -in $ids -and $_.State -eq 'Established' -and $_.RemoteAddress -eq '127.0.0.1' -and $_.RemotePort -eq 17890})
}
function Mode([string]$value) {
 $pipe=[IO.Pipes.NamedPipeClientStream]::new('.','verge-mihomo',[IO.Pipes.PipeDirection]::InOut)
 try {
  $pipe.Connect(1500);$body='{"mode":"'+$value+'"}'
  $message=[string]::Join($crlf,@('PATCH /configs HTTP/1.1','Host: localhost','Content-Type: application/json',('Content-Length: '+[Text.Encoding]::UTF8.GetByteCount($body)),'Connection: close','',$body))
  $bytes=[Text.Encoding]::UTF8.GetBytes($message);$pipe.Write($bytes,0,$bytes.Length);$pipe.Flush()
  $reader=[IO.StreamReader]::new($pipe)
  if($reader.ReadLine() -notmatch '^HTTP/1\.[01] 2\d\d'){throw 'Clash mode change failed'}
 }finally{$pipe.Dispose()}
}
function Ready {
 for($i=0;$i -lt 225;$i++){if((State).allowed){return};Start-Sleep -Milliseconds 200}
 throw 'Network did not become ready'
}
Ready
$before=State
if($before.mode -ne 'global'){throw 'This verification requires an initially verified global connection'}
for($i=0;$i -lt 30 -and -not (Connections).Count;$i++){Start-Sleep -Seconds 1}
$result=[ordered]@{timestamp=[DateTimeOffset]::Now.ToString('o');guardConnectionsBefore=(Connections).Count}
if(-not $result.guardConnectionsBefore){throw 'No live Desktop connection to revoke'}
$pidBefore=@((Desktop).Id)
try {
 Mode 'rule'
 $timer=[Diagnostics.Stopwatch]::StartNew()
 do{$blocked=State;if(-not $blocked.allowed -and $blocked.reasonCode -eq 'mode'){break};Start-Sleep -Milliseconds 50}while($timer.Elapsed.TotalSeconds -lt 5)
 if($blocked.allowed -or $blocked.reasonCode -ne 'mode'){throw 'Rule mode was not blocked'}
 $result.modeDetectionMs=[math]::Round($timer.Elapsed.TotalMilliseconds)
 Start-Sleep -Seconds 3
 $result.guardConnectionsAfter=(Connections).Count
 $result.originalSessionKept=@((Desktop).Id | Where-Object {$_ -in $pidBefore}).Count -gt 0
 if($result.guardConnectionsAfter -ne 0 -or -not $result.originalSessionKept){throw 'Existing-session network revocation failed'}
 $launch=(& $cli --desktop | ConvertFrom-Json)
 $result.dedicatedEntryRejected=-not $launch.ok
 if(-not $result.dedicatedEntryRejected){throw 'Blocked dedicated entry was allowed'}
 $folder=Split-Path ([IO.Path]::GetFullPath($Output)) -Parent
 & (Join-Path $PSScriptRoot 'verify-ui.ps1') -Output (Join-Path $folder 'alert-expanded.png') | Out-Null
 Start-Sleep -Seconds 12
 & (Join-Path $PSScriptRoot 'verify-ui.ps1') -Output (Join-Path $folder 'alert-collapsed.png') | Out-Null
 if($RestartDesktop){
  # Opt-in only: caller must have confirmed there are no active Desktop tasks.
  Desktop | Stop-Process -Force
  Start-Sleep -Seconds 1
  $root=(Get-Content "$env:LOCALAPPDATA\Programs\ClaudeGuard\data-root.txt" -Raw).Trim()
  $log=Join-Path $root 'guard.log'
  $previous=@(Select-String -LiteralPath $log -SimpleMatch '已阻止 Claude Desktop 的新启动').Count
  Start-Process -FilePath 'explorer.exe' -ArgumentList 'shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude' -WindowStyle Hidden
  $timer.Restart()
  do{Start-Sleep -Milliseconds 50;$observed=@(Select-String -LiteralPath $log -SimpleMatch '已阻止 Claude Desktop 的新启动').Count -gt $previous}while(-not $observed -and $timer.Elapsed.TotalSeconds -lt 5)
  Start-Sleep -Milliseconds 500
  $result.directEntryStopped=$observed -and (Desktop).Count -eq 0
  $result.directEntryStopMs=[math]::Round($timer.Elapsed.TotalMilliseconds)
  if(-not $result.directEntryStopped){throw 'Direct Desktop launch was not stopped'}
 }
}finally{
 Mode 'global'
 Ready
 if($RestartDesktop -and -not (Desktop).Count){$null=(& $cli --desktop)}
}
$result | ConvertTo-Json | Set-Content -LiteralPath $Output -Encoding utf8
$result | ConvertTo-Json
