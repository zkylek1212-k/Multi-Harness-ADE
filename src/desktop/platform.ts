// What Electron gave the page for free: window frame, external links, <webview>, and last version's localStorage.
import { ipcRenderer } from './ipc-renderer'

const invoke = (cmd: string, args: Record<string, unknown> = {}): Promise<any> => (window as any).__TAURI__.core.invoke(cmd, args)

// ── Window frame: the same 38px strip the title bar overlay used; the layout already leaves room for it ──
function installWindowControls(): void {
  const style = document.createElement('style')
  style.textContent = `
    #wb-window-controls { position: fixed; top: 0; right: 0; height: 38px; display: flex; z-index: 2147483000; color: #f5f5f7; }
    #wb-window-controls[data-theme="light"] { color: #1d1d1f; }
    #wb-window-controls button { width: 46px; height: 38px; border: 0; padding: 0; background: transparent; color: inherit; font: 10px "Segoe MDL2 Assets", "Segoe Fluent Icons", sans-serif; cursor: default; }
    #wb-window-controls button:hover { background: rgba(128,128,128,.22); }
    #wb-window-controls button[data-act="close"]:hover { background: #c42b1c; color: #fff; }
  `
  document.head.appendChild(style)
  const box = document.createElement('div')
  box.id = 'wb-window-controls'
  const add = (act: string, glyph: string, label: string): HTMLButtonElement => {
    const b = document.createElement('button')
    b.dataset.act = act
    b.textContent = glyph
    b.setAttribute('aria-label', label)
    b.tabIndex = -1
    b.addEventListener('click', () => void invoke('window_control', { action: act }))
    box.appendChild(b)
    return b
  }
  add('minimize', '', 'Minimize')
  add('toggle-maximize', '', 'Maximize')
  add('close', '', 'Close')
  document.body.appendChild(box)

  // The UI still marks drag areas with the CSS property. Nothing else reads it in a WebView2, so do it here.
  const region = (el: Element | null): string => {
    for (let e = el; e; e = e.parentElement) {
      const v = getComputedStyle(e).getPropertyValue('-webkit-app-region') || getComputedStyle(e).getPropertyValue('app-region')
      if (v === 'drag' || v === 'no-drag') return v
    }
    return 'none'
  }
  document.addEventListener(
    'mousedown',
    (e) => {
      if (e.button !== 0 || region(e.target as Element) !== 'drag') return
      e.preventDefault()
      void invoke('window_control', { action: e.detail === 2 ? 'toggle-maximize' : 'drag' })
    },
    true
  )
}

/** The theme call only ever changed the overlay's colours, and the buttons are ours now */
function installThemeHook(): void {
  const original = ipcRenderer.invoke
  ipcRenderer.invoke = (channel: string, ...args: unknown[]): Promise<any> => {
    if (channel === 'window:setTitleBarTheme') {
      document.getElementById('wb-window-controls')?.setAttribute('data-theme', String(args[0]))
      return Promise.resolve(true)
    }
    return original(channel, ...args)
  }
}

// ── External links open in the user's browser ──
function installExternalLinks(): void {
  window.open = ((url?: string | URL): null => {
    if (url) void invoke('open_external', { url: String(url) })
    return null
  }) as typeof window.open
  document.addEventListener(
    'click',
    (e) => {
      const a = (e.target as Element | null)?.closest?.('a[href]') as HTMLAnchorElement | null
      if (!a || (a.target !== '_blank' && !/^(https?:|mailto:)/i.test(a.getAttribute('href') || ''))) return
      e.preventDefault()
      void invoke('open_external', { url: a.href })
    },
    true
  )
}

// ── <webview> as a native child webview. It sits above the page, so it steps aside when something covers it. ──
let nextId = 1
const views = new Map<number, WbWebview>()

class WbWebview extends HTMLElement {
  static observedAttributes = ['src']
  private viewId = nextId++
  private created = false
  private currentUrl = ''
  private timer = 0
  private lastBounds = ''

  connectedCallback(): void {
    views.set(this.viewId, this)
    this.style.display = this.style.display || 'block'
    this.timer = window.setInterval(() => this.track(), 100)
    this.track()
  }

  disconnectedCallback(): void {
    window.clearInterval(this.timer)
    views.delete(this.viewId)
    if (this.created) void invoke('webview_close', { id: this.viewId })
    this.created = false
  }

  attributeChangedCallback(_name: string, _old: string | null, value: string | null): void {
    if (this.created && value && value !== this.currentUrl) {
      this.currentUrl = value
      void invoke('webview_navigate', { id: this.viewId, url: value })
    }
  }

  private covered(r: DOMRect): boolean {
    const points: [number, number][] = [
      [r.left + r.width / 2, r.top + r.height / 2],
      [r.left + 4, r.top + 4],
      [r.right - 4, r.top + 4],
      [r.left + 4, r.bottom - 4],
      [r.right - 4, r.bottom - 4]
    ]
    return points.some(([x, y]) => {
      const hit = document.elementFromPoint(x, y)
      return hit !== null && hit !== this && !this.contains(hit)
    })
  }

  private track(): void {
    const url = this.getAttribute('src')
    if (!url) return
    const r = this.getBoundingClientRect()
    const hidden = r.width < 2 || r.height < 2 || this.covered(r)
    const bounds = JSON.stringify(hidden ? null : [r.left, r.top, r.width, r.height])
    if (!this.created) {
      if (hidden) return
      this.created = true
      this.currentUrl = url
      this.lastBounds = bounds
      void invoke('webview_create', { id: this.viewId, url, x: r.left, y: r.top, w: r.width, h: r.height })
      return
    }
    if (bounds === this.lastBounds) return
    this.lastBounds = bounds
    void invoke('webview_bounds', { id: this.viewId, x: r.left, y: r.top, w: r.width, h: r.height, visible: !hidden })
  }

  onEvent(type: string, url: string): void {
    if (url) this.currentUrl = url
    this.dispatchEvent(new CustomEvent(type))
  }

  getURL(): string {
    return this.currentUrl
  }
  goBack(): void {
    void invoke('webview_action', { id: this.viewId, action: 'back' })
  }
  goForward(): void {
    void invoke('webview_action', { id: this.viewId, action: 'forward' })
  }
  reload(): void {
    void invoke('webview_action', { id: this.viewId, action: 'reload' })
  }
}

function installWebview(): void {
  customElements.define('wb-webview', WbWebview)
  ipcRenderer.on('webview:event', (_e, id: number, type: string, url: string) => views.get(id)?.onEvent(type, url))
}

// ── localStorage: a new origin means an empty store, so bring the previous version's layout and preferences over once ──
const MIGRATED = 'wb-legacy-storage-imported'
async function importLegacyStorage(): Promise<void> {
  try {
    if (localStorage.getItem(MIGRATED)) return
    const rows: Record<string, string> = await invoke('legacy_local_storage')
    for (const [k, v] of Object.entries(rows)) {
      if (localStorage.getItem(k) === null) localStorage.setItem(k, v)
    }
    localStorage.setItem(MIGRATED, '1')
  } catch (e) {
    console.warn('[desktop] could not import previous preferences', e)
  }
}

export async function installPlatform(): Promise<void> {
  await importLegacyStorage()
  installThemeHook()
  installExternalLinks()
  installWebview()
  if (document.body) installWindowControls()
  else window.addEventListener('DOMContentLoaded', installWindowControls)
}
