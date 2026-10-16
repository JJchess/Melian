# Melian Windows installer. Download from GitHub, verify SHA-256, then open Setup.
[CmdletBinding()]
param([string]$Version='latest',[switch]$DownloadOnly,[string]$Destination=(Get-Location).Path)
$ErrorActionPreference='Stop'
if($env:OS -ne 'Windows_NT'){throw 'Use install.sh on macOS.'}
if($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64'){throw 'Windows x64 is required.'}
if($Version -ne 'latest' -and $Version -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$'){throw 'Invalid version.'}
$api='https://api.github.com/repos/JJchess/Melian/releases/'
if($Version -eq 'latest'){$api+='latest'}else{$api+='tags/'+$Version}
$release=Invoke-RestMethod -Uri $api -Headers @{'User-Agent'='Melian-Installer'}
if($release.tag_name -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$' -or $release.draft){throw 'Invalid release metadata.'}
$name='Melian-windows-x64.exe'
$prefix='https://github.com/JJchess/Melian/releases/download/'+$release.tag_name+'/'
$folder=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) ('Melian/Downloads/'+$release.tag_name)
New-Item -ItemType Directory -Path $folder -Force|Out-Null
foreach($file in @($name,'SHA256SUMS.txt')){
    $asset=@($release.assets|Where-Object {$_.name -eq $file})
    if($asset.Count -ne 1 -or $asset[0].browser_download_url -ne ($prefix+$file)){throw "Missing release file: $file"}
    Invoke-WebRequest -UseBasicParsing -Uri $asset[0].browser_download_url -OutFile (Join-Path $folder $file)
}
$checksums=@(Get-Content -LiteralPath (Join-Path $folder 'SHA256SUMS.txt')|Where-Object {$_ -match ('^[a-fA-F0-9]{64}\s+\*?'+[regex]::Escape($name)+'$')})
if($checksums.Count -ne 1){throw 'Missing or ambiguous checksum.'}
$installer=Join-Path $folder $name
if((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ne $checksums[0].Substring(0,64)){throw 'Checksum verification failed. Nothing was installed.'}
if($DownloadOnly){
    New-Item -ItemType Directory -Path $Destination -Force|Out-Null
    # Copy verified bytes, not EFS attributes from a redirected app-local cache.
    [IO.File]::WriteAllBytes((Join-Path $Destination $name),[IO.File]::ReadAllBytes($installer))
    Write-Host ('Verified: '+(Join-Path $Destination $name))
}else{
    Write-Host ('Verified Melian '+$release.tag_name+'. Opening the installer...')
    Invoke-Item -LiteralPath $installer
}
