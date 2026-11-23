import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router';
import { onLanguageChanged, translate } from '@/i18n';

/**
 * hash 路由：Vite base=./ + --url-base 子路径托管场景不感知前缀。
 *
 * 顶层单入口模式（对齐 cloudbeaver / bitwarden，见 vos-app-spec frontend.md §4.9.1）：
 * 平台导航仅注册一个「镜像仓库」入口（apps/com.ictrek.image-hub/src/routers.yml），
 * 本应用不提供内置侧边栏/导航，仅渲染当前路由组件。
 *
 * 路由集（docker-hub 式信息架构）：
 *   镜像列表首页 #/images（默认）
 *   镜像详情     #/image?repository=<urlencoded>（含 / 的仓库名用 query 承载）
 *
 * 路由标题使用 i18n key（`meta.titleKey`），随平台语言切换实时刷新。
 */
const routes: RouteRecordRaw[] = [
  {
    path: '/',
    redirect: '/images',
  },
  {
    path: '/images',
    name: 'images',
    component: () => import('../views/ImagesView.vue'),
    meta: { titleKey: 'nav.images' },
  },
  {
    path: '/image',
    name: 'image-detail',
    component: () => import('../views/ImageDetailView.vue'),
    meta: { titleKey: 'nav.imageDetail' },
  },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});

/** 按当前路由的 `meta.titleKey` + 当前语言刷新浏览器标题 */
export function applyRouteTitle(): void {
  const titleKey = router.currentRoute.value.meta.titleKey;
  const pageTitle = typeof titleKey === 'string' ? translate(titleKey) : '';
  const appTitle = translate('app.title');
  document.title = pageTitle ? `${pageTitle} · ${appTitle}` : appTitle;
}

// 语言变化（平台 locale-change / 本地切换）时刷新标题
onLanguageChanged(applyRouteTitle);

// 路由切换时刷新标题（hash 路由多入口，每个 iframe 独立 document）
router.afterEach(() => applyRouteTitle());