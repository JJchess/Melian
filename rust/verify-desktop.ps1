param([switch]$Launch,[switch]$TestGuardExit,[string]$Output='desktop-verification.json')
$ErrorActionPreference='Stop'
$install=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard'
$cli=Join-Path $install 'cli\claude-guard.exe'
$guard=Join-Path $install 'ClaudeGuard.exe'
$package=Get-AppxPackage -Name Claude
if(-not $package){throw 'Official Claude Desktop MSIX package not found'}
$desktopExe=Join-Path $package.InstallLocation 'app\claude.exe'
function Desktop-Processes { @(Get-Process -Name claude -ErrorAction SilentlyContinue | Where-Object Path -eq $desktopExe) }
function Desktop-Connections {
    $ids=@((Desktop-Processes).Id)
    @(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {$_.OwningProcess -in $ids -and $_.State -eq 'Established'})
}
if($Launch){
    $response=(& $cli --desktop | ConvertFrom-Json)
    if(-not $response.ok){throw $response.error}
    Start-Sleep -Seconds 8
}
$state=(& $cli --status | ConvertFrom-Json).state
if(-not $state.desktop.configured -or $state.desktop.restartRequired){throw 'Desktop is not enrolled or needs a full restart'}
$port=$state.proxyPort
$connections=Desktop-Connections
$guardConnections=@($connections | Where-Object {$_.RemoteAddress -eq '127.0.0.1' -and $_.RemotePort -eq $port})
$result=[ordered]@{
    timestamp=[DateTimeOffset]::Now.ToString('o')
    desktopVersion=$package.Version.ToString()
    configured=$state.desktop.configured
    desktopProcesses=(Desktop-Processes).Count
    guardConnectionsBefore=$guardConnections.Count
    guardExitTest='not run'
}
if($TestGuardExit){
    if(-not $guardConnections.Count){throw 'No live Desktop-to-guard connection; open a Desktop page and retry'}
    $owned=@(Get-Process -Name ClaudeGuard -ErrorAction SilentlyContinue | Where-Object Path -eq $guard)
    if($owned.Count -ne 1){throw 'Expected exactly one installed guard process'}
    try{
        Stop-Process -Id $owned[0].Id -Force
        Start-Sleep -Seconds 2
        $after=Desktop-Connections
        $remaining=@($after | Where-Object {$_.RemoteAddress -eq '127.0.0.1' -and $_.RemotePort -eq $port})
        $result.guardConnectionsAfterExit=$remaining.Count
        $result.desktopStayedRunning=(Desktop-Processes).Count -gt 0
        $socket=[Net.Sockets.TcpClient]::new()
        try{$socket.Connect('127.0.0.1',$port);$result.newConnectionBlocked=$false}catch{$result.newConnectionBlocked=$true}finally{$socket.Dispose()}
        if($remaining.Count -ne 0 -or -not $result.desktopStayedRunning -or -not $result.newConnectionBlocked){throw 'Guard exit behavior failed'}
        $result.guardExitTest='passed'
    }finally{
        if(-not @(Get-Process -Name ClaudeGuard -ErrorAction SilentlyContinue | Where-Object Path -eq $guard).Count){
            Start-Process -FilePath $guard -WindowStyle Hidden
        }
    }
}
$result | ConvertTo-Json | Set-Content -LiteralPath $Output -Encoding utf8
$result | ConvertTo-Json
