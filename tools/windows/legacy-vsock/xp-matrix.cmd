@echo off
mode COM1: BAUD=115200 PARITY=n DATA=8 STOP=1
C:\xp-connect.exe >C:\xp-legacy-probe.log 2>&1
set RESULT=%ERRORLEVEL%
>>C:\xp-legacy-probe.log echo CONNECT_RESULT=%RESULT%
type C:\xp-legacy-probe.log >COM1
shutdown -s -t 0
