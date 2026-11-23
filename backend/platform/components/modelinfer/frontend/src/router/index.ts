import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router';
import { translate } from '@/i18n';

/**
 * hash 路由：Vite base=./ + --url-base 子路径托管场景不感知前缀。
 * 每个页面路由作为平台导航的独立 iframe 入口（见 apps/com.ictrek.modelinfer/src/routers.yml）。
 * `meta.titleKey` 为 i18n key，用于浏览器标题（经 `translate` 取当前语言）。
 */
const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/plaza' },
  {
    path: '/plaza',
    name: 'plaza',
    component: () => import('../views/PlazaView.vue'),
    meta: { titleKey: 'routes.plaza' },
  },
  {
    path: '/cards',
    name: 'cards',
    component: () => import('../views/LocalCardsView.vue'),
    meta: { titleKey: 'routes.cards' },
  },
  {
    path: '/nodes',
    name: 'nodes',
    component: () => import('../views/NodesView.vue'),
    meta: { titleKey: 'routes.nodes' },
  },
  {
    path: '/nodes/:id',
    name: 'node-detail',
    component: () => import('../views/NodeDetailView.vue'),
    meta: { titleKey: 'routes.nodeDetail' },
  },
  {
    path: '/runs',
    name: 'runs',
    component: () => import('../views/RunsView.vue'),
    meta: { titleKey: 'routes.runs' },
  },
  {
    path: '/runs/:id',
    name: 'run-detail',
    component: () => import('../views/RunDetailView.vue'),
    meta: { titleKey: 'routes.runDetail' },
  },
  {
    path: '/settings',
    name: 'settings',
    component: () => import('../views/SettingsView.vue'),
    meta: { titleKey: 'routes.settings' },
  },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});

/** 根据当前路由设置浏览器标题（取当前语言；语言切换时由 main.ts 重新应用） */
export function applyRouteTitle() {
  const key = router.currentRoute.value.meta?.titleKey as string | undefined;
  if (typeof document !== 'undefined') {
    document.title = key ? `${translate(key)} · ${translate('app.title')}` : translate('app.title');
  }
}

router.afterEach(() => applyRouteTitle());