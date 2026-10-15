@echo off
mode COM1: BAUD=115200 PARITY=n DATA=8 STOP=1
call :install >C:\legacy-install.log 2>&1
set RESULT=%ERRORLEVEL%
>>C:\legacy-install.log echo INSTALL_SCRIPT_RESULT=%RESULT%
>C:\legacy-install-result.txt echo %RESULT%
type C:\legacy-install.log >COM1
shutdown -s -t 0
exit /b
:install
C:\CertMgr.exe -add -c C:\Drivers\VIOSock\test-signing.cer -s -r localMachine Root
if errorlevel 1 exit /b 1
C:\CertMgr.exe -add -c C:\Drivers\VIOSock\test-signing.cer -s -r localMachine TrustedPublisher
if errorlevel 1 exit /b 2
C:\install-driver-x86.exe
set DRIVER_RESULT=%ERRORLEVEL%
echo INSTALL_RESULT=%DRIVER_RESULT%
if "%DRIVER_RESULT%"=="0" goto device
if "%DRIVER_RESULT%"=="3010" goto device
exit /b 3
:device
wmic path Win32_PnPEntity where "DeviceID like '%%VEN_1AF4%%DEV_1053%%'" get Name,DeviceID,ConfigManagerErrorCode /format:list
exit /b 0
