@echo off
rem Build a Windows target from the VS developer environment.
rem
rem   win-build.cmd <target> <cargo args...>
rem
rem aarch64-pc-windows-msvc is a CROSS build on an x64 host here. GitHub does
rem not cross-compile this target at all: `release.yml` runs it on `windows-11-arm`,
rem a native ARM64 runner with its own cl.exe. This runner is x64, so the target
rem needs
rem   1) the VS ARM64 toolset  (VC\Tools\MSVC\<ver>\bin\Hostx64\arm64\cl.exe)
rem   2) vcvarsall amd64_arm64 (sets INCLUDE/LIB for the ARM64 UCRT + SDK)
rem   3) clang-cl for the C dependencies (ring and friends)
rem
rem The 2026-11-06 run failed with `fatal error: 'assert.h' file not found`
rem because cc-rs fell back to plain `clang` from PATH, which had no MSVC
rem include paths in scope. This script now refuses to fall back silently: it
rem preflights the toolset and names the missing component.
setlocal

rem `shift` moves %0 as well, so `%~dp0` becomes the path of the first ARGUMENT
rem after it. Capture the script directory first — doing it later resolved to the
rem current directory ("C:\ci\src") and `cd ..\..\codex-rs` failed with the
rem bare "系统找不到指定的路径" that took a whole CI round to pin down.
set "SCRIPT_DIR=%~dp0"
set "TARGET=%~1"
shift

set "VS=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if not exist "%VS%" set "VS=C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if not exist "%VS%" (
    echo [win-build] ERROR: vcvarsall.bat not found. Install "Visual Studio Build Tools 2022" with the C++ workload. 1>&2
    exit /b 1
)

if /i "%TARGET%"=="aarch64-pc-windows-msvc" (
    rem Preflight the ARM64 toolset: without it clang-cl cannot link, and a
    rem silent fallback to PATH clang produces the confusing assert.h error.
    set "ARM64_CL="
    for /d %%D in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do (
        if exist "%%~D\bin\Hostx64\arm64\cl.exe" set "ARM64_CL=%%~D\bin\Hostx64\arm64\cl.exe"
    )
    for /d %%D in ("C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*") do (
        if exist "%%~D\bin\Hostx64\arm64\cl.exe" set "ARM64_CL=%%~D\bin\Hostx64\arm64\cl.exe"
    )
    if not defined ARM64_CL (
        echo [win-build] ERROR: the VS ARM64 toolset is missing. 1>&2
        echo [win-build]   Looked for: VC\Tools\MSVC\*\bin\Hostx64\arm64\cl.exe 1>&2
        echo [win-build]   Fix: run the VS Installer for "Visual Studio Build Tools 2022" and add 1>&2
        echo [win-build]        Individual components ^> "MSVC v143 - VS 2022 C++ ARM64 build tools ^(latest^)" 1>&2
        echo [win-build]   There is no supported fallback: plain clang from PATH has no MSVC 1>&2
        echo [win-build]   include/library paths, which is what produced the "assert.h not found" 1>&2
        echo [win-build]   failures. Build this target on GitHub^'s windows-11-arm runner instead. 1>&2
        exit /b 1
    )
    call "%VS%" amd64_arm64 >nul
    if errorlevel 1 (
        echo [win-build] ERROR: vcvarsall amd64_arm64 failed. 1>&2
        exit /b 1
    )
    rem Set BOTH the target-specific and the generic variables: cc-rs reads the
    rem target-specific one first, but crates that shell out themselves (or
    rem build scripts that forward CC) only look at the generic name.
    set "CC=clang-cl"
    set "CXX=clang-cl"
    set "AR=llvm-lib"
    set "CC_aarch64_pc_windows_msvc=clang-cl"
    set "CXX_aarch64_pc_windows_msvc=clang-cl"
    set "AR_aarch64_pc_windows_msvc=llvm-lib"
    set "CFLAGS_aarch64_pc_windows_msvc=-DCURL_ENABLE_NTLM"
) else (
    call "%VS%" amd64 >nul
    if errorlevel 1 (
        echo [win-build] ERROR: vcvarsall amd64 failed. 1>&2
        exit /b 1
    )
)

rem Diagnostics: the next failure should be readable straight from the log.
echo [win-build] target=%TARGET%
echo [win-build] CC=%CC% CXX=%CXX% AR=%AR%
echo [win-build] vcvarsall=%VS%
if defined INCLUDE (echo [win-build] INCLUDE=set) else (echo [win-build] INCLUDE=EMPTY)
if defined LIB (echo [win-build] LIB=set) else (echo [win-build] LIB=EMPTY)
where clang-cl >nul 2>&1 && echo [win-build] clang-cl=found || echo [win-build] clang-cl=NOT FOUND
where llvm-lib >nul 2>&1 && echo [win-build] llvm-lib=found || echo [win-build] llvm-lib=NOT FOUND

set "SRC_ROOT=%SCRIPT_DIR%..\.."
cd /d "%SRC_ROOT%\codex-rs"
if errorlevel 1 (
    echo [win-build] ERROR: cannot enter %SRC_ROOT%\codex-rs 1>&2
    exit /b 1
)
echo [win-build] cwd=%CD%
cargo build --target %TARGET% %1 %2 %3 %4 %5 %6 %7 %8 %9
exit /b %errorlevel%
