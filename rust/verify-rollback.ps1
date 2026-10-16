param([string]$Installer=(Join-Path $PSScriptRoot 'dist/qa/rollback-test.exe'))
$ErrorActionPreference='Stop'
$installed=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\ClaudeGuard.exe'
$config=Join-Path $env:APPDATA 'ClaudeGuard\settings.json'
$beforeExe=(Get-FileHash -LiteralPath $installed).Hash
$beforeConfig=Get-Content -LiteralPath $config -Raw | ConvertFrom-Json
$env:CLAUDE_GUARD_FAIL_UPGRADE='1'
try{$process=Start-Process -FilePath $Installer -ArgumentList '--quiet' -WindowStyle Hidden -PassThru; $process.WaitForExit()}
finally{Remove-Item Env:CLAUDE_GUARD_FAIL_UPGRADE -ErrorAction SilentlyContinue}
if($process.ExitCode -ne 1){throw 'Expected injected upgrade failure'}
$afterExe=(Get-FileHash -LiteralPath $installed).Hash
$afterConfig=Get-Content -LiteralPath $config -Raw | ConvertFrom-Json
if($beforeExe -ne $afterExe){throw 'Previous executable was not restored'}
foreach($key in @('autoStart','soundEnabled','preferredNode','claudePath','islandLeft','islandTop')){
    if($beforeConfig.$key -ne $afterConfig.$key){throw "Preference changed: $key"}
}
Start-Sleep -Seconds 3
& (Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\cli\claude-guard.exe') --status
if($LASTEXITCODE -ne 0){throw 'Restored guard is not responding'}
[pscustomobject]@{rollbackPassed=$true;preservedConfig=$true;restoredExecutableHash=$afterExe;timestamp=[DateTime]::UtcNow.ToString('o')} |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $PSScriptRoot 'dist/qa/rollback.json')
Write-Output 'Upgrade fault injection: previous executable, settings and running guard restored.'

