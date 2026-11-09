import { createElement, forwardRef } from 'react'

// Electron has <webview>. The Tauri build registers wb-webview (src/desktop/platform.ts) with the same methods and events.
const tag = '__TAURI__' in window ? 'wb-webview' : 'webview'

export const Webview = forwardRef<any, any>(function Webview({ className, ...rest }, ref) {
  // A custom element takes class, not className
  return createElement(tag, { ...rest, ref, class: className, className })
})
