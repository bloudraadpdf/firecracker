param([ValidateSet('win7', 'xp')][string]$Target = 'win7')
$ErrorActionPreference = 'Stop'
Start-Transcript -Path C:\LegacyVsock\package-transcript.txt -Force
try {
    $kit = (Get-Volume | Where-Object { $_.DriveLetter -and (Test-Path ($_.DriveLetter + ':\BuildEnv\SetupBuildEnv.cmd')) } | Select-Object -First 1).DriveLetter + ':'
    $tools = "$kit\Program Files\Windows Kits\10\bin\10.0.19041.0"
    $source = 'C:\LegacyVsock\upstream\viosock'
    $package = "C:\LegacyVsock\$Target-package"
    New-Item -ItemType Directory $package -Force | Out-Null
    if ($Target -eq 'xp') {
        $driver = "$source\sys\objfre_wxp_x86\i386"
        $catalogOs = 'XP_X86'
        $coinstaller = @(Get-ChildItem C:\LegacyVsock\wdk7 -Recurse -Filter WdfCoInstaller01009.dll | Where-Object { $_.FullName -match '\\x86\\' })
        if ($coinstaller.Count -ne 1) { throw 'Expected one x86 KMDF 1.9 coinstaller' }
        Copy-Item $coinstaller[0].FullName "$package\WdfCoInstaller01009.dll"
        Copy-Item "$source\lib\objfre_wxp_x86\viosocklib.dll" $package
    } else {
        $driver = "$source\sys\objfre_win7_amd64\amd64"
        $catalogOs = '7_X64'
        Copy-Item "$driver\viosock\WdfCoinstaller01009.dll" $package
        Copy-Item "$source\lib\x64\Win7Release\viosocklib.dll" "$package\viosocklib_x64.dll"
        Copy-Item "$source\lib\x86\Win7Release\viosocklib.dll" "$package\viosocklib_x86.dll"
    }
    Copy-Item "$driver\viosock.sys" $package
    Copy-Item "$driver\viosock.inf" $package
    $cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.Subject -eq 'CN=Firecracker Legacy Vsock Test' -and $_.HasPrivateKey } | Select-Object -First 1
    if (-not $cert) { throw 'Expected the existing test-signing certificate' }
    Export-Certificate -Cert $cert -FilePath "$package\test-signing.cer" | Out-Null
    Import-Certificate -FilePath "$package\test-signing.cer" -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    & "$tools\x64\signtool.exe" sign /fd sha1 /sm /s My /sha1 $cert.Thumbprint "$package\viosock.sys"
    if ($LASTEXITCODE -ne 0) { throw 'Driver signing failed' }
    & "$tools\x86\Inf2Cat.exe" "/driver:$package" "/os:$catalogOs" /uselocaltime
    if ($LASTEXITCODE -ne 0) { throw 'Catalog generation failed' }
    & "$tools\x64\signtool.exe" sign /fd sha1 /sm /s My /sha1 $cert.Thumbprint "$package\viosock.cat"
    if ($LASTEXITCODE -ne 0) { throw 'Catalog signing failed' }
    & "$tools\x64\signtool.exe" verify /pa "$package\viosock.cat"
    if ($LASTEXITCODE -ne 0) { throw 'Catalog verification failed' }
    Get-ChildItem $package | Get-FileHash -Algorithm SHA256 | Format-Table -AutoSize
    Set-Content C:\LegacyVsock\package-result.txt 0
} catch {
    $_ | Out-File C:\LegacyVsock\package-error.txt
    Set-Content C:\LegacyVsock\package-result.txt 1
} finally {
    Stop-Transcript
    & shutdown.exe /s /t 10
}
