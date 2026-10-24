@echo off
setlocal
for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath`) do set "MOLITE_VS=%%i"
if not defined MOLITE_VS exit /b 1
call "%MOLITE_VS%\VC\Auxiliary\Build\vcvars64.bat" >nul
if errorlevel 1 exit /b 2
cl /nologo /W4 /Iinclude examples\c_smoke.c /Fetarget\debug\c_smoke.exe /Fotarget\debug\c_smoke.obj /link target\debug\molite_core.dll.lib
if errorlevel 1 exit /b 3
target\debug\c_smoke.exe target\c-smoke-data
exit /b %errorlevel%
