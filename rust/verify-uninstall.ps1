$ErrorActionPreference='Stop'
$installer=Join-Path $PSScriptRoot 'dist/ClaudeGuard.Setup.exe'
$install=Join-Path $env:LOCALAPPDATA 'Programs/ClaudeGuard'
$settings=Join-Path $env:APPDATA 'ClaudeGuard/settings.json'
$before=[IO.File]::ReadAllBytes($settings)
$runKey='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runValue=(Get-ItemProperty $runKey -Name ClaudeGuard -ErrorAction SilentlyContinue).ClaudeGuard
$identity=[Environment]::UserDomainName+'\'+[Environment]::UserName
$hash=[Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($identity))
$taskName='ClaudeGuard-'+[Convert]::ToHexString($hash).Substring(0,20)
$scheduler=New-Object -ComObject Schedule.Service;$scheduler.Connect();$folder=$scheduler.GetFolder('\')
$taskXml=$null
try{$taskXml=$folder.GetTask($taskName).Xml}catch {if($_.Exception.GetBaseException().HResult -ne -2147024894){throw}}
$shortcuts=@([Environment]::GetFolderPath('DesktopDirectory'),[Environment]::GetFolderPath('Programs')) | ForEach-Object {Join-Path $_ 'Claude 网络守卫.lnk'}
$keys=@('HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.claudeguard.bridge','HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.claudeguard.bridge','HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ClaudeGuard')
$result=[ordered]@{}
function Setup([string[]]$Arguments){
    $p=Start-Process -FilePath $installer -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    $p.WaitForExit()
    if($p.ExitCode -ne 0){throw "Installer failed: $($Arguments -join ' ')"}
}
try{
    Setup @('--quiet','--uninstall')
    if(Test-Path -LiteralPath $install){throw 'Install directory remains after uninstall'}
    foreach($key in $keys){if(Test-Path $key){throw 'Application registry key remains'}}
    foreach($shortcut in $shortcuts){if(Test-Path -LiteralPath $shortcut){throw 'Application shortcut remains'}}
    if((Get-ItemProperty $runKey -Name ClaudeGuard -ErrorAction SilentlyContinue).ClaudeGuard){throw 'Autostart entry remains'}
    $found=$null
    try{$found=$folder.GetTask($taskName)}catch {if($_.Exception.GetBaseException().HResult -ne -2147024894){throw}}
    if($found){throw 'Startup task remains after uninstall'}
    $userPath=[Environment]::GetEnvironmentVariable('PATH','User')
    if($userPath -split ';' | Where-Object {$_.TrimEnd('\','/') -ieq (Join-Path $install 'cli')}){throw 'CLI user PATH entry remains'}
    if(-not (Test-Path -LiteralPath $settings)){throw 'User settings were unexpectedly removed'}
    $result.uninstallRemovedProgramAndRegistrations=$true
    $result.uninstallPreservedUserSettings=$true
    $result.uninstallRemovedScheduledTask=$true
    Setup @('--quiet')
    $fresh=Get-Content -LiteralPath $settings -Raw|ConvertFrom-Json
    if(-not $fresh.autoStart -or $fresh.soundEnabled){throw 'Fresh install defaults are incorrect'}
    $result.freshDefaultsAutostartOnSoundOff=$true
}finally{
    if(-not (Test-Path -LiteralPath (Join-Path $install 'ClaudeGuard.exe')) -or -not (Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ClaudeGuard')){Setup @('--quiet')}
    $cli=Join-Path $install 'cli/claude-guard.exe'
    # Restore the pre-test preferences without changing browser/Claude processes.
    $identity=[Environment]::UserDomainName+'\'+[Environment]::UserName
    $hash=[Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($identity))
    $pipe=[IO.Pipes.NamedPipeClientStream]::new('.','ClaudeGuard-'+[Convert]::ToHexString($hash).Substring(0,20),[IO.Pipes.PipeDirection]::InOut)
    try{if(Get-Process ClaudeGuard -ErrorAction SilentlyContinue|Where-Object Path -eq (Join-Path $install 'ClaudeGuard.exe')){$pipe.Connect(1000);$bytes=[Text.Encoding]::UTF8.GetBytes('{"action":"exit"}'+[char]10);$pipe.Write($bytes,0,$bytes.Length);$pipe.Flush();$reader=[IO.StreamReader]::new($pipe);$reader.ReadLine()|Out-Null}}finally{$pipe.Dispose()}
    Start-Sleep -Seconds 1
    $running=Get-Process ClaudeGuard -ErrorAction SilentlyContinue|Where-Object Path -eq (Join-Path $install 'ClaudeGuard.exe')
    foreach($p in $running){if(-not $p.WaitForExit(5000)){throw 'Guard did not stop for preference restore'}}
    [IO.File]::WriteAllBytes($settings,$before)
    if($runValue){Set-ItemProperty $runKey -Name ClaudeGuard -Value $runValue}else{Remove-ItemProperty $runKey -Name ClaudeGuard -ErrorAction SilentlyContinue}
    if($taskXml){$folder.RegisterTask($taskName,$taskXml,6,$null,$null,3,$null)|Out-Null}else{try{$folder.DeleteTask($taskName,0)}catch {if($_.Exception.GetBaseException().HResult -ne -2147024894){throw}}}
    Start-Process -FilePath (Join-Path $install 'ClaudeGuard.exe') -ArgumentList '--autostart' -WindowStyle Hidden
}
Start-Sleep -Seconds 2
& (Join-Path $install 'cli/claude-guard.exe') --status | Out-Null
if($LASTEXITCODE -ne 0){throw 'Restored guard is unavailable'}
$result.restoredUserPreferencesAndRunningGuard=$true
$result.timestamp=[DateTime]::UtcNow.ToString('o')
$result|ConvertTo-Json|Set-Content (Join-Path $PSScriptRoot 'dist/qa/uninstall.json')
$result|ConvertTo-Json
