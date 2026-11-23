import { fileURLToPath, URL } from 'node:url';
import { defineConfig, loadEnv } from 'vite';
import vue from '@vitejs/plugin-vue';

const DEFAULT_API_TARGET = 'http://127.0.0.1:19914';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  const apiTarget = env.VITE_API_TARGET || DEFAULT_API_TARGET;

  return {
    plugins: [vue()],
    // 相对路径：Vite base=./ + hash 路由，适配 VOS 子路径 iframe 托管
    base: './',
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    server: {
      port: 5189,
      proxy: {
        '/api': { changeOrigin: true, target: apiTarget },
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