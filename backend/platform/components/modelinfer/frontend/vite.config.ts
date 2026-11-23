import { fileURLToPath, URL } from 'node:url';
import { defineConfig, loadEnv } from 'vite';
import vue from '@vitejs/plugin-vue';

// 环境变量 VITE_API_TARGET：dev proxy target（/api 代理到 modelinfer 后端 axum）
// 缺省 http://127.0.0.1:19908
const DEFAULT_API_TARGET = 'http://127.0.0.1:19908';

// base: './' 使构建产物使用相对路径，配合 --url-base 子路径托管（VOS iframe 场景）
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const apiTarget = env.VITE_API_TARGET || DEFAULT_API_TARGET;
  const embedded = env.VITE_SCHEDULER_INFERENCE === 'true';

  return {
    plugins: [vue()],
    base: './',
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
        '@scheduler-auth': embedded
          ? '/build/scheduler/vos-auth.ts'
          : fileURLToPath(new URL('./src/api/standalone-auth.ts', import.meta.url)),
      },
    },
    server: {
      port: 5188,
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
            vendor: ['vue', 'vue-router', 'axios', 'ant-design-vue', '@ant-design/icons-vue', 'dayjs'],
          },
        },
      },
    },
  };
});
