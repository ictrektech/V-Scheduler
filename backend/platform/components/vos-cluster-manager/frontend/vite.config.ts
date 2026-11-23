import { fileURLToPath, URL } from 'node:url';
import { defineConfig, loadEnv } from 'vite';
import vue from '@vitejs/plugin-vue';

// 环境变量 VITE_API_TARGET：
// - 优先级：shell 环境变量 > .env / .env.<mode>（由 loadEnv 解析）
// - 用途：dev proxy target（/api 代理到 vos-cluster-manager 后端 axum）
// - 缺省 http://127.0.0.1:18093
const DEFAULT_API_TARGET = 'http://127.0.0.1:18093';

// vos-cluster-manager 前端 Vite 配置
// base: './' 使构建产物使用相对路径，配合 --url-base 子路径托管（VOS iframe 场景）
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const apiTarget = env.VITE_API_TARGET || DEFAULT_API_TARGET;

  return {
    plugins: [vue()],
    base: './',
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
        '@scheduler-auth': env.VITE_SCHEDULER_CLUSTER === 'true'
          ? '/build/scheduler/vos-auth.ts'
          : fileURLToPath(new URL('../../modelinfer/frontend/src/api/standalone-auth.ts', import.meta.url)),
      },
    },
    server: {
      port: 5186,
      proxy: {
        '/api': {
          changeOrigin: true,
          target: apiTarget,
        },
      },
    },
    build: {
      outDir: 'dist',
      target: 'es2022',
      sourcemap: false,
      rollupOptions: {
        output: {
          manualChunks: {
            vendor: [
              'vue',
              'vue-router',
              'axios',
              'ant-design-vue',
              '@ant-design/icons-vue',
              'dayjs',
            ],
          },
        },
      },
    },
  };
});
