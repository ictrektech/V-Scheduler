import { createRouter, createWebHashHistory, type RouteRecordRaw } from 'vue-router';

/**
 * hash 路由：Vite base=./ + --url-base 子路径托管场景不感知前缀
 * （文档请求路径恒为 <url-base>/，hash 段由 vue-router 解析）
 *
 * 沉浸式多入口模式（对齐 vos-virt-manager）：
 * 平台导航以同域 iframe 将每个页面路由作为独立入口加载（见
 * apps/com.ictrek.vos-cluster-manager/src/routers.yml），本应用不提供内置
 * 侧边栏/导航，仅渲染当前路由组件；页面间跳转在 iframe 内通过 hash 完成。
 *
 * 路由集（cluster-role-and-create-entry 后）：仅 overview / nodes 两页；
 * 创建集群收敛为概览页「创建集群」按钮 + Modal（无独立 #/create 路由）。
 */
const routes: RouteRecordRaw[] = [
  {
    path: '/',
    redirect: '/overview',
  },
  {
    path: '/overview',
    name: 'overview',
    component: () => import('../views/OverviewView.vue'),
    meta: { title: '集群概览' },
  },
  {
    path: '/nodes',
    name: 'nodes',
    component: () => import('../views/NodesView.vue'),
    meta: { title: '节点管理' },
  },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});