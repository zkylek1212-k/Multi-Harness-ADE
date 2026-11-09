// The real app, the real React UI, the real backend: node scripts/check-desktop.mjs [debug|release]
import assert from 'node:assert'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as http from 'node:http'
import * as os from 'node:os'
import * as path from 'node:path'
import { Desktop, sleep } from './lib/desktop-driver.mjs'

const profile = process.argv[2] || 'debug'
const ws = path.join(fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'wb-check-')), 'project')
const base = path.dirname(ws)
fs.mkdirSync(path.join(base, 'data'), { recursive: true })
fs.mkdirSync(path.join(ws, 'src'), { recursive: true })
fs.writeFileSync(path.join(ws, 'README.md'), '# hi\n')
fs.writeFileSync(path.join(ws, 'src', 'a.ts'), 'export const a = 1\n')
execFileSync('git', ['init', '-b', 'main'], { cwd: ws, stdio: 'ignore' })
fs.writeFileSync(path.join(base, 'data', 'settings.json'), JSON.stringify({ lastWorkspace: ws, recentWorkspaces: [ws], language: 'en' }))
const d = new Desktop({ profile, base })

let passed = 0
const check = async (name, fn) => {
  await fn()
  passed++
  console.log(`ok - ${name}`)
}

try {
  const p = await d.page()
  await check('page loads and window.api exists', () => p.waitFor(`document.querySelector('#root')?.children.length > 0 && !!window.api`))
  await check('window frame present', async () => {
    assert.equal(await p.eval(`document.querySelectorAll('#wb-window-controls button').length`), 3)
    await p.waitFor(`getComputedStyle(document.querySelector('.app-unified-header')).getPropertyValue('-webkit-app-region') === 'drag'`)
  })
  await check('workspace reaches the page', async () => {
    assert.equal(await p.eval(`window.api.files.workspaceRoot()`), ws)
    assert.equal(await p.eval(`new URLSearchParams(location.search).get('workspace')`), ws)
  })
  await check('files through the real IPC', async () => {
    const list = await p.eval(`window.api.files.list(${JSON.stringify(ws)})`)
    assert.deepEqual(list.map((x) => x.name).filter((n) => n !== '.workbench').sort(), ['README.md', 'src'])
    await p.eval(`window.api.files.write(${JSON.stringify(path.join(ws, 'new.txt'))}, '內容')`)
    assert.equal(fs.readFileSync(path.join(ws, 'new.txt'), 'utf8'), '內容')
    await assert.rejects(p.eval(`window.api.files.read('C:\\\\Windows\\\\win.ini')`), /Access denied/)
  })
  await check('external change reaches the page', async () => {
    await p.eval(`(window.__changes = [], window.api.files.onExternalChange((c) => window.__changes.push(c.relativePath)), true)`)
    await sleep(300)
    fs.writeFileSync(path.join(ws, 'src', 'b.ts'), 'changed\n')
    await p.waitFor(`window.__changes.includes('src/b.ts')`)
  })
  await check('settings and git', async () => {
    assert.equal((await p.eval(`window.api.settings.get()`)).language, 'en')
    assert.equal((await p.eval(`window.api.git.status()`)).isRepo, true)
  })
  await check('terminal: output, input order, resize, exit', async () => {
    const id = await p.eval(`window.api.pty.spawn({ command: 'cmd', cols: 100, rows: 30 })`)
    await p.eval(`(window.__t = { out: '', exit: null, size: null },
      window.api.pty.onData(${JSON.stringify(id)}, (s) => window.__t.out += s),
      window.api.pty.onExit(${JSON.stringify(id)}, (c) => window.__t.exit = c),
      window.api.pty.onResized(${JSON.stringify(id)}, (c, r) => window.__t.size = [c, r]), true)`)
    // What was printed before attaching comes back in the snapshot, the way the terminal panel gets it
    await p.eval(`window.api.pty.attach(${JSON.stringify(id)}).then((a) => { window.__t.out = a.snapshot + window.__t.out; return true })`)
    try {
      await p.waitFor(`window.__t.out.length > 0`, 30000)
    } catch (e) {
      throw new Error(`${e.message}\nno output and an empty snapshot: the terminal itself did not come up`)
    }
    // Every keystroke its own message: order must hold
    await p.eval(`(() => { for (const c of 'echo 0123456789abcdefghijklmnop\\r') window.api.pty.write(${JSON.stringify(id)}, c); return true })()`)
    // On the screen, not in the raw stream: ConPTY repaints differently depending on timing, and may split a line with cursor moves.
    // The typed line and the command's output must both show the whole marker, in order.
    const markerSeen = `(window.__t.out.replace(/\\x1b\\[[0-9;?]*[A-Za-z]/g, '').split('0123456789abcdefghijklmnop').length - 1) >= 2`
    try {
      await p.waitFor(markerSeen, 30000)
    } catch (e) {
      throw new Error(`${e.message}\nterminal output: ${JSON.stringify(await p.eval('window.__t.out'))}`)
    }
    await p.eval(`(window.api.pty.resize(${JSON.stringify(id)}, 90, 20), true)`)
    await p.waitFor(`JSON.stringify(window.__t.size) === '[90,20]'`)
    await p.eval(`(window.api.pty.write(${JSON.stringify(id)}, 'exit\\r'), true)`)
    await p.waitFor(`window.__t.exit !== null`)
  })
  await check('webview element: outside content in a child webview without IPC', async () => {
    // The page itself tries, and tells the test server what happened
    let told = null
    let popup = null
    const site = http.createServer((q, s) => {
      if (q.url.startsWith('/report')) {
        told = new URL(q.url, 'http://x').searchParams.get('r')
        popup = new URL(q.url, 'http://x').searchParams.get('p')
        s.end()
        return
      }
      s.setHeader('Content-Type', 'text/html')
      s.end(`<script>(async () => { let r; try { const t = window.__TAURI_INTERNALS__; if (!t) r = 'no-internals'; else { await t.invoke('ipc', { channel: 'settings:get', args: [] }); r = 'allowed' } } catch (e) { r = 'denied' } let p; try { p = window.open('/popup') ? 'opened' : 'blocked' } catch (e) { p = 'blocked' } await fetch('/report?r=' + r + '&p=' + p) })()</script>`)
    })
    await new Promise((r) => site.listen(0, '127.0.0.1', r))
    const url = `http://127.0.0.1:${site.address().port}/`
    await p.eval(`(() => { const e = document.createElement('wb-webview'); e.id = 'w2'; e.setAttribute('src', ${JSON.stringify(url)}); e.style.cssText = 'position:fixed;left:0;top:100px;width:300px;height:200px'; document.body.appendChild(e); return true })()`)
    for (let i = 0; i < 100 && told === null; i++) await sleep(100)
    // A window holding a child webview is still the same window to the backend
    assert.equal(await p.eval(`window.api.files.workspaceRoot()`), ws, 'the window still counts as open')
    const list = await p.eval(`window.__TAURI__.window.getCurrentWindow().label`)
    assert.match(list, /^w\d+$/)
    const before = await p.eval(`window.innerWidth`)
    await p.eval(`window.__TAURI__.core.invoke('window_control', { action: 'toggle-maximize' })`)
    await p.waitFor(`window.innerWidth !== ${before}`)
    await p.eval(`window.__TAURI__.core.invoke('window_control', { action: 'toggle-maximize' })`)
    await p.waitFor(`window.innerWidth === ${before}`)
    await p.eval(`(document.getElementById('w2').remove(), true)`)
    site.close()
    assert.ok(told === 'denied' || told === 'no-internals', `child webview IPC: ${told}`)
    assert.equal(popup, 'blocked', 'the embedded browser opens no popups')
  })
  await check('webview element: a data: URL is refused', async () => {
    await p.eval(`(() => { const e = document.createElement('wb-webview'); e.id = 'w1'; e.setAttribute('src', 'data:text/html,hi'); e.style.cssText = 'position:fixed;left:50px;top:80px;width:200px;height:100px;z-index:999999'; document.body.appendChild(e); return true })()`)
    await sleep(600)
    await p.eval(`(document.getElementById('w1').remove(), true)`)
  })
  await check('the UI itself renders the dashboard or file tree', () => p.waitFor(`document.body.innerText.length > 50`))
  await check('WebGL for the terminal renderer', async () => {
    assert.equal(await p.eval(`!!document.createElement('canvas').getContext('webgl2')`), true)
  })
  await check('clipboard works in this origin', async () => {
    assert.equal(await p.eval(`window.isSecureContext && !!navigator.clipboard`), true)
  })
  await check('remote control serves the phone app from the bundle', async () => {
    const port = 47950
    await p.eval(`window.api.remote.setPort(${port})`)
    const st = await p.eval(`window.api.remote.setEnabled(true)`)
    assert.equal(st.running, true, JSON.stringify(st))
    const https = await import('node:https')
    const get = (u) => new Promise((res, rej) => https.get({ host: '127.0.0.1', port, path: u, rejectUnauthorized: false, agent: false }, (r) => { let b = ''; r.on('data', (c) => (b += c)); r.on('end', () => res({ status: r.statusCode, body: b })) }).on('error', rej))
    const page = await get('/')
    assert.equal(page.status, 200)
    assert.match(page.body, /remote/)
    assert.equal((await get('/remote-sw.js')).status, 200)
    assert.equal((await p.eval(`window.api.remote.setEnabled(false)`)).running, false)
  })
  const errors = await p.errors()
  assert.ok(errors.some((e) => /Cannot open data:/.test(e || '')), 'the data: URL was refused')
  assert.deepEqual(errors.filter((e) => !/Cannot open data:/.test(e || '')), [])
  await check('closing the window ends the app and its terminals', async () => {
    await p.eval(`window.api.pty.spawn({ command: 'cmd' })`)
    p.eval(`document.querySelector('#wb-window-controls button[data-act="close"]').click()`).catch(() => {})
    const code = await Promise.race([d.exited, sleep(15000).then(() => 'timeout')])
    assert.notEqual(code, 'timeout', 'app did not exit')
  })
  console.log(`check-desktop: ${passed} passed`)
} catch (e) {
  console.error(e)
  console.error('--- app log ---\n' + d.log)
  process.exitCode = 1
} finally {
  d.cleanup()
}
