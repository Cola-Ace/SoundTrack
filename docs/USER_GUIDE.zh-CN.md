# SoundTrack 使用指南

SoundTrack 由桌面端和 OBS 插件组成：桌面端发现正在播放声音的应用，将应用音频分配到 Game、Media、Voice 三路；OBS 插件把选定路由作为 OBS 音频源。

> 本指南对应 OBS Studio 31.1.2 x64。插件 DLL 已按该版本构建，但尚未完成在 OBS 中加载、录制的端到端验证。其他 OBS 版本和 ARM64 版本请先单独验证。

## 使用前准备

- Windows x64：更新后的 Windows 10 2004（build 19041）及以上版本，或 Windows 11。Windows 10 22H2（包括 19045.6466）满足版本要求。
- SoundTrack 桌面端负责捕获和混音。配置 OBS 插件中的程序位置后，插件会在 OBS 加载完成时启动它。
- OBS Studio 31.1.2 x64，以及匹配的 SoundTrack OBS 插件。
- SoundTrack 和 OBS 在同一 Windows 用户账户下运行。

## 启动 SoundTrack 桌面端

如果你拿到的是 SoundTrack 安装包（MSI 或 NSIS），运行安装程序并按向导完成安装。开发者可按文末的构建步骤生成安装包；产物位于 `desktop\src-tauri\target\release\bundle` 下的 `msi` 或 `nsis` 目录。NSIS 安装包默认安装到当前 Windows 用户。

手动启动 SoundTrack 时，应用发现、音频捕获、路由混音和本地 OBS 音频连接会自动启动。主窗口顶部显示捕获状态；“音频捕获已就绪”表示当前捕获探测通过。OBS 插件启动的实例不会显示主窗口，会留在系统托盘；正常退出 OBS 时，该实例也会退出。

关闭主窗口会将 SoundTrack 收到系统托盘，后台服务仍会运行。左键单击托盘图标或右键菜单中的“打开 SoundTrack”可重新显示窗口；要停止后台服务，请在托盘菜单中选择“退出 SoundTrack”。

应用配置会自动保存到 `%APPDATA%\SoundTrack\config.json`，无需手动保存。

## 管理应用和路由

### 查看和分配应用

有活动音频会话的应用会出现在 Game、Media、Voice 三张路由卡片中。新应用默认进入 Game。用应用行中的下拉框切换路由；手动选择会保存为该应用的规则，后续启动时继续生效。

Explorer（Windows 桌面外壳）和系统声音会话不参与应用捕获。通过 Explorer 启动的 TeamSpeak、游戏等应用仍可独立分配路由；已有的 Explorer 规则不再生效。

需要为尚未运行的程序预先设置路由时：

1. 选择右上角“添加应用”。
2. 选择默认路由。
3. 点击“选择 .exe 文件”并选中程序。

SoundTrack 会保存该程序的完整路径，并在它运行后应用路由。搜索框可按应用名、窗口标题或路径筛选列表。

### 排除或恢复应用

点击应用行的排除按钮可将它从所有路由移除。被排除的应用会显示在页面下方“已排除应用”区域；点击“恢复捕获”可重新加入。删除手动规则会让应用恢复默认 Game 路由。

默认情况下，规则按程序路径匹配。勾选“允许文件名回退匹配”后，SoundTrack 也会按 `.exe` 文件名匹配其他目录中的同名程序；如果电脑上有多个同名程序，建议保持关闭，避免将规则应用到错误的程序。

### 调整路由音量

每张路由卡片显示峰值电平和过载状态，并提供 0–100% 增益滑块和静音按钮。默认增益为 50%，默认未静音。增益和静音只影响 SoundTrack 混音及送往 OBS 的音频，不会改变原应用或 Windows 的播放音量。

如果应用行出现捕获错误或父子进程路由冲突提示，该应用的捕获可能已暂停。检查相关父子应用是否被分配到不同路由；需要隔离时，可将相关进程放到同一路由，或排除其中一个应用。

## 在 OBS 中安装和使用插件

### 安装插件

1. 退出 OBS。
2. 找到构建好的插件文件：

   `crates\obs-plugin\target\x86_64-pc-windows-msvc\release\soundtrack_obs_plugin.dll`

3. 将 DLL 复制到 OBS 安装目录下的 `obs-plugins\64bit` 文件夹。例如：

   `E:\downloads\OBS-Studio-31.1.2-Windows-x64\obs-plugins\64bit\soundtrack_obs_plugin.dll`

4. 启动 OBS。OBS 在启动时加载插件；若 OBS 已经打开，复制 DLL 后需要重启 OBS。

### 添加路由音频源

1. 在 OBS 的“来源”面板点击“＋”。
2. 选择“SoundTrack 路由音频”。
3. 在源属性的“SoundTrack 程序位置”中选择桌面端可执行文件，在“路由”下拉框中选择 Game、Media 或 Voice。所有 SoundTrack 路由源共用该程序位置，只需在一个源中配置。
4. 若要在 OBS 混音器中分别控制三路，请创建三个来源，并分别选择 Game、Media 和 Voice。

配置程序位置后，SoundTrack 插件会在 OBS 完成加载时静默启动桌面端；如果 OBS 已经运行，修改源属性并应用后也会启动。插件会自动连接桌面端；源属性中的状态文字可用于查看连接情况：

- “等待 SoundTrack 连接”：尚未连上桌面端。
- “已连接 SoundTrack”：插件已连上桌面端。

如果 SoundTrack 在 OBS 启动前已经由用户手动打开，插件不会抢到前台；OBS 退出时也不会关闭这个手动启动的实例。确认桌面端的对应路由有电平后，OBS 中同一路由的音频源才会收到音频。之后可像其他 OBS 音频源一样将它用于录制或直播。

## 常见问题

### 桌面端提示音频捕获未就绪

应用列表仍会扫描。程序根据系统版本判断是否可以尝试按应用捕获，并实际初始化 Process Loopback 接口：

- 低于 Windows 10 2004（build 19041）：显示当前系统版本和升级要求，不启动音频捕获，也不会改为录制整个播放设备的混音。
- Windows 10 2004 及以上和 Windows 11：使用相同的 Process Loopback 按进程树捕获。若初始化失败，提示会包含系统版本和具体错误；安装可用的 Windows 更新、检查 Windows Audio 服务后，点击“重试”。

该版本门槛与 [OBS 31.1.2 的实现](https://github.com/obsproject/obs-studio/blob/31.1.2/plugins/win-wasapi/plugin-main.cpp#L35-L50) 一致。微软文档标注的最低 build 20348 与 OBS 在更新后的 Win10 上实际采用的门槛不同。

开发环境可运行 `cargo run -p soundtrack-audio-capture -- --check` 查看系统版本并检查接口初始化。检查通过后，还需要让目标应用播放声音，确认对应路由有电平并在 OBS 中录制验证；初始化成功不代表已经验证真实音频和应用隔离。

### 应用没有出现在列表中

先让应用播放声音，再点击刷新。若要提前设置尚未运行的应用，请用“添加应用”选择它的 `.exe` 文件。

### OBS 找不到“SoundTrack 路由音频”

确认 DLL 位于当前 OBS 安装目录的 `obs-plugins\64bit`，且 OBS 为 x64；然后重启 OBS。此插件构建目标为 OBS 31.1.2 x64。

### OBS 源显示等待连接或没有电平

检查 OBS 源属性中的 SoundTrack 程序位置是否指向有效的桌面端可执行文件，并确认 OBS 与 SoundTrack 使用同一 Windows 用户账户。再检查桌面端捕获状态、目标应用是否有声音、应用所在路由是否正确，以及 OBS 源选中的路由是否与桌面端一致。

## 从源码启动和构建

开发环境可从仓库根目录运行：

```powershell
cd desktop
pnpm install
pnpm tauri dev
```

构建桌面端安装包：

```powershell
cd desktop
pnpm tauri build
```

OBS 插件需要 OBS 31.1.2 的源码/构建头文件、Rust `x86_64-pc-windows-msvc` 目标和 LLVM `libclang`。具体环境变量和构建命令见 [OBS 插件构建说明](../crates/obs-plugin/README.md)。
