# Claude 网络守卫 2.0

Windows 11 x64 · Rust 原生桌面应用

平时只有一个黑色小胶囊：状态点与 Claude。绿色表示网络检查通过，黄色表示检查中，红色表示连接受阻。绿色不代表所有浏览器配置都已接入。

单击胶囊，展开四个操作：

- **Claude Code**：从守卫专用入口打开前台 Claude Code。
- **Claude 网页**：在已接入扩展的浏览器配置里打开网页，继续使用原有登录状态。
- **开机自启**：立即切换并保存 Windows 登录后自启。
- **退出**：退出守卫，受保护连接随即失败。

点击外部或按 Esc 收起。Tab、上下方向键移动焦点，Enter / 空格执行。拖动胶囊可移动位置。关闭面板不会退出守卫。

## 安装与升级

运行 **ClaudeGuard.Setup.exe**。安装位置为当前用户的 %LOCALAPPDATA%\Programs\ClaudeGuard，不需要安装 .NET、WebView 或 Rust。

新安装默认开启登录自启、关闭提示音。升级保留已有自启选择、提示音、恢复节点、Claude 程序路径与窗口位置。

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

## Claude Code 命令

安装器将 cli 目录加入当前用户 PATH。新开终端后运行：

    claude-guard
    claude-guard --cwd "D:\项目" -- --resume

原始参数分别传递，不经过 shell 字符串拼接。启动前检查网络；仅对子进程设置受控代理。异常时只断开网络，不结束会话。

只覆盖经此入口启动的 Windows 前台 Claude Code。已有会话、直接运行 claude、后台 supervisor / --bg / daemon、WSL、SSH 远程任务不在首版保护范围。

诊断命令：--status、--recheck、--show、--open-web、--guard-help。

## 异常时

面板按当前原因显示一个处理按钮：启动 Clash、打开 Clash、重新检查或恢复代理。选择具体节点请在 Clash 中完成。首次验证通过后，守卫自动保存恢复目标。恢复前先阻断，完成切换后重新验证。

启动时等待 Clash 保持安静。曾经检查通过后，持续两秒的新异常会短暂展开六秒；同一原因不重复提醒。异常卡片的「查看详情」显示节点、IP 属地、验证时间和浏览器接入情况。

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

正式构建入口为根目录 build.ps1，调用 rust/build.ps1。产品代码全部为 Rust；浏览器 API 适配保留 JavaScript。源码包含 Cargo.lock，运行 cargo test --workspace 和原有 15 项扩展测试。

构建需要 Rust stable MSVC、MSVC x64 构建工具、Windows SDK、Node.js。UI 使用 windows-rs 调用 Win32、Direct2D、DirectWrite、DirectComposition；异步网络采用 Tokio。C 运行库静态链接。

在 Visual Studio 的 x64 Native Tools / Developer PowerShell 中执行根目录 `./build.ps1`。输出为 `rust/dist/ClaudeGuard.Setup.exe`。构建脚本也支持本机已有的 `.tools` 工具缓存，但源码包不分发 Microsoft 工具链；在其他电脑上请先正常安装上述构建依赖。

旧 .NET 安装器及源码位于 rollback-dotnet，未进入 Rust 安装包。

详细测试结果与待验证项目见 VALIDATION.md。Windows 重启登录、真实账号登录与实际睡眠恢复，只有执行后才记为通过。

