/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// A porta é fixa porque `tauri.conf.json` aponta para ela em `devUrl`.
const DEV_PORT = 1420;

export default defineConfig({
  plugins: [react(), tailwindcss()],

  // Tauri serve a UI de um processo separado; falhar alto é melhor do que
  // silenciosamente subir em outra porta e a janela ficar em branco.
  clearScreen: false,
  server: {
    port: DEV_PORT,
    strictPort: true,
    host: '127.0.0.1',
    watch: {
      ignored: ['**/src-tauri/**', '**/target/**'],
    },
  },

  build: {
    // O webview é sempre WebKitGTK recente: não precisamos transpilar para trás.
    target: 'esnext',
    minify: 'esbuild',
    sourcemap: true,
    emptyOutDir: true,
  },

  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
