# Claude 网络守卫 2.1

Windows 11 x64 · Rust 原生桌面应用

正常时只有一个黑色小胶囊：状态点与 Claude。绿色表示网络检查通过，黄色表示检查中。异常时加宽为红色警示胶囊，持续显示「Claude · 连接受阻」，恢复后才还原。绿色不代表所有浏览器配置都已接入。

单击胶囊，展开五个操作：

- **Claude 桌面**：接入并打开本机 Claude Desktop，保留原有账号与会话。
- **Claude Code**：从守卫专用入口打开前台 Claude Code。
- **Claude 网页**：在已接入扩展的浏览器配置里打开网页，继续使用原有登录状态。
- **开机自启**：立即切换并保存 Windows 登录后自启。
- **退出**：退出守卫，受保护连接随即失败。

点击外部或按 Esc 收起。Tab、上下方向键移动焦点，Enter / 空格执行。拖动胶囊可移动位置。关闭面板不会退出守卫。

## 安装与升级

运行 **ClaudeGuard.Setup.exe**。安装位置为当前用户的 %LOCALAPPDATA%\Programs\ClaudeGuard，不需要安装 .NET、WebView 或 Rust。

新安装默认开启登录自启和提示音。升级保留已有自启选择、提示音、恢复节点、Claude 程序路径与窗口位置；配置中的 soundEnabled 可关闭声音。视觉警示始终启用，静音时仍可识别异常。

「开机自启」实际在当前用户登录后执行：使用 Windows 任务计划程序的登录触发器，延迟约 10 秒，启动失败时每分钟重试，最多 3 次。不开启管理员权限、不保存密码。菜单开关即时创建或移除该任务；卸载也会清理。旧版 Run 注册表启动项在升级后移除，避免重复启动。

安装器先解压、检查新版，再停止守卫及其 Native Messaging 桥接。它不结束浏览器、终端、前台 CLI 或 Claude 子进程。旧安装保留为 Programs 目录中的 ClaudeGuard.rollback-*；配置备份位于 %APPDATA%\ClaudeGuard\backups。升级失败时恢复旧程序与配置。

可使用 ClaudeGuard.Setup.exe --quiet 安静安装；--uninstall 卸载。卸载前请在 Edge、Chrome 的扩展页移除「Claude 网络守卫」，这样浏览器会恢复原代理设置。卸载保留用户配置与日志，不主动清除历史回退归档。

## 首次使用网页

点击「Claude 网页」，尚未接入时会出现同一面板内的引导：

1. 打开 Edge 或 Chrome 的扩展页，开启「开发者模式」。
2. 选择「加载解压缩的扩展」，加载安装目录的 extension 文件夹。
3. 重启该浏览器，关闭此前建立的连接。
4. 扩展显示接入成功后，再点击「Claude 网页」。

两个浏览器、每个浏览器配置都要分别接入。关闭扩展、未安装扩展的配置、其他浏览器均不在保护范围。遇到其他代理扩展、企业策略或系统 PAC 冲突，扩展会报告冲突并阻断 Claude 域名。

## Claude Desktop

安装新版官方 Windows MSIX 客户端，网络检查通过后点击「Claude 桌面」，或运行 `claude-guard --desktop`。首次接入前请从 Claude 的托盘菜单完全退出客户端；守卫不会强制结束已有会话。

守卫使用 Claude Desktop 官方固定代理配置 `egressProxyUrl`，写入当前用户的 `HKCU\SOFTWARE\Policies\Claude`，指向本机守卫。配置在 Desktop 下次启动时读取；接入后从原快捷方式打开同样生效，守卫退出时这条代理配置保留，请求失败。菜单的「查看详情」和 `--status` 可查看桌面接入状态。

网络不满足保护条件时，守卫拒绝专用入口启动，并监测、结束从原快捷方式发起的新 Desktop 启动。已有会话保留窗口，受保护连接断开。原快捷方式拦截采用约 200ms 轮询，窗口可能短暂出现；只在守卫运行时执行。守卫退出后的固定代理仍阻断联网，但不再拦截窗口启动。

「已配置」与「已连接守卫」分别显示：后者要求观察到 Desktop 到 17890 的实际 TCP 连接；只有配置值而没有连接证据时，不宣称连接已验证。这项观察不代表已覆盖每个外部工具。

如果 Windows 将策略目录设为只读，首次接入和卸载清理会弹出 UAC 授权。专用安装助手只修改该用户的这一项代理值，守卫和 Claude 客户端仍以普通权限运行；取消授权时不会启动未接入的客户端。

要求 Desktop 1.44121.1 或更新版本。目前自动识别官方 MSIX 安装，不把独立 Claude Code 的 `claude.exe` 当成桌面客户端。存在设备策略、已有固定代理/PAC，或可检测的内置 Code 代理策略冲突时，拒绝覆盖并显示原因。只设置代理，不改账号、模型、登录状态或桌面数据。卸载只移除守卫创建且仍指向守卫的代理值；之后需重启 Desktop 才恢复其原代理来源。

固定代理适用于客户端、内置会话请求及 Windows Cowork 的代理感知网络路径。自动更新、外部浏览器登录、SSH 远端、系统身份认证代理，以及忽略代理的外部程序不在这项保护范围；外部 Edge/Chrome 仍需扩展。它不是全系统防火墙。内置 Code/Cowork 的实际任务验收与普通桌面页面验收分别记录，不因写入配置就宣称全部验证通过。

依据：[官方网络代理说明](https://claude.com/docs/third-party/claude-desktop/network-proxy)、[Windows 策略优先级](https://claude.com/docs/third-party/claude-desktop/mdm)。

## Claude Code 命令

安装器将 cli 目录加入当前用户 PATH。新开终端后运行：

    claude-guard
    claude-guard --cwd "D:\项目" -- --resume

原始参数分别传递，不经过 shell 字符串拼接。启动前检查网络；仅对子进程设置受控代理。异常时只断开网络，不结束会话。

只覆盖经此入口启动的 Windows 前台 Claude Code。已有会话、直接运行 claude、后台 supervisor / --bg / daemon、WSL、SSH 远程任务不在首版保护范围。

其他命令：--desktop、--status、--recheck、--show、--open-web、--guard-help。

## 异常时

面板按当前原因显示一个处理按钮：启动 Clash、打开 Clash、重新检查或恢复代理。选择具体节点请在 Clash 中完成。首次验证通过后，守卫自动保存恢复目标。恢复前先阻断，完成切换后重新验证。

启动时等待 Clash 不弹出提醒，但阻断状态始终以红色文字显示。曾经检查通过后，持续两秒的新异常会自动展开原因 12 秒，并做 6 秒轻微呼吸提示和一次提示音；同一次异常不重复提醒，恢复后再次出现则重新提醒。遵循 Windows 动画偏好；不抢键盘焦点，不更改系统音量，也不绕过静音。点击外部或按 Esc 可收起，红色文字保留到恢复。异常卡片的「查看详情」显示节点、IP 属地、验证时间及浏览器、Desktop 接入情况。

## 保护条件

必须同时满足：Clash 全局模式、能够解析到确定的代理节点、实际代理 IP 属地为美国。节点名称中的“美国”不作为判据。

- 通过 verge-mihomo 命名管道，每秒读取模式、嵌套策略组与节点配置。
- 通过实际 Clash HTTP 代理，每 30 秒查询 https://api.country.is/，验证有效期为 45 秒。
- 控制器健康租约为 3 秒；独立线程每 100ms 检查租约。
- 直连、负载均衡、未知节点链、非美国、超时、过期：保持阻断。
- 代理固定监听 127.0.0.1:17890，不占用其他端口替代；端口冲突会明确显示。
- 节点、配置、网络变化或睡眠恢复撤销旧检测代次，关闭现有受保护连接。
- 不解密 HTTPS；不记录请求内容、凭据或订阅信息。
- 模式与节点变化存在约 1～2 秒检测窗口，不保证变化瞬间零请求通过。

扩展把 claude.ai、claude.com、anthropic.com 及其子域固定送到守卫。没有直连后备；守卫退出或被强制结束后连接失败。其他网站按 Windows 当前的静态系统代理或直连设置转发。

## 数据与构建

配置、日志通常位于 %APPDATA%\ClaudeGuard。安装器会把原有数据目录的实际位置记在安装目录的 `data-root.txt` 中，保证安装环境、Windows 登录和浏览器启动读取同一份配置；遇到 Windows 打包应用的目录重定向时，以此文件记录为准。升级不搬动已有数据。配置沿用 1.x 字段，可手工设置 claudePath；代理端口和控制管道修改后需要重启。日志不记录会话内容。

守卫与桌面代理助手通过当前用户的一次性 Windows 任务启动，防止从打包应用打开安装器时，代理注册表值被写进隔离副本而 Desktop 无法读取。临时任务没有定时触发器，启动确认后立即删除，不改变登录自启开关。接入记录 desktop-proxy.json 保存代理值的归属，不包含会话数据。

正式构建入口为根目录 build.ps1，调用 rust/build.ps1。产品代码全部为 Rust；浏览器 API 适配保留 JavaScript。源码包含 Cargo.lock，运行 cargo test --workspace 和原有 15 项扩展测试。

构建需要 Rust stable MSVC、MSVC x64 构建工具、Windows SDK、Node.js。UI 使用 windows-rs 调用 Win32、Direct2D、DirectWrite、DirectComposition；异步网络采用 Tokio。C 运行库静态链接。

在 Visual Studio 的 x64 Native Tools / Developer PowerShell 中执行根目录 `./build.ps1`。输出为 `rust/dist/ClaudeGuard.Setup.exe`。构建脚本也支持本机已有的 `.tools` 工具缓存，但源码包不分发 Microsoft 工具链；在其他电脑上请先正常安装上述构建依赖。

旧 .NET 安装器及源码位于 rollback-dotnet，未进入 Rust 安装包。

详细测试结果与待验证项目见 VALIDATION.md。Windows 重启登录、真实账号登录与实际睡眠恢复，只有执行后才记为通过。
