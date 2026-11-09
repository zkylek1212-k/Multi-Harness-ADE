// Load on the real app: node scripts/bench-desktop.mjs [release|debug]
// Needs a build with the test channel: npx tauri build --no-bundle --features e2e (run in core/apps/desktop). N fake CLIs at 256 KiB/s
// each. Single runs, so read the order of magnitude.
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { Desktop, sleep } from './lib/desktop-driver.mjs'

const profile = process.argv[2] || 'release'
const flood = path.resolve('scripts/bench/flood.cjs')
const mem = path.resolve('scripts/bench/mem.ps1')
const p99 = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length * 0.99)] ?? 0

for (const n of [1, 4, 10]) {
  const t0 = Date.now()
  const d = new Desktop({ profile })
  fs.writeFileSync(path.join(d.data, 'settings.json'), JSON.stringify({ language: 'en' }))
  try {
    const p = await d.page()
    await p.waitFor(`document.querySelector('#root')?.children.length > 0 && !!window.api`)
    const startMs = Date.now() - t0
    await p.eval(`(window.__bytes = 0, true)`)
    for (let i = 0; i < n; i++) {
      const id = await p.eval(`window.api.pty.spawn({ command: 'cmd', cols: 120, rows: 30 })`)
      await p.eval(`(window.api.pty.onData(${JSON.stringify(id)}, (s) => { window.__bytes += s.length }), true)`)
      await p.eval(`window.api.pty.attach(${JSON.stringify(id)})`)
      await p.eval(`(window.api.pty.write(${JSON.stringify(id)}, ${JSON.stringify(`"${process.execPath}" "${flood}" 262144 40\r`)}), true)`)
    }
    await sleep(10000)
    // Round trip of a control message while the output is flowing, measured inside the page
    const rtt = await p.eval(`(async () => { const out = []; for (let i = 0; i < 200; i++) { const t = performance.now(); await window.api.files.workspaceRoot(); out.push(performance.now() - t); await new Promise((r) => setTimeout(r, 50)) } return out })()`)
    const before = await p.eval(`window.__bytes`)
    await sleep(2000)
    const rate = ((await p.eval(`window.__bytes`)) - before) / 2 / 1024
    const m = JSON.parse(execFileSync('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', mem, '-RootPid', String(d.child.pid)], { encoding: 'utf8' }))
    console.log(JSON.stringify({ terminals: n, start_ms: startMs, rtt_p99_ms: Math.round(p99(rtt) * 10) / 10, received_kib_s: Math.round(rate), ...m }))
  } finally {
    d.cleanup()
    await sleep(1500)
  }
}
