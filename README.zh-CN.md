# SoundTrack

[English](README.md) · [简体中文](README.zh-CN.md)

将 Windows 应用音频分配到 **Game、Media、Voice** 三路，在 OBS 中作为独立音频源使用。调整观众听到的混音，同时保持自己听到的原应用音量不变。

SoundTrack 由 Rust 音频引擎、Tauri 2 / Vue 3 桌面调音台和配套 OBS 插件组成。通过 WASAPI Process Loopback 捕获应用进程树，再通过本地命名管道向 OBS 传输 48 kHz、双声道 Float32 音频，无需虚拟音频设备。桌面界面目前为简体中文。

## 功能

- 自动发现有音频会话的应用；未配置的应用默认进入 Game。
- 保存路由分配与排除规则，也可为尚未运行的应用添加规则。
- 每路独立增益、静音和实时峰值电平，默认增益为 50%。
- 按可执行文件路径匹配，可选文件名回退匹配。
- 最小化窗口后驻留系统托盘，继续捕获。
- 由 OBS 静默启动 SoundTrack，并在 OBS 退出时关闭它启动的实例。

Explorer 和系统声音会话不参与捕获。Voice 用于语音聊天等应用音频；当前功能不包含麦克风采集或浏览器标签页独立分流。

## 环境要求与验证状态

- **Windows x64：**更新后的 Windows 10 2004（build 19041）及以上版本，包括 Windows 10 22H2 和 Windows 11。应用还会在启动时实际检测 Process Loopback 接口能否初始化。
- **OBS Studio 31.1.2 x64：**插件使用该版本的头文件和 ABI。
- SoundTrack 与 OBS 需在同一 Windows 用户账户下运行。

项目仍在开发中。真实音频捕获、进程隔离及 OBS 加载和录制的端到端实机验证尚未完成。正式使用前，请运行下方捕获诊断并检查一段 OBS 测试录像。

## 开始使用

先按下方说明构建桌面端和 OBS 插件，或安装你已构建的 MSI/NSIS 安装包和插件 DLL。

1. 退出 OBS，将 `soundtrack_obs_plugin.dll` 复制到 OBS 安装目录的 `obs-plugins\64bit`，然后启动 OBS。
2. 添加 **SoundTrack 路由音频** 源，在属性的 **SoundTrack 程序位置** 中选择安装后的 `soundtrack-desktop.exe`，并选择 Game、Media 或 Voice。所有 SoundTrack 源共用该程序位置。
3. 从托盘图标打开 SoundTrack，让应用播放声音，再将它们分配到所需路由。使用 **添加应用** 可为尚未运行的 `.exe` 创建规则。
4. 在 OBS 中为所需路由分别添加来源，确认 SoundTrack 电平和对应 OBS 音源均收到声音。
5. 为避免重复收音，关闭与这些应用重叠的 OBS 桌面音频、应用音频捕获，并关闭这些源的音频监听。OBS 采样率使用 48 kHz。

OBS 加载已配置的源后会静默启动 SoundTrack；在源属性中设置程序路径并应用也会触发启动。用户手动启动的实例会在 OBS 退出后继续运行。最小化桌面窗口可驻留托盘；关闭窗口或选择 **退出 SoundTrack** 会退出应用。

配置自动保存到 `%APPDATA%\SoundTrack\config.json`。增益与静音仅影响发送给 OBS 的 SoundTrack 混音，不改变原应用的播放音量。

需要分轨录制时，可在 OBS「高级音频属性」中将三路都分配到音轨 1，再分别将 Game 分配到音轨 2、Media 分配到音轨 3、Voice 分配到音轨 4，并在录制输出设置中启用这些音轨。

更多操作与故障排查见[简体中文使用指南](docs/USER_GUIDE.zh-CN.md)。

## 从源码构建

在 Windows 上使用 PowerShell 7，并准备：

- Rust MSVC 工具链、包含 C++ 支持的 Visual Studio Build Tools 和 Windows SDK。
- 与 Vite 8 兼容的 Node.js，以及 pnpm。
- 桌面端运行所需的 Microsoft Edge WebView2 Runtime。
- 构建 OBS 插件还需要 LLVM `libclang` 和匹配的 OBS Studio 31.1.2 源码及构建头文件。

### 桌面端

```powershell
git clone https://github.com/Cola-Ace/SoundTrack.git
cd SoundTrack/desktop
pnpm install --frozen-lockfile
pnpm tauri dev
```

在 `desktop/` 目录下检查前端或构建 Windows 安装包：

```powershell
# 前端类型检查与生产构建
pnpm build

# 构建 MSI 和 NSIS 安装包
pnpm tauri build
```

使用默认 Cargo 输出目录时，安装包位于仓库根目录的 `target/release/bundle/msi/` 和 `target/release/bundle/nsis/`；桌面端可执行文件为 `target/release/soundtrack-desktop.exe`。NSIS 默认按当前用户安装。

### OBS 插件

插件是独立的 Cargo 项目，不属于根目录 workspace。在仓库根目录运行以下命令，并将头文件路径替换为自己的 OBS 源码与构建目录：

```powershell
$env:OBS_INCLUDE_DIRS = 'C:\src\obs-studio-31.1.2\libobs;C:\src\obs-build-31.1.2\config;C:\src\obs-build-31.1.2\libobs'
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo build --locked --release --target x86_64-pc-windows-msvc --manifest-path crates/obs-plugin/Cargo.toml
```

仅安装 OBS 不会提供源码及构建头文件。`OBS_INCLUDE_DIRS` 使用分号分隔各头文件目录，需包含生成的头文件。输出为 `crates/obs-plugin/target/x86_64-pc-windows-msvc/release/soundtrack_obs_plugin.dll`。

更多构建与 IPC 细节见 [OBS 插件说明](crates/obs-plugin/README.md)。

### 捕获诊断

在仓库根目录检测捕获接口，再录制正在播放声音的应用：

```powershell
cargo run -p soundtrack-audio-capture -- --check
cargo run -p soundtrack-audio-capture -- 1234 10 capture.wav
```

将 `1234` 替换为目标 PID，捕获范围包含其子进程。时长与输出路径可省略，默认录制 10 秒并保存为 `capture.wav`。`--check` 检测接口初始化；请试听录音确认实际收到声音。

## 项目结构

| 路径 | 用途 |
| --- | --- |
| `desktop/` | Vue 3 / TypeScript 界面与 Tauri 桌面宿主 |
| `crates/audio-capture/` | Windows Process Loopback 捕获与诊断 CLI |
| `crates/audio-protocol/` | 共用音频 IPC 协议 |
| `crates/obs-plugin/` | 单独构建的 OBS 音频源插件 |
| `docs/` | 使用文档 |

## 许可证

[MIT](LICENSE)。
