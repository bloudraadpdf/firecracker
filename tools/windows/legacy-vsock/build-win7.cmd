@echo off
setlocal
set "KIT="
for %%D in (D E F G H I J K) do if exist %%D:\BuildEnv\SetupBuildEnv.cmd set "KIT=%%D:"
if not defined KIT exit /b 10
call %KIT%\BuildEnv\SetupBuildEnv.cmd
if errorlevel 1 exit /b 11
cd /d C:\LegacyVsock\upstream
if errorlevel 1 exit /b 14
for %%P in (VirtIO\VirtioLib.vcxproj VirtIO\WDF\VirtioLib-WDF.vcxproj viosock\sys\viosock.vcxproj viosock\lib\viosocklib.vcxproj) do (
  msbuild %%P /m:2 /t:Build /p:Configuration="Win7 Release" /p:Platform=x64 /p:SignMode=Off /p:RunCodeAnalysis=false /p:EnableInf2cat=false /v:minimal
  if errorlevel 1 exit /b 12
)
msbuild viosock\lib\viosocklib.vcxproj /m:2 /t:Build /p:Configuration="Win7 Release" /p:Platform=Win32 /p:SignMode=Off /p:RunCodeAnalysis=false /p:EnableInf2cat=false /v:minimal
if errorlevel 1 exit /b 13
exit /b 0
