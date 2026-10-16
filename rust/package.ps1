param([string]$Version='2.2.0')
$ErrorActionPreference='Stop'
$project=Split-Path $PSScriptRoot -Parent
$release=Join-Path $project ('release/Melian-'+$Version)
New-Item -ItemType Directory -Force $release|Out-Null
$source=Join-Path $release ('.source-'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force (Join-Path $source 'rust/.cargo'),(Join-Path $source 'extension'),(Join-Path $source 'tests/extension-tests')|Out-Null
foreach($name in @('Cargo.toml','Cargo.lock','README.zh-CN.md','VALIDATION.md','BROWSER-INCIDENT.md')){Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $source 'rust')}
Get-ChildItem -LiteralPath $PSScriptRoot -File | Where-Object Extension -in '.ps1','.sh' | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $source 'rust')}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'crates') -Destination (Join-Path $source 'rust') -Recurse
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '.cargo/config.toml') -Destination (Join-Path $source 'rust/.cargo')
foreach($name in @('README.md','LICENSE','install.ps1','install.sh','.gitignore','.gitattributes')){Copy-Item -LiteralPath (Join-Path $project $name) -Destination $source}
Copy-Item -LiteralPath (Join-Path $project 'docs') -Destination $source -Recurse
Copy-Item -LiteralPath (Join-Path $project '.github') -Destination $source -Recurse
Get-ChildItem -LiteralPath (Join-Path $project 'extension') -File | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $source 'extension')}
Copy-Item -LiteralPath (Join-Path $project 'LICENSE') -Destination (Join-Path $source 'extension')
Get-ChildItem -LiteralPath (Join-Path $project 'tests/extension-tests') -Filter '*.mjs' -File | ForEach-Object {Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $source 'tests/extension-tests')}
Copy-Item -LiteralPath (Join-Path $project 'tests/install-macos.sh') -Destination (Join-Path $source 'tests')
foreach($pair in @(@($source,'Melian.Source.zip'),@((Join-Path $source 'extension'),'Melian.Extension.zip'))){
    $zip=Join-Path $release $pair[1]
    if(Test-Path -LiteralPath $zip){Remove-Item -LiteralPath $zip}
    [IO.Compression.ZipFile]::CreateFromDirectory($pair[0],$zip,[IO.Compression.CompressionLevel]::Optimal,$false)
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'dist/Melian.Setup.exe') -Destination $release -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.zh-CN.md') -Destination (Join-Path $release '使用说明.md') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'VALIDATION.md') -Destination (Join-Path $release '验收记录.md') -Force
Copy-Item -LiteralPath (Join-Path $project 'LICENSE') -Destination $release -Force
$hashes=Get-ChildItem -LiteralPath $release -File | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object Name | ForEach-Object {((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()+'  '+$_.Name)}
[IO.File]::WriteAllText((Join-Path $release 'SHA256SUMS.txt'),($hashes -join "`n")+"`n",[Text.UTF8Encoding]::new($false))
# Remove only the newly created, validated source staging directory.
$resolved=[IO.Path]::GetFullPath($source)
if(-not $resolved.StartsWith([IO.Path]::GetFullPath($release)+[IO.Path]::DirectorySeparatorChar) -or -not (Split-Path $resolved -Leaf).StartsWith('.source-')){throw 'Source staging path escaped release directory'}
Remove-Item -LiteralPath $resolved -Recurse -Force
Get-ChildItem -LiteralPath $release -File | Select-Object Name,Length
