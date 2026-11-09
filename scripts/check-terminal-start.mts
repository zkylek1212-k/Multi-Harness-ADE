// node --experimental-strip-types scripts/check-terminal-start.mts [count]
// Every terminal has to come up and take input, including on a slow machine: ConPTY stays silent until its startup query is answered,
// so a race there would show as a blank terminal. Reads the backend's screen, so it needs no page.
import assert from 'node:assert/strict'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend } from './lib/core-call.mts'

const count = Number(process.argv[2] || 30)
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-term-start-'))
const b = new Backend({ WB_DATA_DIR: path.join(tmp, 'data') })

const screen = (id: string): Promise<string> => b.ipc('test:ptyScreen', id)
const until = async (cond: () => Promise<boolean>, ms: number): Promise<boolean> => {
  const t0 = Date.now()
  while (Date.now() - t0 < ms) {
    if (await cond()) return true
    await new Promise((r) => setTimeout(r, 25))
  }
  return false
}

const slowest = { prompt: 0, input: 0 }
const failures: string[] = []
try {
  await b.ipc('test:openWindow', tmp)
  for (let i = 0; i < count; i++) {
    const id = await b.ipc('pty:spawn', { command: 'cmd', cols: 100, rows: 30 })
    const t0 = Date.now()
    if (!(await until(async () => (await screen(id)).includes('>'), 30000))) {
      failures.push(`#${i} no prompt after 30 s, screen: ${JSON.stringify(await screen(id))}`)
      await b.ipc('pty:kill', id)
      continue
    }
    slowest.prompt = Math.max(slowest.prompt, Date.now() - t0)
    // Typed the way a person types: one message per key, right away
    const marker = `marker${i}x0123456789`
    const t1 = Date.now()
    for (const c of `echo ${marker}\r`) await b.ipc('pty:write', id, c)
    // Once as typed, once as the command's output
    if (!(await until(async () => (await screen(id)).split(marker).length - 1 >= 2, 30000))) {
      failures.push(`#${i} input did not arrive in order, screen: ${JSON.stringify(await screen(id))}`)
    }
    slowest.input = Math.max(slowest.input, Date.now() - t1)
    await b.ipc('pty:kill', id)
  }
  assert.deepEqual(failures, [])
  console.log(`terminal start: ${count} terminals came up and took input (slowest prompt ${slowest.prompt} ms, input ${slowest.input} ms)`)
} finally {
  b.stop()
  await new Promise((r) => setTimeout(r, 300))
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3 })
}
