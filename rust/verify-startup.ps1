$ErrorActionPreference='Stop'
$identity=[Environment]::UserDomainName+'\'+[Environment]::UserName
$hash=[Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($identity))
$taskName='ClaudeGuard-'+[Convert]::ToHexString($hash).Substring(0,20)
$service=New-Object -ComObject Schedule.Service
$service.Connect()
$folder=$service.GetFolder('\')
$task=$folder.GetTask($taskName)
$definition=$task.Definition
$settingsFile=Join-Path $env:APPDATA 'ClaudeGuard/settings.json'
$marker=Join-Path $env:LOCALAPPDATA 'Programs/ClaudeGuard/data-root.txt'
if(Test-Path -LiteralPath $marker){$settingsFile=Join-Path (Get-Content $marker -Raw).Trim() 'settings.json'}
$beforeSettings=Get-Content -LiteralPath $settingsFile -Raw|ConvertFrom-Json
$exe=Join-Path $env:LOCALAPPDATA 'Programs/ClaudeGuard/ClaudeGuard.exe'
if(-not $task.Enabled -or $definition.Triggers.Item(1).Type -ne 9){throw 'Enabled logon task missing'}
if($definition.Actions.Item(1).Path -ne $exe){throw 'Wrong startup executable'}
if($definition.Principal.RunLevel -ne 0 -or $definition.Principal.LogonType -ne 3){throw 'Startup must use current interactive user without elevation'}
if($definition.Settings.RestartCount -ne 3){throw 'Missing failure retries'}
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class StartupQa {
 [DllImport("user32.dll",CharSet=CharSet.Unicode)]public static extern IntPtr FindWindow(string c,string n);
 [DllImport("user32.dll")]public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
}
'@
$hwnd=[StartupQa]::FindWindow('ClaudeGuard.Rust.Island',$null)
if($hwnd -eq [IntPtr]::Zero){throw 'Guard window missing'}
# The exact menu action must remove/recreate both the OS task and saved choice.
try {
 [StartupQa]::SendMessage($hwnd,0x8001,[IntPtr]3,[IntPtr]::Zero)|Out-Null
 $off=Get-Content -LiteralPath $settingsFile -Raw|ConvertFrom-Json
 if($off.autoStart){throw 'Menu did not disable startup'}
 $found=$null
 try{$found=$folder.GetTask($taskName)}catch {if($_.Exception.GetBaseException().HResult -ne -2147024894){throw}}
 if($found -and $found.Enabled){throw 'Disabled menu left task enabled'}
 if((Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name ClaudeGuard -ErrorAction SilentlyContinue).ClaudeGuard){throw 'Legacy Run entry remains'}
}finally {
 $current=Get-Content -LiteralPath $settingsFile -Raw|ConvertFrom-Json
 if(-not $current.autoStart){[StartupQa]::SendMessage($hwnd,0x8001,[IntPtr]3,[IntPtr]::Zero)|Out-Null}
}
$task=$folder.GetTask($taskName)
if(-not $task.Enabled){throw 'Menu did not restore startup'}
$running=Get-Process ClaudeGuard|Where-Object Path -eq $exe
[StartupQa]::SendMessage($hwnd,0x8001,[IntPtr]4,[IntPtr]::Zero)|Out-Null
foreach($p in $running){if(-not $p.WaitForExit(5000)){throw 'Exit menu did not exit'}}
try {
 $task.Run($null)|Out-Null
 $clock=[Diagnostics.Stopwatch]::StartNew()
 do{Start-Sleep -Milliseconds 100;$started=Get-Process ClaudeGuard -ErrorAction SilentlyContinue|Where-Object Path -eq $exe}while(-not $started -and $clock.Elapsed.TotalSeconds -lt 15)
 if(-not $started){throw 'Windows Scheduler did not start the installed application'}
 Start-Sleep -Seconds 2
 $state=& (Join-Path (Split-Path $exe) 'cli/claude-guard.exe') --status|ConvertFrom-Json
 if(-not $state.ok){throw 'Scheduled app is not responding'}
 $nativeWindow=[StartupQa]::FindWindow('ClaudeGuard.Rust.Island',$null)
 if($nativeWindow -eq [IntPtr]::Zero){throw 'Scheduled app did not create its island'}
 # Exercise persistence from the actual Scheduler process as well.
 for($i=0;$i -lt 2;$i++){
   [StartupQa]::SendMessage($nativeWindow,0x8001,[IntPtr]3,[IntPtr]::Zero)|Out-Null
   $saved=Get-Content -LiteralPath $settingsFile -Raw|ConvertFrom-Json
   if($saved.autoStart -ne ($i -eq 1)){throw 'Scheduled process used a different data directory'}
 }
 $saved=Get-Content -LiteralPath $settingsFile -Raw|ConvertFrom-Json
 foreach($key in @('preferredNode','claudePath','soundEnabled','islandLeft','islandTop')){if($saved.$key -ne $beforeSettings.$key){throw "Scheduled startup changed preference: $key"}}
}finally {
 if(-not (Get-Process ClaudeGuard -ErrorAction SilentlyContinue|Where-Object Path -eq $exe)){Start-Process -FilePath $exe -ArgumentList '--autostart' -WindowStyle Hidden}
}
[ordered]@{logonTrigger=$true;delay=$definition.Triggers.Item(1).Delay;failureRetries=$definition.Settings.RestartCount;menuToggle=$true;exitMenu=$true;actualSchedulerLaunch=$true;scheduledProcessSharesOriginalPreferences=$true;interactiveUserNoElevation=$true;actualNewLogon='pending';timestamp=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json|Tee-Object (Join-Path $PSScriptRoot 'dist/qa/startup.json')

