param([string]$Output)
$ErrorActionPreference='Stop'
Add-Type @'
using System;using System.Text;using System.Runtime.InteropServices;
public static class FileLocation {
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]public static extern uint GetFinalPathNameByHandle(IntPtr h,StringBuilder p,uint n,uint flags);
}
'@
$exe=Join-Path $env:LOCALAPPDATA 'Programs/ClaudeGuard/ClaudeGuard.exe'
$final=$null
if(Test-Path -LiteralPath $exe){
 $file=[IO.File]::OpenRead($exe)
 try{$text=New-Object Text.StringBuilder 2048;[FileLocation]::GetFinalPathNameByHandle($file.SafeFileHandle.DangerousGetHandle(),$text,2048,0)|Out-Null;$final=$text.ToString()}finally{$file.Dispose()}
}
$result=[ordered]@{
 executableExists=Test-Path -LiteralPath $exe
 actualExecutable=$final
 run=(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name ClaudeGuard -ErrorAction SilentlyContinue).ClaudeGuard
 chromeHost=(Get-ItemProperty 'HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.claudeguard.bridge' -ErrorAction SilentlyContinue).'(default)'
 appData=$env:APPDATA
 processId=$PID
 settingsExists=Test-Path -LiteralPath (Join-Path $env:APPDATA 'ClaudeGuard/settings.json')
 settingsContent=if(Test-Path -LiteralPath (Join-Path $env:APPDATA 'ClaudeGuard/settings.json')){Get-Content (Join-Path $env:APPDATA 'ClaudeGuard/settings.json') -Raw}else{$null}
}
foreach($pair in @(@('settings',(Join-Path $env:APPDATA 'ClaudeGuard/settings.json')),@('longSettings',('\\?\'+(Join-Path $env:APPDATA 'ClaudeGuard/settings.json'))))){
 try{$f=[IO.File]::OpenRead($pair[1]);try{$t=New-Object Text.StringBuilder 2048;[FileLocation]::GetFinalPathNameByHandle($f.SafeFileHandle.DangerousGetHandle(),$t,2048,0)|Out-Null;$result[$pair[0]]=$t.ToString()}finally{$f.Dispose()}}catch{$result[$pair[0]]=$_.Exception.Message}
}
$result|ConvertTo-Json|Set-Content -LiteralPath $Output -Encoding UTF8
