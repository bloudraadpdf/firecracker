# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0
param(
    [Parameter(Mandatory=$true)][string]$HostAddress,
    [Parameter(Mandatory=$true)][string]$DnsAddress,
    [string]$Label = 'WINDOWS',
    [string]$MacAddress = '06:00:AC:1E:FE:07',
    [string]$GuestAddress = '172.30.254.2',
    [string]$Gateway = '172.30.254.1',
    [switch]$SecureChecks
)
$ErrorActionPreference = 'Stop'
Start-Transcript -Path C:\Users\Public\fc-network.log
Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' | Format-List ProductName, CurrentBuildNumber, UBR, DisplayVersion
function Send-Report([string]$message) {
    $report = New-Object Net.Sockets.TcpClient
    $report.Connect($HostAddress, 40570)
    $bytes = [Text.Encoding]::UTF8.GetBytes($message + "`n")
    $report.GetStream().Write($bytes, 0, $bytes.Length)
    $report.Close()
}
try {
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
    for ($iteration = 1; $iteration -le 3; $iteration++) {
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
        $message = "PASS $($Label)_NET iteration=$iteration bytes=2097152 DNS HTTPS TLS1.2 TCP_EQUAL STORAGE_SHA256=$digest $(Get-Date)"
        $message
        Send-Report $message
        if ($iteration -lt 3) { Start-Sleep -Seconds 60 }
    }
} catch {
    $_ | Format-List * -Force
    try { Send-Report "FAIL $($Label)_NET $($_.Exception.Message)" } catch { $_ | Format-List * -Force }
} finally {
    Stop-Transcript
    shutdown.exe /s /t 15
}
