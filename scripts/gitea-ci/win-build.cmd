@echo off
rem Build a Windows target from the VS developer environment.
rem
rem   win-build.cmd <target> <cargo args...>
rem
rem aarch64-pc-windows-msvc is a cross build on an x64 host: the C parts of the
rem dependency tree (ring and friends) go through cc-rs, which calls
rem "clang --target=..." and then cannot see the Windows SDK headers. Running
rem inside `vcvarsall amd64_arm64` provides INCLUDE/LIB, and clang-cl (which
rem understands MSVC flags) is used instead of bare clang.
setlocal

set "TARGET=%~1"
shift

set "VS=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if not exist "%VS%" set "VS=C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"

if /i "%TARGET%"=="aarch64-pc-windows-msvc" (
    call "%VS%" amd64_arm64 >nul || exit /b 1
    set "CC_aarch64_pc_windows_msvc=clang-cl"
    set "CXX_aarch64_pc_windows_msvc=clang-cl"
    set "AR_aarch64_pc_windows_msvc=llvm-lib"
    set "CFLAGS_aarch64_pc_windows_msvc=-DCURL_ENABLE_NTLM"
) else (
    call "%VS%" amd64 >nul || exit /b 1
)

cd /d "%~dp0..\..\codex-rs" || exit /b 1
echo [win-build] target=%TARGET% cl=%CC_aarch64_pc_windows_msvc%
cargo build --target %TARGET% %1 %2 %3 %4 %5 %6 %7 %8 %9
exit /b %errorlevel%
