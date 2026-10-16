$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$exe=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\ClaudeGuard.exe'
$cli=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\cli\claude-guard.exe'
$identity=[Environment]::UserDomainName+'\'+[Environment]::UserName
$hash=[Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($identity))
$pipeName='ClaudeGuard-'+([Convert]::ToHexString($hash).Substring(0,20))
$crlf=[string][char]13+[char]10
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class GuardQa {
 [DllImport("user32.dll",CharSet=CharSet.Unicode)]public static extern IntPtr FindWindow(string c,string n);
 [DllImport("user32.dll")]public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
function Guard([string]$Action='status'){
 $p=[IO.Pipes.NamedPipeClientStream]::new('.',$pipeName,[IO.Pipes.PipeDirection]::InOut)
 try{$p.Connect(1000);$b=[Text.Encoding]::UTF8.GetBytes('{"action":"'+$Action+'"}'+[char]10);$p.Write($b,0,$b.Length);$p.Flush();$r=[IO.StreamReader]::new($p);return ($r.ReadLine()|ConvertFrom-Json)}finally{$p.Dispose()}
}
function Ready{
 $clock=[Diagnostics.Stopwatch]::StartNew()
 do{try{$r=Guard;if($r.state.allowed -and $r.state.country -eq 'US'){return $r.state}}catch{}
 Start-Sleep -Milliseconds 200}while($clock.Elapsed.TotalSeconds -lt 45)
 throw 'Guard did not verify the real connection'
}
function Mode([string]$value){
 $p=[IO.Pipes.NamedPipeClientStream]::new('.','verge-mihomo',[IO.Pipes.PipeDirection]::InOut)
 try{
  $p.Connect(1500);$body='{"mode":"'+$value+'"}'
  $message=[string]::Join($crlf,@('PATCH /configs HTTP/1.1','Host: localhost','Content-Type: application/json',('Content-Length: '+[Text.Encoding]::UTF8.GetByteCount($body)),'Connection: close','',$body))
  $b=[Text.Encoding]::UTF8.GetBytes($message);$p.Write($b,0,$b.Length);$p.Flush()
  $reader=[IO.StreamReader]::new($p);$line=$reader.ReadLine()
  if($line -notmatch '^HTTP/1\.[01] 2\d\d'){throw "Clash mode request failed: $line"}
 }finally{$p.Dispose()}
}
function Tunnel{
 $c=[Net.Sockets.TcpClient]::new('127.0.0.1',17890);$s=$c.GetStream();$s.ReadTimeout=5000;$s.WriteTimeout=5000
 $b=[Text.Encoding]::ASCII.GetBytes([string]::Join($crlf,@('CONNECT api.country.is:443 HTTP/1.1','Host: api.country.is:443','','')))
 $s.Write($b,0,$b.Length);$text=''
 while(-not $text.EndsWith($crlf+$crlf)){$n=$s.ReadByte();if($n -lt 0){break};$text+=[char]$n;if($text.Length -gt 16384){throw 'Oversized response'}}
 if($text -notmatch '^HTTP/1.1 200'){$c.Dispose();throw "CONNECT failed: $text"};return $c
}
function Closed($client){
 try{$b=New-Object byte[] 1;$n=$client.GetStream().Read($b,0,1);if($n -ne 0){throw 'Tunnel remained open'}}catch [IO.IOException]{if($_.Exception.GetBaseException() -is [Net.Sockets.SocketException] -and $_.Exception.GetBaseException().SocketErrorCode -eq [Net.Sockets.SocketError]::TimedOut){throw 'Tunnel did not close before timeout'}}finally{$client.Dispose()}
}
$result=[ordered]@{}
$ready=Ready;$result.realUsProxy=($ready.country -eq 'US')
$beforeConfig=Get-Content "$env:APPDATA/ClaudeGuard/settings.json" -Raw | ConvertFrom-Json
try{
 $tunnel=Tunnel
 Mode 'rule'
 $timer=[Diagnostics.Stopwatch]::StartNew()
 do{$blocked=(Guard).state; if(-not $blocked.allowed -and $blocked.reasonCode -eq 'mode'){break};Start-Sleep -Milliseconds 50}while($timer.Elapsed.TotalSeconds -lt 5)
 if($blocked.allowed -or $blocked.reasonCode -ne 'mode'){throw 'Rule mode was not blocked'}
 Closed $tunnel
 $result.ruleModeBlocked=$true;$result.existingTunnelRevoked=$true;$result.modeDetectionMilliseconds=$timer.Elapsed.TotalMilliseconds
}finally{Mode 'global';Guard 'recheck'|Out-Null}
Ready|Out-Null
& $cli -- --version | Set-Content (Join-Path $root 'dist/qa/claude-version.txt')
if($LASTEXITCODE -ne 0){throw 'Protected Claude Code entry failed'};$result.claudeEntry=$true
$hwnd=[GuardQa]::FindWindow('ClaudeGuard.Rust.Island',$null)
if($hwnd -eq [IntPtr]::Zero){throw 'Native island missing'}
$before=(Guard).state.verifiedAt
[GuardQa]::SendMessage($hwnd,0x218,[IntPtr]18,[IntPtr]::Zero)|Out-Null
$after=(Guard).state
if($after.allowed -and $after.verifiedAt -eq $before){throw 'Resume event did not invalidate verification'}
Ready|Out-Null;$result.resumeMessageInvalidation=$true
for($i=0;$i -lt 2;$i++){
 [GuardQa]::SendMessage($hwnd,0x8001,[IntPtr]3,[IntPtr]::Zero)|Out-Null
 $settings=Get-Content "$env:APPDATA/ClaudeGuard/settings.json" -Raw|ConvertFrom-Json
 $scheduler=New-Object -ComObject Schedule.Service;$scheduler.Connect();$entry=$false
 try{$entry=$scheduler.GetFolder('\').GetTask($pipeName).Enabled}catch {if($_.Exception.GetBaseException().HResult -ne -2147024894){throw}}
 if([bool]$entry -ne $settings.autoStart){throw 'Startup setting did not persist'}
}
$settings=Get-Content "$env:APPDATA/ClaudeGuard/settings.json" -Raw|ConvertFrom-Json
if($settings.autoStart -ne $beforeConfig.autoStart){throw 'Startup preference was not restored'};$result.startupToggle=$true
$duplicates=@(1..2|ForEach-Object{Start-Process -FilePath $exe -WindowStyle Hidden -PassThru})
foreach($p in $duplicates){if(-not $p.WaitForExit(5000)){throw 'Second instance stayed alive'}}
if(@(Get-Process ClaudeGuard|Where-Object Path -eq $exe).Count -ne 1){throw 'Single-instance check failed'};$result.singleInstance=$true
Ready|Out-Null
$tunnel=Tunnel
$guard=Get-Process ClaudeGuard|Where-Object Path -eq $exe
try{
 Stop-Process -Id $guard.Id -Force
 Closed $tunnel
 try{$socket=[Net.Sockets.TcpClient]::new();$socket.Connect('127.0.0.1',17890);$socket.Dispose();throw 'Proxy accepted a connection after guard termination'}catch [Net.Sockets.SocketException]{}
}finally{if(-not (Get-Process ClaudeGuard -ErrorAction SilentlyContinue|Where-Object Path -eq $exe)){Start-Process -FilePath $exe -ArgumentList '--autostart' -WindowStyle Hidden}}
Ready|Out-Null
$result.forcedTerminationClosesConnections=$true
$result.timestamp=[DateTime]::UtcNow.ToString('o')
$result|ConvertTo-Json|Set-Content (Join-Path $root 'dist/qa/installed-integration.json')
$result|ConvertTo-Json

