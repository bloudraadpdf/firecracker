# Copyright 2026 Bloudraad.
# SPDX-License-Identifier: Apache-2.0

$ErrorActionPreference = 'Stop'
$transcriptStarted = $false
try {
    Start-Transcript -Path C:\legacy-install.log -Force
    $transcriptStarted = $true
    & certutil.exe -addstore -f Root C:\Drivers\VIOSock\test-signing.cer
    if ($LASTEXITCODE -ne 0) { throw 'Root certificate import failed' }
    & certutil.exe -addstore -f TrustedPublisher C:\Drivers\VIOSock\test-signing.cer
    if ($LASTEXITCODE -ne 0) { throw 'Publisher certificate import failed' }
    & bcdedit.exe /set '{current}' testsigning on
    if ($LASTEXITCODE -ne 0) { throw 'Test-signing configuration failed' }
    & C:\install-driver-x64.exe 'PCI\VEN_1AF4&DEV_1053&SUBSYS_10531AF4&REV_01' 'C:\Drivers\VIOSock\viosock.inf'
    $result = $LASTEXITCODE
    "INSTALL_RESULT=$result"
    if ($result -ne 0 -and $result -ne 3010) { throw "Driver installation failed: $result" }
    Get-WmiObject Win32_PnPEntity | Where-Object { $_.DeviceID -like '*VEN_1AF4&DEV_1053*' } | Format-List Name,DeviceID,ConfigManagerErrorCode,Status
    Set-Content C:\legacy-install-result.txt 0
} catch {
    $_ | Out-File C:\legacy-install-error.txt
    Set-Content C:\legacy-install-result.txt 1
} finally {
    try {
        if ($transcriptStarted) { Stop-Transcript }
    } finally {
        & shutdown.exe /s /t 0
    }
}
