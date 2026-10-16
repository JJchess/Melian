# Melian 图标

采用选定的 **02 / Nightingale（夜莺）** 方向：向右的头部、向左舒展的长翼，以及翼身之间的弧形留白。标志使用实色轮廓，配色为炭黑 `#191a1b` 与暖白 `#f4f0e8`。

深色圆角底用于 Windows 桌面、开始菜单、托盘及安装器；独立鸟形用于单色品牌展示。应用内状态点继续表达网络状态，品牌图标本身不表示连接已通过验证。

| 文件 | 用途 |
| --- | --- |
| `nightingale.svg` | 标准尺寸的可编辑夜莺矢量母版，无背景 |
| `nightingale-small.svg` | 16–24 px 专用矢量母版，加宽留白并放大轮廓 |
| `brand-mark.svg` | 生成的圆角应用图标，也是 README 中的标志 |
| `brand-mark.png` | 512 × 512 透明背景预览 |
| `../rust/crates/desktop/assets/melian.ico` | Windows 多尺寸图标，嵌入桌面程序和安装器 |

ICO 包含 16、20、24、32、40、48、64、96、128、256 px。16、20、24 px 使用小尺寸母版，其余使用标准母版；两种版本保持相同的夜莺朝向与轮廓特征。外部背景透明，圆角底保持不透明。

修改 `nightingale.svg` 或 `nightingale-small.svg` 后，在 Windows 上使用 Node.js 22 或更新版本及 Edge 重新生成应用 SVG、PNG 和 ICO；不要直接编辑生成文件：

```powershell
node docs/generate-icons.mjs
```

脚本使用独立临时浏览器配置，不读取日常浏览器的登录数据。可通过 `MELIAN_BROWSER` 指定其他 Chromium 浏览器程序路径。检查 `rust/dist/qa/icons/preview.png` 中的浅色、深色与小尺寸效果后，再运行项目构建。

概念阶段使用内置 imagegen 探索方向；生产图标根据选定概念重绘为两套 SVG 路径，构建和图标导出不依赖图像生成服务。

`windows_resources.rs` 将图标作为资源组 **1** 嵌入 Windows EXE，同时写入 Melian 产品名称。桌面窗口、托盘和快捷方式都使用这一资源，安装器包含相同的图标。生成的 ICO 已纳入源码，日常编译不需要浏览器。
