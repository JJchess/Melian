param([switch]$SkipTests)
$ErrorActionPreference='Stop'
& (Join-Path $PSScriptRoot 'rust\build.ps1') -SkipTests:$SkipTests
