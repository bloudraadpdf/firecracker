# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0

param([string]$Prefix = 'win7-legacy', [string]$Tag = 'WIN7LEGACY', [switch]$SecureChecks)
$ErrorActionPreference = 'Stop'
Start-Transcript -Path "C:\$Prefix-probe.log" -Force
try {
    & bcdedit.exe /enum '{current}'
    $device = Get-WmiObject Win32_PnPEntity | Where-Object { $_.DeviceID -like '*VEN_1AF4&DEV_1053*' }
    $device | Format-List Name,DeviceID,ConfigManagerErrorCode,Status
    if (@($device).Count -ne 1 -or $device.ConfigManagerErrorCode -ne 0) { throw 'Viosock device did not start' }
    for ($iteration = 1; $iteration -le 3; $iteration++) {
        if ($SecureChecks) { & "$PSScriptRoot\secure-check.ps1" -Iteration $iteration }
        $data = New-Object byte[] 2097152
        $random = New-Object Security.Cryptography.RNGCryptoServiceProvider
        $random.GetBytes($data)
        $random = $null
        $file = New-Object IO.FileStream("C:\$Prefix-data.bin", [IO.FileMode]::Create, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough)
        $file.Write($data, 0, $data.Length)
        $file.Flush()
        $file.Close()
        $read = [IO.File]::ReadAllBytes("C:\$Prefix-data.bin")
        if ([Convert]::ToBase64String($data) -cne [Convert]::ToBase64String($read)) { throw 'Storage mismatch' }
        $hash = [Security.Cryptography.SHA256]::Create()
        $digest = [BitConverter]::ToString($hash.ComputeHash($read)).Replace('-', '').ToLowerInvariant()
        "Hash : $digest"
        & C:\legacy-connect.exe | Out-String | Write-Output
        if ($LASTEXITCODE -ne 0) { throw "CONNECT probe exit $LASTEXITCODE" }
        "${Tag}_PASS iteration=$iteration"
        if ($iteration -lt 3) { Start-Sleep -Seconds 60 }
    }
} catch {
    "${Tag}_FAIL $_"
} finally {
    Stop-Transcript
    & shutdown.exe /s /t 0
}
