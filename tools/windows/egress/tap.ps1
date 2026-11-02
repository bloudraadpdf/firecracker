# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
param(
    [Parameter(Mandatory=$true)][string]$HostAddress,
    [Parameter(Mandatory=$true)][string]$DnsAddress,
    [string]$Label = 'WINDOWS',
    [string]$MacAddress = '06:00:AC:1E:FE:07',
    [string]$GuestAddress = '172.30.254.2',
    [string]$Gateway = '172.30.254.1',
    [switch]$SecureChecks,
    [switch]$GenerationChecks,
    [string]$GenerationProbeSha256
)
$ErrorActionPreference = 'Stop'
$transcriptStarted = $false
$waiter = $null
$waiterStarted = $false
$eventPipes = @()
$eventDirectory = 'C:\Users\Public\fc-gencounter-events'
$eventDirectoryOwned = $false
function Send-Report([string]$message) {
    if ($eventDirectoryOwned) {
        if ($message.IndexOf("`r") -ge 0 -or $message.IndexOf("`n") -ge 0) { throw 'Expected one report line' }
        $bytes = [Text.Encoding]::UTF8.GetBytes($message + "`n")
        $log = New-Object IO.FileStream((Join-Path $eventDirectory 'reports.log'), [IO.FileMode]::Append, [IO.FileAccess]::Write, [IO.FileShare]::Read, 4096, [IO.FileOptions]::WriteThrough)
        try {
            if ($log.Length + $bytes.Length -gt 16384) { throw 'Generation report log exceeds 16384 bytes' }
            $log.Write($bytes, 0, $bytes.Length)
            $log.Flush()
        } finally { $log.Close() }
    }
    $report = New-Object Net.Sockets.TcpClient
    $report.Connect($HostAddress, 40570)
    $bytes = [Text.Encoding]::UTF8.GetBytes($message + "`n")
    $report.GetStream().Write($bytes, 0, $bytes.Length)
    $report.Close()
}
function Read-EventLines([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return }
    if ((Get-Item -LiteralPath $path).Length -gt 4096) { throw 'Event output exceeds 4096 bytes' }
    [IO.File]::ReadAllLines($path)
}
function Read-GenerationId([string]$path) {
    $values = @()
    for ($read = 1; $read -le 2; $read++) {
        $lines = @(& $path)
        $code = $LASTEXITCODE
        Write-Host "VMGENID_QUERY_READ=$read EXIT=$code"
        foreach ($line in $lines) { Write-Host $line }
        if ($code -ne 0 -or $lines.Count -ne 1) { throw 'Generation-counter query failed' }
        if ($lines[0] -cnotmatch '^VMGENID=([0-9a-f]{32})$') { throw 'Malformed generation-counter result' }
        $values += $matches[1]
    }
    if ($values[0] -cne $values[1]) { throw 'Generation ID changed within the running stage' }
    $values[0]
}
function Update-EventPipes($pipes) {
    foreach ($pipe in $pipes) {
        if ($pipe.Eof) { continue }
        if (-not $pipe.Pending) {
            $pipe.Pending = $pipe.Stream.BeginRead($pipe.Bytes, $pipe.Count, $pipe.Bytes.Length - $pipe.Count, $null, $null)
            $pipe.Wait = $pipe.Pending.AsyncWaitHandle
        }
        if ($pipe.Wait.WaitOne(0, $false)) {
            try { $count = $pipe.Stream.EndRead($pipe.Pending) }
            finally {
                $pipe.Wait.Close()
                $pipe.Wait = $null
                $pipe.Pending = $null
            }
            $pipe.Count += $count
            if ($pipe.Count -gt 4096) { throw 'Event output exceeds 4096 bytes' }
            $pipe.Eof = $count -eq 0
        }
    }
}
function Save-EventPipes($pipes) {
    foreach ($pipe in $pipes) {
        $bytes = New-Object byte[] $pipe.Count
        [Array]::Copy($pipe.Bytes, $bytes, $pipe.Count)
        [IO.File]::WriteAllBytes($pipe.Path, $bytes)
    }
}
function Drain-EventPipes($pipes) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        Update-EventPipes $pipes
        if (@($pipes | Where-Object { -not $_.Eof }).Count -eq 0) { return }
        Start-Sleep -Milliseconds 20
    } while ($clock.ElapsedMilliseconds -lt 5000)
    throw 'Owned event pipes did not reach EOF within five seconds'
}
function Close-EventPipes($pipes) {
    $errors = @()
    foreach ($pipe in $pipes) {
        if (-not $pipe.Stream) { continue }
        try {
            if ($pipe.Pending) {
                if (-not $pipe.Wait.WaitOne(5000, $false)) {
                    $pipe.Stream.Close()
                    if (-not $pipe.Wait.WaitOne(5000, $false)) { throw 'Owned event read did not stop after closing its pipe' }
                }
                $null = $pipe.Stream.EndRead($pipe.Pending)
            }
        } catch {
            $errors += $_.Exception.Message
        } finally {
            try { $pipe.Stream.Close() }
            catch { $errors += $_.Exception.Message }
            if ($pipe.Wait) { $pipe.Wait.Close() }
            $pipe.Pending = $null
            $pipe.Wait = $null
            $pipe.Stream = $null
        }
    }
    if ($errors.Count -ne 0) { throw ($errors -join '; ') }
}
try {
    if ($GenerationChecks) {
        if ($GenerationProbeSha256 -cnotmatch '^[0-9a-fA-F]{64}$') { throw 'GenerationChecks requires GenerationProbeSha256 as 64 hex digits' }
        $probePath = 'C:\Users\Public\vmgenid.exe'
        $probeHash = [Security.Cryptography.SHA256]::Create()
        try { $probeDigest = [BitConverter]::ToString($probeHash.ComputeHash([IO.File]::ReadAllBytes($probePath))).Replace('-', '').ToLowerInvariant() }
        finally { $probeHash.Clear() }
        if ($probeDigest -cne $GenerationProbeSha256.ToLowerInvariant()) { throw 'Unexpected event probe executable' }
        if (Test-Path -LiteralPath $eventDirectory) { throw 'Event output directory already exists' }
    } elseif ($GenerationProbeSha256) {
        throw 'GenerationProbeSha256 requires GenerationChecks'
    }
    if ($GenerationChecks) {
        Start-Transcript -Path C:\Users\Public\fc-network.log -NoClobber
    } else {
        Start-Transcript -Path C:\Users\Public\fc-network.log
    }
    $transcriptStarted = $true
    if ($GenerationChecks) {
        Write-Host "VMGENID_PROBE_SHA256=$probeDigest"
        $null = New-Item -ItemType Directory -Path $eventDirectory
        $eventDirectoryOwned = $true
    }
    Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' | Format-List ProductName, CurrentBuildNumber, UBR, DisplayVersion
    $adapter = $null
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        $adapter = Get-WmiObject Win32_NetworkAdapterConfiguration | Where-Object { $_.MACAddress -eq $MacAddress }
        if ($adapter) { break }
        Start-Sleep -Seconds 2
    }
    if (@($adapter).Count -ne 1) { throw 'Expected one NetKVM adapter' }
    $adapter | Format-List Description, MACAddress, DHCPEnabled, IPAddress
    $result = $adapter.EnableStatic(@($GuestAddress), @('255.255.255.252'))
    if ($result.ReturnValue -ne 0) { throw "EnableStatic returned $($result.ReturnValue)" }
    $result = $adapter.SetGateways(@($Gateway), @(1))
    if ($result.ReturnValue -ne 0) { throw "SetGateways returned $($result.ReturnValue)" }
    $result = $adapter.SetDNSServerSearchOrder(@($DnsAddress))
    if ($result.ReturnValue -ne 0) { throw "SetDNS returned $($result.ReturnValue)" }
    Start-Sleep -Seconds 5
    (Get-Item "$env:windir\System32\winhttp.dll").VersionInfo | Format-List FileVersion, ProductVersion
    $previousGeneration = $null
    for ($iteration = 1; $iteration -le 3; $iteration++) {
        if ($GenerationChecks) {
            $generation = Read-GenerationId $probePath
            if ($generation -cnotmatch '^[0-9a-f]{32}$' -or $generation -ceq '00000000000000000000000000000000') { throw 'Expected one nonzero 16-byte generation ID' }
            if ($previousGeneration -and $generation -ceq $previousGeneration) { throw 'Generation ID unchanged after restore' }
            if ($iteration -gt 1) {
                if (-not $waiter -or -not $waiter.WaitForExit(30000)) { throw 'Restored event waiter did not exit within 30 seconds' }
                Drain-EventPipes $eventPipes
                Save-EventPipes $eventPipes
                if ($waiter.ExitCode -ne 0) { throw "Restored event waiter exit $($waiter.ExitCode)" }
                $completed = $false
                for ($attempt = 0; $attempt -lt 50; $attempt++) {
                    $lines = @(Read-EventLines $waiterOutput)
                    if ($lines.Count -eq 2 -and $lines[0] -ceq "ARMED VMGENID=$previousGeneration" -and $lines[1] -ceq "VMGENID=$generation") {
                        $completed = $true
                        break
                    }
                    Start-Sleep -Milliseconds 100
                }
                if (-not $completed) { throw 'Restored event output does not match both fresh polling reads' }
                if ((Get-Item -LiteralPath $waiterError).Length -ne 0) { throw 'Restored event waiter wrote stderr' }
                $lines | Write-Output
                $eventMessage = "VMGENID_EVENT_COMPLETE iteration=$iteration armed_iteration=$($iteration - 1) pid=$($waiter.Id) old=$previousGeneration new=$generation exit=0"
                $eventMessage
                Send-Report $eventMessage
                Close-EventPipes $eventPipes
                $eventPipes = @()
                $waiter.Close()
                $waiter = $null
                $waiterStarted = $false
            }
            $previousGeneration = $generation
        }
        if ($SecureChecks) { & "$PSScriptRoot\secure-check.ps1" -Iteration $iteration }
        "ITERATION $iteration $(Get-Date)"
        $data = New-Object byte[] 65536
        $storageRandom = New-Object Security.Cryptography.RNGCryptoServiceProvider
        $storageRandom.GetBytes($data)
        $storageRandom = $null
        $file = New-Object IO.FileStream('C:\Users\Public\fc-restore-data.bin', [IO.FileMode]::Create, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough)
        $file.Write($data, 0, $data.Length)
        $file.Flush()
        $file.Close()
        $read = [IO.File]::ReadAllBytes('C:\Users\Public\fc-restore-data.bin')
        if ([Convert]::ToBase64String($data) -cne [Convert]::ToBase64String($read)) { throw 'Storage mismatch' }
        $hash = [Security.Cryptography.SHA256]::Create()
        $digest = [BitConverter]::ToString($hash.ComputeHash($read)).Replace('-', '').ToLowerInvariant()
        "PASS $($Label)_STORAGE bytes=65536 sha256=$digest"
        [Net.Dns]::GetHostAddresses('www.microsoft.com') | Format-Table
        & C:\Users\Public\https.exe | Out-String | Write-Output
        if ($LASTEXITCODE -ne 0) { throw "HTTPS probe exit $LASTEXITCODE" }
        $client = New-Object Net.Sockets.TcpClient
        $client.SendTimeout = 60000
        $client.ReceiveTimeout = 60000
        $client.Connect($HostAddress, 40569)
        $stream = $client.GetStream()
        $sent = New-Object byte[] 8192
        $received = New-Object byte[] 8192
        $random = New-Object Security.Cryptography.RNGCryptoServiceProvider
        for ($chunk = 0; $chunk -lt 256; $chunk++) {
            $random.GetBytes($sent)
            $stream.Write($sent, 0, $sent.Length)
            $offset = 0
            while ($offset -lt $received.Length) {
                $count = $stream.Read($received, $offset, $received.Length - $offset)
                if ($count -eq 0) { throw 'Unexpected EOF' }
                $offset += $count
            }
            if ([Convert]::ToBase64String($sent) -cne [Convert]::ToBase64String($received)) {
                throw "Payload mismatch at chunk $chunk"
            }
        }
        $client.Close()
        $random = $null
        if ($GenerationChecks -and $iteration -lt 3) {
            $waiterOutput = Join-Path $eventDirectory "iteration-$iteration.stdout"
            $waiterError = Join-Path $eventDirectory "iteration-$iteration.stderr"
            $waiter = New-Object Diagnostics.Process
            $waiter.StartInfo.FileName = $probePath
            $waiter.StartInfo.Arguments = "--wait $generation --timeout-ms 300000"
            $waiter.StartInfo.UseShellExecute = $false
            $waiter.StartInfo.CreateNoWindow = $true
            $waiter.StartInfo.RedirectStandardOutput = $true
            $waiter.StartInfo.RedirectStandardError = $true
            if (-not $waiter.Start()) { throw 'Event waiter did not start' }
            $waiterStarted = $true
            $null = $waiter.Handle
            foreach ($entry in @(@($waiter.StandardOutput.BaseStream, $waiterOutput), @($waiter.StandardError.BaseStream, $waiterError))) {
                $eventPipes += New-Object PSObject -Property @{ Stream = $entry[0]; Path = $entry[1]; Bytes = (New-Object byte[] 4097); Count = 0; Pending = $null; Wait = $null; Eof = $false }
            }
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $armed = $false
            do {
                Update-EventPipes $eventPipes
                if ($waiter.HasExited) { throw 'Event waiter exited before the snapshot boundary' }
                if ($eventPipes[1].Count -ne 0) { throw 'Event waiter wrote stderr before its ARMED marker' }
                $text = [Text.Encoding]::ASCII.GetString($eventPipes[0].Bytes, 0, $eventPipes[0].Count)
                if ($text -ceq "ARMED VMGENID=$generation`n" -or $text -ceq "ARMED VMGENID=$generation`r`n") {
                    $armed = $true
                    break
                }
                if ($text.IndexOf("`n") -ge 0 -or $eventPipes[0].Eof) { throw 'Unexpected event ARMED output' }
                Start-Sleep -Milliseconds 20
            } while ($clock.ElapsedMilliseconds -lt 6000)
            if (-not $armed) { throw 'Event waiter did not arm within six seconds' }
            if ($waiter.HasExited) { throw 'Event waiter exited after its ARMED marker' }
            Save-EventPipes $eventPipes
            $lines = @(Read-EventLines $waiterOutput)
            $lines[0]
            $eventMessage = "VMGENID_EVENT_ARMED iteration=$iteration pid=$($waiter.Id) old=$generation alive=true timeout_ms=300000"
            $eventMessage
            Send-Report $eventMessage
            if ($waiter.HasExited) { throw 'Event waiter exited before the PASS report' }
        }
        $generationField = ''
        if ($GenerationChecks) { $generationField = " VMGENID=$generation" }
        $message = "PASS $($Label)_NET iteration=$iteration bytes=2097152 DNS HTTPS TLS1.2 TCP_EQUAL STORAGE_SHA256=$digest$generationField $(Get-Date)"
        $message
        Send-Report $message
        if ($iteration -lt 3) { Start-Sleep -Seconds 60 }
    }
} catch {
    $_ | Format-List * -Force
    if ($transcriptStarted) {
        try { Send-Report "FAIL $($Label)_NET $($_.Exception.Message)" } catch { $_ | Format-List * -Force }
    }
} finally {
    try {
        if ($waiter) {
            try {
                if ($waiterStarted) {
                    if (-not $waiter.HasExited) {
                        $waiter.Kill()
                        if (-not $waiter.WaitForExit(5000)) { throw 'Owned event waiter did not stop within five seconds' }
                    }
                    Drain-EventPipes $eventPipes
                }
            } finally {
                try {
                    if ($waiterStarted) { Save-EventPipes $eventPipes }
                } finally {
                    try { Close-EventPipes $eventPipes }
                    finally { $waiter.Close() }
                }
            }
        }
    } catch {
        $_ | Format-List * -Force
        try { Send-Report "FAIL $($Label)_NET event cleanup $($_.Exception.Message)" } catch { $_ | Format-List * -Force }
    } finally {
        try {
            if ($transcriptStarted) { Stop-Transcript }
        } finally {
            shutdown.exe /s /t 15
        }
    }
}
