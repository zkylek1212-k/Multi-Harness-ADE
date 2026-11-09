import { resolve } from 'path'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'

/** Same pages, with the Tauri entry in front and its IPC transport allowed by the page's CSP */
const desktopPage = (): Plugin => ({
  name: 'wb-desktop-page',
  transformIndexHtml: {
    order: 'pre',
    handler(html, ctx) {
      if (!ctx.filename.endsWith('index.html')) return html
      return html
        .replace('src="/src/main.tsx"', 'src="../desktop/main.ts"')
        .replace("connect-src 'self'", "connect-src 'self' ipc: http://ipc.localhost")
    }
  }
})

export default defineConfig({
  root: resolve('src/renderer'),
  base: './',
  resolve: {
    alias: {
      '@': resolve('src/renderer/src'),
      '@panels': resolve('src/renderer/src/panels'),
    }
  },
  plugins: [desktopPage(), react()],
  build: {
    outDir: resolve('out/desktop'),
    emptyOutDir: true,
    target: 'esnext',
    chunkSizeWarningLimit: 8000,
    rollupOptions: {
      input: {
        index: resolve('src/renderer/index.html'),
        remote: resolve('src/renderer/remote.html')
      }
    }
  }
})
