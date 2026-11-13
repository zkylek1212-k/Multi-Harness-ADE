// A stand-in agent for check-team.mts, started through cliPaths as "claude". Like Claude, it starts the MCP servers
// from --mcp-config and talks MCP to them over stdio, so the real `wb-hook mcp` and the real pipe are in the path.
// Typed lines (Enter runs one):
//   call <tool> <json>    call a team tool
//   [Agent Workbench] ... the app's notice: call read_inbox, then run every "DO: <tool> <json>" line found in the
//                         messages, the way a model follows its task
// Every result goes to <tmp>/wb-team-fake/<hex of cwd>.jsonl as {kind, tool, ok, text}: a long text would wrap on
// screen, and the worktree must stay clean for merges. The screen shows short lines and a "> " prompt, never a menu.
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const logDir = path.join(os.tmpdir(), 'wb-team-fake')
fs.mkdirSync(logDir, { recursive: true })
const logFile = path.join(logDir, Buffer.from(process.cwd().toLowerCase()).toString('hex').slice(-120) + '.jsonl')
const record = (o) => fs.appendFileSync(logFile, JSON.stringify(o) + '\n')

const at = process.argv.indexOf('--mcp-config')
const config = at >= 0 ? JSON.parse(fs.readFileSync(process.argv[at + 1], 'utf8')) : null
const server = config?.mcpServers?.workbench
const out = (s) => process.stdout.write(s)

let child = null
let nextId = 1
const pending = new Map()
function rpc(method, params) {
  const id = nextId++
  return new Promise((resolve) => {
    pending.set(id, resolve)
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
  })
}
if (server) {
  child = spawn(server.command, server.args || [], { stdio: ['pipe', 'pipe', 'inherit'] })
  let buf = ''
  child.stdout.on('data', (d) => {
    buf += d
    let i
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i)
      buf = buf.slice(i + 1)
      try {
        const m = JSON.parse(line)
        pending.get(m.id)?.(m)
        pending.delete(m.id)
      } catch {}
    }
  })
}

const call = async (tool, args) => {
  const r = await rpc('tools/call', { name: tool, arguments: args })
  const text = r.result?.content?.[0]?.text ?? JSON.stringify(r.error)
  const isOk = !r.result?.isError
  record({ kind: 'RESULT', tool, ok: isOk, text })
  out(`RESULT ${tool} ${isOk ? 'ok' : 'err'}\r\n`)
  return text
}

const start = async () => {
  if (!child) return out('FAKE-TEAM no-mcp\r\n> ')
  await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'fake', version: '1' } })
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')
  const list = await rpc('tools/list', {})
  out(`FAKE-TEAM ready tools=${(list.result?.tools || []).map((t) => t.name).join(',')}\r\n> `)
}

process.stdin.setRawMode?.(true)
process.stdin.resume()
let line = ''
let queue = Promise.resolve()
process.stdin.on('data', (d) => {
  for (const ch of d.toString()) {
    if (ch === '\r') {
      const cmd = line.trim()
      line = ''
      queue = queue.then(() => run(cmd)).catch((e) => (record({ kind: 'ERROR', text: String(e) }), out('ERROR\r\n> ')))
    } else if (ch >= ' ') {
      line += ch
      out(ch)
    }
  }
})

async function run(cmd) {
  out('\r\n')
  if (cmd.startsWith('[Agent Workbench]')) {
    const inbox = await call('read_inbox', {})
    record({ kind: 'INBOX', text: inbox })
    out('INBOX read\r\n')
    for (const m of inbox.matchAll(/^DO: (\w+) (\{.*\})$/gm)) await call(m[1], JSON.parse(m[2]))
  } else if (cmd.startsWith('call ')) {
    const [, tool, json] = /^call (\w+) ?(.*)$/.exec(cmd) || []
    await call(tool, json ? JSON.parse(json) : {})
  }
  out('> ')
}

await start()
