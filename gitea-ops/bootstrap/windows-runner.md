# Windows 构建机（`x86_64-pc-windows-msvc`）

## 1. 一次性准备

```powershell
# Rust（host = x86_64-pc-windows-msvc）
#   安装时勾选 "Desktop development with C++"，或事后装 VS Build Tools
winget install Rustlang.Rustup
rustup default 1.99.1
rustup target add x86_64-pc-windows-msvc

# Git for Windows（提供 bash，工作流用 shell: bash）
winget install Git.Git

# Python 3.11+（打包脚本用）
winget install Python.Python.3.12

# act_runner（宿主机模式）
mkdir C:\ci\act_runner; cd C:\ci\act_runner
curl.exe -fSLO https://gitea.com/gitea/act_runner/releases/download/v0.2.11/act_runner-0.2.11-windows-amd64.exe
# 注册（标签必须是 windows-host:host）
.\act_runner-0.2.11-windows-amd64.exe register --no-interactive `
  --instance http://<gitea-host>:3301 --token <token> `
  --labels windows-host:host --name windows-desktop
```

`config.yaml`：`capacity: 1`（一次只跑一个 job，避免与 VS 环境互相干扰）。

## 2. 启动与保活

```cmd
cd C:\ci\act_runner
start "" /b act_runner-0.2.11-windows-amd64.exe daemon --config config.yaml
```

长期保活建议做成「任务计划程序」的登录时启动项，或用 NSSM 注册成服务。
**注意**：用 `Start-Process -RedirectStandardError` 指定日志文件，否则崩了没有痕迹。

## 3. 该机的 ci 布局

工作流的 MINGW 分支使用：

```
root=C:/ci      cache=C:/ci/cache
cargo_home=$HOME/.cargo      rustup_home=$HOME/.rustup
zig_dir=C:/ci/toolchain/zig
```

rusty_v8 归档放 `C:/ci/cache/rusty_v8/<target>/`。

## 4. 必须踩过的两个坑

### 4.1 `win-build.cmd` 必须是 CRLF

`.cmd` 若被写成 LF 换行，cmd.exe 对 `cd /d ... || exit /b 1` 这类构造解析异常，
只回一句「系统找不到指定的路径」，看不出是换行问题。

仓库 `.gitattributes` 已锁：

```
*.cmd text eol=crlf
*.bat text eol=crlf
*.ps1 text eol=crlf
```

编辑或脚本化改写后务必检查：`python3 -c "print(open('f.cmd','rb').read().count(b'\r\n'))"`。

### 4.2 `shift` 会连带移位 `%0`

```bat
set "TARGET=%~1"
shift                     rem ← 这里之后 %~dp0 已经是第一个参数的路径
cd /d "%~dp0..\..\codex-rs"   rem ← 错的
```

必须先把脚本目录存进变量：

```bat
set "SCRIPT_DIR=%~dp0"
set "TARGET=%~1"
shift
cd /d "%SCRIPT_DIR%..\..\codex-rs"
```

## 5. aarch64-pc-windows-msvc（当前未启用）

在 x64 上交叉编译 ARM64 需要 VS 的 ARM64 工具集：

```
VS Installer → Build Tools 2022 → Individual components →
"MSVC v143 - VS 2022 C++ ARM64 build tools (latest)"     （约 1–2 GB，需管理员）
```

装好后把 `.gitea/workflows/release.yml` 矩阵里注释掉的那三行恢复，并把
`verify-artifacts` 的目标列表加回 `aarch64-pc-windows-msvc`。
在此之前该目标由 GitHub 的 `windows-11-arm` 原生 runner 负责。

## 6. 连不上 Gitea 时

Windows 在另一网段且到不了 Gitea 主机时，用一台**双栈机器**做 3301 端口中继
（`socat`/简单 TCP 代理均可），注册时 `--instance` 指向中继地址；工作流里可用
`CI_GITEA_BASE` 覆盖 checkout 用的地址。
