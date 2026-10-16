# Melian 图标

标志以双拱构成 M，中央保留一枚薄荷色四角星；深色圆角底用于 Windows 桌面、开始菜单、托盘及安装器。应用内状态点继续表达网络状态，品牌图标本身不表示连接已通过验证。

| 文件 | 用途 |
| --- | --- |
| `brand-mark.svg` | 可编辑的矢量源文件，也是 README 中的标志 |
| `brand-mark.png` | 512 × 512 透明背景预览 |
| `../rust/crates/desktop/assets/melian.ico` | Windows 多尺寸图标，嵌入桌面程序和安装器 |

ICO 包含 16、20、24、32、40、48、64、96、128、256 px。小尺寸增加笔画粗细，16 / 20 px 省去星芒，保证托盘中的字形清楚。

修改 SVG 后，在 Windows 上使用 Node.js 22 或更新版本及 Edge 重新生成：

```powershell
node docs/generate-icons.mjs
```

脚本使用独立临时浏览器配置，不读取日常浏览器的登录数据。可通过 `MELIAN_BROWSER` 指定其他 Chromium 浏览器程序路径。检查 `rust/dist/qa/icons/preview.png` 中的浅色、深色与小尺寸效果后，再运行项目构建。

`windows_resources.rs` 将图标作为资源组 **1** 嵌入 Windows EXE，同时写入 Melian 产品名称。桌面窗口、托盘和快捷方式都使用这一资源，安装器包含相同的图标。生成的 ICO 已纳入源码，日常编译不需要浏览器。
