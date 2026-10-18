param([switch]$SkipTests)
$ErrorActionPreference='Stop'
$rustRoot=$PSScriptRoot
$projectRoot=Split-Path $rustRoot -Parent
$dist=Join-Path $rustRoot 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$cargo=Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if(-not (Test-Path -LiteralPath $cargo)){$cargo='cargo'}
# Use an existing MSVC installation or the verified local tool cache.
$link=Get-Command link.exe -ErrorAction SilentlyContinue
$msvcRoot=$null
if(-not $link){
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if(Test-Path -LiteralPath $vswhere){
        $installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if($installation){$toolset=Get-ChildItem (Join-Path $installation 'VC/Tools/MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1; if($toolset){$msvcRoot=$toolset.FullName}}
    }
}
if($link){$msvcRoot=Split-Path (Split-Path (Split-Path (Split-Path $link.Source -Parent) -Parent) -Parent) -Parent}
if(-not $msvcRoot -or -not (Test-Path -LiteralPath (Join-Path $msvcRoot 'lib\x64'))){
    $cached=Get-ChildItem (Join-Path $rustRoot '.tools\msvc\Contents\VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    if($cached){$msvcRoot=$cached.FullName}
}
if($msvcRoot){
    $env:PATH=(Join-Path $msvcRoot 'bin\Hostx64\x64')+';'+$env:PATH
    $env:LIB=(Join-Path $msvcRoot 'lib\x64')+';'+$env:LIB
}
$sdk=Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Lib' -Directory | Where-Object {Test-Path -LiteralPath (Join-Path $_.FullName 'um\x64\kernel32.lib')} | Sort-Object Name -Descending | Select-Object -First 1
if($sdk){$env:LIB=(Join-Path $sdk.FullName 'um\x64')+';'+(Join-Path $sdk.FullName 'ucrt\x64')+';'+$env:LIB}
function Run-Cargo([string[]]$Arguments){
    & $cargo @Arguments
    if($LASTEXITCODE -ne 0){throw "cargo failed: $($Arguments -join ' ')"}
}
Push-Location $rustRoot
try{
    if(-not $SkipTests){
        Run-Cargo @('test','--workspace','--locked')
        & node (Join-Path $projectRoot 'tests\extension-tests\run.mjs')
        if($LASTEXITCODE -ne 0){throw 'Extension tests failed'}
    }
    Run-Cargo @('build','--release','--locked','-p','ClaudeGuard','-p','claude-guard')
    $payload=Join-Path $dist ('payload-'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss'))
    New-Item -ItemType Directory -Force (Join-Path $payload 'cli'),(Join-Path $payload 'extension') | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $payload
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination (Join-Path $payload 'extension')
    Copy-Item -LiteralPath (Join-Path $rustRoot 'target\release\ClaudeGuard.exe') -Destination $payload
    Copy-Item -LiteralPath (Join-Path $rustRoot 'target\release\claude-guard.exe') -Destination (Join-Path $payload 'cli')
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'extension') -File | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $payload 'extension')}
    Copy-Item -LiteralPath (Join-Path $rustRoot 'README.zh-CN.md') -Destination (Join-Path $payload '使用说明.md')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs\CODEX.md') -Destination (Join-Path $payload 'Codex 使用说明.md')
    if(Test-Path -LiteralPath (Join-Path $rustRoot 'VALIDATION.md')){Copy-Item -LiteralPath (Join-Path $rustRoot 'VALIDATION.md') -Destination $payload}
    $zip=Join-Path $dist 'payload.zip'
    if(Test-Path -LiteralPath $zip){Remove-Item -LiteralPath $zip}
    [IO.Compression.ZipFile]::CreateFromDirectory($payload,$zip,[IO.Compression.CompressionLevel]::Optimal,$false)
    $env:CLAUDE_GUARD_PAYLOAD=$zip
    Run-Cargo @('build','--release','--locked','-p','guard-setup')
    Copy-Item -LiteralPath (Join-Path $rustRoot 'target\release\guard-setup.exe') -Destination (Join-Path $dist 'ClaudeGuard.Setup.exe') -Force
    Copy-Item -LiteralPath (Join-Path $dist 'ClaudeGuard.Setup.exe') -Destination (Join-Path $dist 'Melian.Setup.exe') -Force
    $check=Start-Process -FilePath (Join-Path $dist 'ClaudeGuard.Setup.exe') -ArgumentList '--check-payload','--quiet' -WindowStyle Hidden -Wait -PassThru
    if($check.ExitCode -ne 0){throw 'Installer payload verification failed'}
    Write-Output ('Installer: '+(Join-Path $dist 'Melian.Setup.exe'))
}finally{Remove-Item Env:CLAUDE_GUARD_PAYLOAD -ErrorAction SilentlyContinue;Pop-Location}

