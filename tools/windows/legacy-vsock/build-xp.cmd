@echo on
setlocal
for %%D in (D E F G H) do if exist %%D:\BuildEnv\SetupBuildEnv.cmd set EWDK=%%D:
if not defined EWDK exit /b 2
call %EWDK%\BuildEnv\SetupBuildEnv.cmd
set WDK7=C:\LegacyVsock\wdk7\WinDDK\7600.16385.win7_wdk.100208-1538
set HEADER_ARGS=/nologo /c /kernel /Zl /TC /W3 /WX /D_X86_=1 /Di386=1 /D_WIN32_WINNT=0x0501 /DNTDDI_VERSION=0x05010200 /I%WDK7%\inc\ddk /I%WDK7%\inc\crt /I%WDK7%\inc\api
cl %HEADER_ARGS% /FoC:\LegacyVsock\header-baseline.obj C:\LegacyVsock\xp-header-probe.c
if not errorlevel 1 exit /b 20
set CL=/DNONTOSPINTERLOCK
cl %HEADER_ARGS% /FoC:\LegacyVsock\header-fixed.obj C:\LegacyVsock\xp-header-probe.c
if errorlevel 1 exit /b 21
cd /d C:\LegacyVsock\upstream
set XP_ARGS=/m:2 /t:Build /p:Configuration="WinXP Release" /p:Platform=Win32 /p:PlatformToolset=v142 /p:UseLegacyDDK=true /p:DDKINSTALLROOT=C:\LegacyVsock\wdk7\WinDDK\ /p:DDKVER=7600.16385.win7_wdk.100208-1538 /p:SignMode=Off /p:RunCodeAnalysis=false /p:EnableInf2cat=false /v:minimal
msbuild VirtIO\VirtioLib.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 10
msbuild VirtIO\WDF\VirtioLib-WDF.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 11
msbuild viosock\sys\viosock.vcxproj %XP_ARGS%
if errorlevel 1 exit /b 12
exit /b 0
