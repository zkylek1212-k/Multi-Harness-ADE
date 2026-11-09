// Starts the real desktop app and drives its pages through the test channel in debug/e2e builds. No mock in the path.
import { spawn, execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as net from 'node:net'
import * as os from 'node:os'
import * as path from 'node:path'
import { createInterface } from 'node:readline'

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

export function desktopExe(profile = 'debug') {
  if (process.env.WB_DESKTOP_PATH) return process.env.WB_DESKTOP_PATH
  const target = process.env.CARGO_TARGET_DIR || path.join(process.env.LOCALAPPDATA || '', 'wb-cargo-target')
  return path.join(target, profile, 'agent-workbench.exe')
}

class Page {
  constructor(desktop, label) {
    this.desktop = desktop
    this.label = label
  }

  /** Run an expression in the page (it may be a promise) and return its JSON value; a rejection throws here */
  eval(js) {
    return this.desktop.request({ label: this.label, js })
  }

  async waitFor(js, ms = 20000) {
    const until = Date.now() + ms
    let last
    while (Date.now() < until) {
      try {
        last = await Promise.race([this.eval(js), sleep(3000).then(() => 'eval timed out')])
        if (last && last !== 'eval timed out') return last
      } catch (e) {
        last = e.message
      }
      await sleep(100)
    }
    throw new Error(`timed out waiting for ${js} (last: ${JSON.stringify(last)})`)
  }

  errors() {
    return this.eval('window.__wbErrors')
  }
}

export class Desktop {
  constructor({ home, data, args = [], env = {}, profile = 'debug', base } = {}) {
    this.base = base || fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'wb-desktop-'))
    this.home = home || path.join(this.base, 'home')
    this.data = data || path.join(this.base, 'data')
    this.port = 20000 + Math.floor(Math.random() * 20000)
    fs.mkdirSync(this.home, { recursive: true })
    fs.mkdirSync(this.data, { recursive: true })
    this.child = spawn(desktopExe(profile), args, {
      env: {
        ...process.env,
        USERPROFILE: this.home,
        WB_DATA_DIR: this.data,
        // Every run gets its own browser profile
        WEBVIEW2_USER_DATA_FOLDER: path.join(this.base, 'wv2'),
        WB_E2E_PORT: String(this.port),
        ...env
      },
      stdio: ['ignore', 'pipe', 'pipe']
    })
    this.log = ''
    this.child.stdout.on('data', (d) => (this.log = (this.log + d).slice(-20000)))
    this.child.stderr.on('data', (d) => (this.log = (this.log + d).slice(-20000)))
    this.exited = new Promise((r) => this.child.on('exit', (c) => r(c)))
    this.nextId = 1
    this.pending = new Map()
  }

  async connect() {
    if (this.socket) return
    const until = Date.now() + 40000
    for (;;) {
      try {
        this.socket = await new Promise((res, rej) => {
          const s = net.connect(this.port, '127.0.0.1', () => res(s))
          s.once('error', rej)
        })
        break
      } catch (e) {
        if (Date.now() > until) throw new Error(`no test channel (${e.message}); log:\n${this.log}`)
        await sleep(200)
      }
    }
    this.socket.on('error', () => {})
    this.socket.on('close', () => {
      for (const p of this.pending.values()) p.reject(new Error('app closed'))
      this.pending.clear()
    })
    createInterface({ input: this.socket }).on('error', () => {}).on('line', (l) => {
      const m = JSON.parse(l)
      const p = this.pending.get(m.id)
      if (!p) return
      this.pending.delete(m.id)
      if (m.ok) p.resolve(m.value)
      else p.reject(new Error(m.value))
    })
  }

  async request(msg) {
    await this.connect()
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.socket.write(JSON.stringify({ id, ...msg }) + '\n')
    })
  }

  windows() {
    return this.request({ list: true })
  }

  /** A page once its script has loaded and the app rendered */
  async page(label = 'w1', ms = 40000) {
    const until = Date.now() + ms
    let last
    while (Date.now() < until) {
      try {
        if ((await this.windows()).includes(label)) {
          const p = new Page(this, label)
          await p.waitFor(`!!window.api && !!window.__TAURI__`, 20000)
          return p
        }
      } catch (e) {
        last = e
      }
      await sleep(200)
    }
    throw new Error(`no page ${label} (${last?.message}); log:\n${this.log}`)
  }

  kill() {
    try {
      this.socket?.destroy()
      execFileSync('taskkill', ['/PID', String(this.child.pid), '/T', '/F'], { stdio: 'ignore' })
    } catch {}
  }

  cleanup() {
    this.kill()
    try {
      fs.rmSync(this.base, { recursive: true, force: true, maxRetries: 8, retryDelay: 300 })
    } catch {} // WebView2 can hold its folder a moment longer; it is only a temp folder
  }
}
