# modelinfer 前端

modelinfer（`com.ictrek.modelinfer`）前端：Vue 3 + TypeScript + Vite + Ant Design Vue，
以**同域 iframe** 每页一个入口嵌入 VOS 平台（hash 路由，见
`apps/com.ictrek.modelinfer/src/routers.yml`）。

## 目录

```
src/
├── main.ts                 # 注册 pinia / i18n / antd / router，非阻塞启动平台联动
├── App.vue                 # ConfigProvider（动态 locale + theme.algorithm）
├── platform/vos-platform.ts# VOS 平台桥接对象适配层（等待/订阅/状态对齐）
├── platform/open-tab.ts    # 详情页经 platform.openTab 打开新标签页 + 桥接缺失回退
├── platform/localized-title.ts # 多语言标题 map 产出 + 桥接版本能力探测
├── stores/app.ts           # 语言 + 明暗主题状态（localStorage 持久化）
├── i18n/                   # vue-i18n 实例、错误码解析、按命名空间拆分的语言包
│   ├── index.ts
│   ├── api-error.ts        # 后端数字错误码 -> 本地化提示
│   └── locales/{zh-CN,en}/ # common/status/error/modelFamily + 各页面命名空间
├── api/                    # HTTP 客户端与领域接口
├── components/             # 通用组件（状态标签、指标板、终端块等）
├── composables/            # useMetrics 等
├── utils/                  # 纯函数（状态/格式化/指标/家族/manifest）
└── views/                  # 七个页面视图
```

## 平台语言 / 主题联动

作为同域 iframe 嵌入时，平台注入 `window.vos_platform`（版本 `0.2.0`，见
`components/vivibit-web/openspec/changes/iframe-bridge-state-broadcast/`）。`platform/vos-platform.ts`：

1. 等待对象注入（已存在立即用；非 iframe 立即降级；iframe 内轮询至 3s 超时，不阻塞首屏挂载）；
2. **先订阅** `broadcast.on('locale-change' | 'theme-change', ...)`，**后** `await getState()` 对齐，
   避免订阅前变化丢失；
3. 未嵌入 / 旧版对象（缺 `broadcast` / `getState`）时安全降级为本地行为。

语言值映射：平台 `zh*` → `zh-CN`，`en-US` / 其他 → `en`；主题仅接受 `light` / `dark`。

## 详情页在平台新标签页打开

节点详情（`#/nodes/:id`）与运行配置详情（`#/runs/:id`）是应用内部 hash 子路由，**未**在
`routers.yml` 声明为平台页面。嵌入平台时，点击列表「详情」优先调用
`vos_platform.platform.openTab({ url, title })`（桥接版本 `0.3.0` 起支持 `openTab`；`0.4.0`
起 `title` 支持多语言 map，见
`components/vivibit-web/openspec/changes/iframe-bridge-open-tab-title-i18n/`）把详情作为**独立 Vben
标签页**打开；同一实体重复打开复用既有标签页（平台按 fullPath 去重）。

`platform/open-tab.ts`：

1. `buildAppAbsoluteUrl(hashPath)`：以当前 iframe 文档 URL 为基准构造含应用挂载基路径的**绝对
   URL**（因构建 `base: './'`，不能依赖 `import.meta.env.BASE_URL`）；
2. `openDetailInNewTab({ hashPath, title })`：同步探测 `window.vos_platform.platform.openTab`，
   存在则调用并返回 `true`；未注入 / 无该函数 / 调用失败（非未登录）返回 `false`，调用方在收到
   `false` 时回退当前 iframe 内导航（`router.push`，即独立访问 / 旧平台的现状行为）。

**标签页标题多语言**（`platform/localized-title.ts`）：

- 详情入口调用 `localizedTitle(key, entityName)` 产出 `Record<locale, string>`（各语言取同一 i18n
  key + 语言中性的实体名后缀），交由平台按当前语言解析；
- `openDetailInNewTab` 经 `normalizeTitleForPlatform` 判断桥接版本：`>= 0.4.0` 原样传 map，
  旧平台用 `pickLocalized` 折叠为当前语言字符串（避免旧平台把对象渲染为 `[object Object]`）；
- **重翻译由平台负责**：平台切换语言 / 刷新恢复时按 map 重新解析已打开的详情标签页标题，应用不
  监听语言回推。

未登录（`PlatformNotAuthenticatedError`）不回退，交由平台完成登录跳转。

## 主题体系

- antd 组件：`App.vue` 的 `ConfigProvider` 按主题切换 `theme.algorithm`
  （`defaultAlgorithm` / `darkAlgorithm`）与 `locale`（`zh_CN` / `en_US`）。
- 自定义样式：统一引用 `src/style.css` 的 `--mi-*` 设计 token；`:root` 为浅色，`html.dark` 为暗色。
  新增自定义颜色应使用 token，禁止写死仅适用于浅色的色值。
- 根标记：`document.documentElement.classList.toggle('dark', isDark)`（由 `stores/app.ts` 维护）。
- 主题来源：嵌入时以平台 `theme` 为准；独立态读取 `localStorage['modelinfer-theme']`（缺省浅色）。

## 国际化约定

- 所有用户可见文案经 vue-i18n 管理；语言包按命名空间拆分于 `src/i18n/locales/{zh-CN,en}/`，
  由 `locales/{zh-CN,en}.ts` 聚合。**中英 key 必须完全对等**（单测校验）。
- 新增页面文案：在对应命名空间片段（如 `zh-CN/plaza.ts` / `en/plaza.ts`）同时补充两种语言。
- 组件内使用 `useI18n()` 的 `t`；列表列 / 选项数组须为 `computed`，以保证切换语言后重算。
- 状态枚举经 `StatusTag` + `utils/status.ts` 的 i18n key 渲染，禁止硬编码状态文案。
- 模型家族名经 `modelFamilyLabel(family, t)` 渲染。
- 后端数字错误码（20701-20739）经 `i18n/api-error.ts` 的 `resolveApiErrorMessage(e)` 本地化；
  未知码回退后端 `msg` 或 `common.requestFailed`。
- 语言切换同时更新 `document.documentElement.lang`、浏览器标题（路由 `meta.titleKey`）与 dayjs locale。
- 语言持久化 key：`modelinfer-lang`；主题持久化 key：`modelinfer-theme`。

## 构建 / 测试

```bash
corepack pnpm install --frozen-lockfile
corepack pnpm dev          # 开发（/api 代理到 VITE_API_TARGET，缺省 http://127.0.0.1:19908）
corepack pnpm check:type   # 类型检查
corepack pnpm test         # vitest 单测
corepack pnpm build        # 产物 dist/（base=./）
```