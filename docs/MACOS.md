# Melian for macOS

macOS 13 及更新版本；提供 Apple 芯片与 Intel 原生构建。当前版本提供原生 AppKit 胶囊、菜单栏入口、登录自启，以及 Claude Code 专用启动入口。

## 保护范围

仅保护从 Melian 菜单或 `claude-guard` 启动的前台 Claude Code。异常时中断受保护网络，保留终端与会话。直接运行原始 `claude`、已有会话、后台任务和 SSH 远端不在范围内。

**macOS Claude Desktop 与浏览器接入尚未验证，当前入口不会将它们当作受保护客户端打开。** 不修改系统代理、浏览器代理或登录数据。绿色只表示网络检查通过。

## 代理发现

自动尝试常见 Clash Verge Rev、ClashX / ClashX Meta、Clash Party / Mihomo Party 和独立 Mihomo 的 Unix socket、本机控制接口及配置文件。仅接受本机接口；配置中的控制密钥只在内存中使用，不记录到日志。自动发现不是对所有客户端版本的兼容承诺。

兼容范围以 Clash / Mihomo 控制协议为准。仅提供系统 VPN、SOCKS 端口，或不开放兼容控制接口的客户端，无法验证全局模式与节点链，当前不会放行。

无法发现时保持阻断。高级自定义可将配置文件 `mihomoPipeName` 设为 Unix socket 的绝对路径；或用 `MELIAN_CONTROLLER` 指定本机 HTTP 控制地址、`MELIAN_CONTROLLER_SECRET` 指定密钥。多个运行中的内核建议明确指定目标。

## 安装与数据

- 应用：`~/Applications/Melian.app`。
- 配置与日志：`~/Library/Application Support/Melian`。
- 登录启动：`~/Library/LaunchAgents/io.github.jjchess.melian.plist`，可在菜单中关闭。
- 卸载：`bash install.sh --uninstall`；保留设置和日志，删除应用、CLI 链接和自启文件。

安装包带有临时本地签名（ad-hoc），尚未提供 Apple Developer ID 签名及公证。安装脚本验证发布文件的 SHA-256，不关闭 Gatekeeper，也不更改系统安全设置。

## 验证边界

macOS CI 构建与测试覆盖独立的 arm64 / x64 目标，并实际创建 AppKit 窗口和导出渲染图。真实 Claude 登录、物理多屏、睡眠恢复、辅助功能及重新登录自启需要实机验收；未执行前不标记为通过。
## Codex 监测（2.3.0）

Mac 与 Windows 使用同一只读监测模块。点击菜单里的 Codex 查看额度、正常重置时间和 Tibo 公告；自动发现 Codex.app 或常见 CLI 安装，复用其登录。公告来源为 codex-reset.com。完整说明见 [Codex 使用说明](CODEX.md)。
