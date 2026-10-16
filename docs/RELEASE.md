Windows 11 x64 与 macOS 13+（Apple 芯片 / Intel）的原生 Rust 应用。

- 新增一行命令安装与 SHA-256 校验。
- 新增 macOS AppKit 胶囊、登录自启、Clash / Mihomo 自动发现及 Claude Code 受保护启动入口。
- 采用夜莺品牌图标与 Apache License 2.0。
- 清理源码仓库中的历史二进制包及本机临时验收脚本；安装产物改为 Release 附件。

Windows 保留已验证的 Claude Desktop 与 Claude Code 接入。macOS 当前仅保护专用入口启动的 Claude Code；Desktop 接入尚未验证。两平台的自动网页保护继续停用，不修改浏览器或系统代理。

macOS 使用 ad-hoc 签名，尚未进行 Apple 公证。真实重启登录、多屏与睡眠恢复仍待实机验收。构建成功与自动化通过不等于这些场景已完成。
