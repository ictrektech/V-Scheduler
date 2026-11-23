# 模型家族图标来源与许可

本目录下的模型家族图标（`*.svg`，除 `generic.svg`）来自 LobeHub Icons：

- 项目主页：<https://icons.lobehub.com/>
- 源码仓库：<https://github.com/lobehub/lobe-icons>
- npm 包：`@lobehub/icons-static-svg@1.95.1`
- 下载地址：<https://registry.npmmirror.com/@lobehub/icons-static-svg/-/icons-static-svg-1.95.1.tgz>
- License：MIT

`generic.svg` 为本仓库自绘的通用模型兜底图标，不来自 LobeHub。

修改说明：`kimi.svg` 为 LobeHub 的 `kimi-color` 变体，其 "K" 主体原始为白色（仅深色背景
可见）；同步脚本将其改为品牌蓝 `#1783FF`，以适配浅色卡片背景。

同步方式（固定版本、可复现）：

```bash
./components/modelinfer/scripts/sync-model-icons.sh
```

更新图标时：修改脚本中的 `LOBE_ICONS_VERSION` 与 `MODEL_ICONS` 列表后重新执行。
