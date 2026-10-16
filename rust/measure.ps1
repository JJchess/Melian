param([string]$Label='dotnet',[int]$Samples=3)
$ErrorActionPreference='Stop'
$identity=[Environment]::UserDomainName+'\'+[Environment]::UserName
$sha=[Security.Cryptography.SHA256]::Create()
$pipeName='ClaudeGuard-'+([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($identity))).Replace('-','').Substring(0,20))
$exe=Join-Path $env:LOCALAPPDATA 'Programs\ClaudeGuard\ClaudeGuard.exe'
function Guard([string]$Action){
    $pipe=[IO.Pipes.NamedPipeClientStream]::new('.',$pipeName,[IO.Pipes.PipeDirection]::InOut)
    try {
        $pipe.Connect(300)
        $bytes=[Text.Encoding]::UTF8.GetBytes('{"action":"'+$Action+'"}'+[char]10)
        $pipe.Write($bytes,0,$bytes.Length);$pipe.Flush()
        $reader=[IO.StreamReader]::new($pipe,[Text.Encoding]::UTF8)
        $reader.ReadLine() | ConvertFrom-Json
    }finally{$pipe.Dispose()}
}
$times=@()
for($i=0;$i -lt $Samples;$i++){
    $running=Get-Process ClaudeGuard -ErrorAction SilentlyContinue | Where-Object Path -eq $exe
    Guard 'exit' | Out-Null
    foreach($p in $running){if(-not $p.WaitForExit(5000)){throw 'Guard did not exit'}}
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $process=Start-Process -FilePath $exe -ArgumentList '--autostart' -WindowStyle Hidden -PassThru
    while($true){
        try{$state=Guard 'status'; if($state.ok){break}}catch{}
        if($clock.Elapsed.TotalSeconds -gt 15){throw 'Startup timeout'}
        Start-Sleep -Milliseconds 20
    }
    $times+=$clock.Elapsed.TotalMilliseconds
    Start-Sleep -Seconds 2
}
# Do not interact with the island during the idle sample.
$state=Guard 'status'
$process.Refresh();$before=$process.TotalProcessorTime.TotalSeconds
$clock=[Diagnostics.Stopwatch]::StartNew()
Start-Sleep -Seconds 15
$process.Refresh()
$result=[pscustomobject]@{
    label=$Label;timestamp=[DateTime]::UtcNow.ToString('o');startupMetric='warm launch to first successful status response';
    startupMilliseconds=$times;startupMeanMilliseconds=($times|Measure-Object -Average).Average;
    workingSetBytes=$process.WorkingSet64;privateBytes=$process.PrivateMemorySize64;
    cpuSingleCorePercent=100*($process.TotalProcessorTime.TotalSeconds-$before)/$clock.Elapsed.TotalSeconds;
    logicalProcessors=[Environment]::ProcessorCount;sampleSeconds=$clock.Elapsed.TotalSeconds
}
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $PSScriptRoot "dist\qa\metrics-$Label.json") -Encoding utf8
$result | Format-List

