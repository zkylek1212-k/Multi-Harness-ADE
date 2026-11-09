// What the UI checks used from Electron, on headless Edge: the same Chromium engine WebView2 runs, and on every Windows machine.
const { spawn } = require('node:child_process')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const WebSocket = require('ws')

const EDGE = process.env.WB_EDGE_PATH || ['C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 'C:/Program Files/Microsoft/Edge/Application/msedge.exe'].find((p) => fs.existsSync(p))
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-edge-'))
let browser = null
let session = null

function cleanup() {
  try {
    browser?.kill()
  } catch {
    /* already gone */
  }
  for (let i = 0; i < 5; i++) {
    try {
      fs.rmSync(profile, { recursive: true, force: true })
      return
    } catch {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200)
    }
  }
}

function start() {
  if (session) return session
  session = new Promise((resolve, reject) => {
    if (!EDGE) return reject(new Error('Microsoft Edge not found; set WB_EDGE_PATH'))
    browser = spawn(EDGE, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--allow-file-access-from-files', '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] })
    let err = ''
    browser.stderr.on('data', (d) => {
      err += d
      const m = /DevTools listening on (ws:\S+)/.exec(err)
      if (m) resolve(m[1])
    })
    browser.on('exit', (c) => reject(new Error(`Edge exited early (${c}): ${err.slice(0, 500)}`)))
  })
  return session
}

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url, { perMessageDeflate: false, maxPayload: 512 * 1024 * 1024 })
    this.opened = new Promise((res, rej) => this.ws.once('open', res).once('error', rej))
    this.next = 1
    this.waiting = new Map()
    this.listeners = new Map()
    this.ws.on('message', (raw) => {
      const m = JSON.parse(raw.toString())
      if (m.id) {
        const w = this.waiting.get(m.id)
        this.waiting.delete(m.id)
        if (m.error) w?.reject(new Error(`${m.error.message} (${w.method})`))
        else w?.resolve(m.result)
      } else {
        for (const f of this.listeners.get(m.method) || []) f(m.params)
      }
    })
    this.ws.on('close', () => {
      for (const w of this.waiting.values()) w.reject(new Error('browser closed'))
      this.waiting.clear()
    })
    this.ws.on('error', () => {})
  }
  async send(method, params = {}) {
    await this.opened
    return new Promise((resolve, reject) => {
      const id = this.next++
      this.waiting.set(id, { resolve, reject, method })
      this.ws.send(JSON.stringify({ id, method, params }))
    })
  }
  on(method, f) {
    if (!this.listeners.has(method)) this.listeners.set(method, [])
    this.listeners.get(method).push(f)
  }
  once(method, f) {
    const list = this.listeners.get(method) || []
    const wrap = (p) => {
      list.splice(list.indexOf(wrap), 1)
      f(p)
    }
    list.push(wrap)
    this.listeners.set(method, list)
  }
}

const levels = { debug: 0, log: 0, info: 1, warning: 2, error: 3, assert: 3 }

class WebContents {
  constructor(win, opts) {
    this.win = win
    this.opts = opts
    this.handlers = new Map()
    this.ready = this.init()
    this.debugger = { attach: () => {}, detach: () => {}, sendCommand: async (m, p) => (await this.ready).send(m, p) }
  }
  async init() {
    const http = require('node:http')
    const wsUrl = await start()
    const u = new URL(wsUrl)
    const page = await new Promise((resolve, reject) => {
      const r = http.request({ host: u.hostname, port: u.port, path: '/json/new?about:blank', method: 'PUT' }, (res) => {
        let b = ''
        res.on('data', (c) => (b += c))
        res.on('end', () => resolve(JSON.parse(b)))
      })
      r.on('error', reject)
      r.end()
    })
    const cdp = new Cdp(page.webSocketDebuggerUrl)
    this.targetId = page.id
    await cdp.send('Page.enable')
    await cdp.send('Runtime.enable')
    await cdp.send('Log.enable')
    const emit = (level, message) => {
      for (const f of this.handlers.get('console-message') || []) f({}, level, message, 0, '')
    }
    cdp.on('Runtime.consoleAPICalled', (p) => emit(levels[p.type] ?? 0, p.args.map((a) => a.value ?? a.description ?? '').join(' ')))
    cdp.on('Runtime.exceptionThrown', (p) => emit(3, p.exceptionDetails.exception?.description || p.exceptionDetails.text))
    const preload = this.opts?.webPreferences?.preload
    if (preload) await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: fs.readFileSync(preload, 'utf8') })
    const { width = 800, height = 600 } = this.opts || {}
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false })
    return cdp
  }
  on(name, f) {
    if (!this.handlers.has(name)) this.handlers.set(name, [])
    this.handlers.get(name).push(f)
  }
  async executeJavaScript(code) {
    const cdp = await this.ready
    const r = await cdp.send('Runtime.evaluate', { expression: code, awaitPromise: true, returnByValue: false, userGesture: true })
    if (r.exceptionDetails) {
      const e = r.exceptionDetails
      throw new Error(e.exception?.description || e.text)
    }
    if (!r.result.objectId) return r.result.value
    // A result too tangled to copy (a live terminal, a DOM node) comes back as an empty object, as it did in Electron, instead of failing the script that produced it
    const v = await cdp.send('Runtime.callFunctionOn', { objectId: r.result.objectId, functionDeclaration: 'function () { try { return JSON.parse(JSON.stringify(this)) } catch { return {} } }', returnByValue: true })
    return v.result.value
  }
  /**
   * Waits for the page to stop changing, at least `min` ms. A fixed sleep is a guess about how fast the machine is:
   * on a slow one the test reads a screen that has not rendered yet.
   */
  async settled(min = 100) {
    await new Promise((r) => setTimeout(r, min))
    await this.executeJavaScript(`new Promise((done) => {
      let quiet
      const finish = () => { clearTimeout(quiet); clearTimeout(cap); observer.disconnect(); done(true) }
      const observer = new MutationObserver(() => { clearTimeout(quiet); quiet = setTimeout(finish, 150) })
      const cap = setTimeout(finish, 5000)
      observer.observe(document, { subtree: true, childList: true, characterData: true, attributes: true })
      quiet = setTimeout(finish, 150)
    })`)
  }
  async capturePage() {
    const cdp = await this.ready
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' })
    const png = Buffer.from(data, 'base64')
    return { toPNG: () => png, getSize: () => ({ width: png.readUInt32BE(16), height: png.readUInt32BE(20) }) }
  }
  isDestroyed() {
    return !!this.gone
  }
}

class BrowserWindow {
  constructor(opts = {}) {
    this.webContents = new WebContents(this, opts)
  }
  async loadURL(url) {
    const cdp = await this.webContents.ready
    const loaded = new Promise((res) => cdp.once('Page.loadEventFired', res))
    const r = await cdp.send('Page.navigate', { url })
    if (r.errorText) throw new Error(`${r.errorText}: ${url}`)
    await loaded
  }
  loadFile(file, o = {}) {
    const url = require('node:url').pathToFileURL(path.resolve(file))
    if (o.query) url.search = new URLSearchParams(o.query).toString()
    if (o.hash) url.hash = o.hash
    return this.loadURL(url.href)
  }
  async setContentSize(width, height) {
    const cdp = await this.webContents.ready
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false })
  }
  destroy() {
    this.webContents.gone = true
    this.webContents.ready.then((c) => c.ws.close()).catch(() => {})
  }
  isDestroyed() {
    return !!this.webContents.gone
  }
}

const app = {
  whenReady: () => Promise.resolve(),
  on: () => {},
  setPath: () => {},
  quit: () => app.exit(process.exitCode || 0),
  exit: (code) => {
    cleanup()
    process.exit(code)
  }
}

module.exports = { app, BrowserWindow }
