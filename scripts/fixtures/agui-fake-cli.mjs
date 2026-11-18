// A stand-in CLI for scripts/check-agui.mts. No AI: it answers in the shapes the real CLIs use (P0 recordings).
//   app-server generate-json-schema --out DIR  -> a Codex schema that lacks thread/resume (a fork too old for chat mode)
//   -p ... stream-json [--resume ID]           -> Claude's stream-json: echoes each prompt, asks once when told "ask"
// With --mcp-config it starts the `workbench` server like Claude does and talks MCP to it, so the real `wb-hook mcp`
// and pipe are in the path. A prompt is then "followed" the way a model would:
//   a line `DO: <tool> <json>`      calls that tool and answers "<tool> ok|err <result>" (only at a line start, so a
//                                   DO inside a task's details waits for whoever gets the task)
//   a board task titled CLAIM...    claim_task on it
//   a free-talk message             PASS unless the user's words name this bot's role
import { spawn } from 'node:child_process'
import * as fs from 'node:fs'
import * as path from 'node:path'
import { createInterface } from 'node:readline'

const argv = process.argv.slice(2)
if (argv[0] === 'app-server' && argv[1] === 'generate-json-schema') {
  const out = argv[argv.indexOf('--out') + 1]
  const write = (f, names) => fs.writeFileSync(path.join(out, f), JSON.stringify({ enum: names }))
  write('ClientRequest.json', ['initialize', 'thread/start', 'turn/start', 'turn/interrupt'])
  write('ServerNotification.json', ['turn/started', 'turn/completed', 'item/started', 'item/completed', 'item/agentMessage/delta', 'thread/tokenUsage/updated'])
  write('ServerRequest.json', ['item/commandExecution/requestApproval', 'item/fileChange/requestApproval'])
  write('CommandExecutionRequestApprovalResponse.json', ['accept', 'acceptForSession', 'decline'])
  process.exit(0)
}

const resumed = argv.includes('--resume') ? argv[argv.indexOf('--resume') + 1] : null
const session = resumed || 'fake-session-1'
const out = (m) => process.stdout.write(JSON.stringify(m) + '\n')
const say = (id, text) => {
  out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'message_start', message: { id } } })
  out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_start', index: 0, content_block: { type: 'text' } } })
  out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text } } })
  out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_stop', index: 0 } })
}
const done = () => out({ type: 'result', subtype: 'success', is_error: false, result: '', usage: { input_tokens: 1, output_tokens: 1 } })

// ── the workbench MCP server, as Claude starts it from --mcp-config ──
const at = argv.indexOf('--mcp-config')
const server = at >= 0 ? JSON.parse(fs.readFileSync(argv[at + 1], 'utf8')).mcpServers?.workbench : null
let mcp = null
let nextId = 1
const pending = new Map()
const rpc = (method, params) =>
  new Promise((resolve) => {
    const id = nextId++
    pending.set(id, resolve)
    mcp.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
  })
const ready = (async () => {
  if (!server) return
  mcp = spawn(server.command, server.args || [], { stdio: ['pipe', 'pipe', 'inherit'] })
  createInterface({ input: mcp.stdout }).on('line', (line) => {
    try {
      const m = JSON.parse(line)
      pending.get(m.id)?.(m)
      pending.delete(m.id)
    } catch {}
  })
  await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'fake', version: '1' } })
  mcp.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')
})()
const call = async (tool, args) => {
  const r = await rpc('tools/call', { name: tool, arguments: args })
  return `${tool} ${r.result?.isError ? 'err' : 'ok'} ${r.result?.content?.[0]?.text ?? JSON.stringify(r.error)}`
}

let n = 0
let waiting = null
async function take(m) {
  if (m.type === 'control_response' && waiting === m.response.request_id) {
    waiting = null
    const behavior = m.response.response.behavior
    out({ type: 'user', parent_tool_use_id: null, message: { content: [{ type: 'tool_result', tool_use_id: 'tool-1', content: behavior === 'allow' ? 'removed' : 'denied' }] } })
    say(`m${++n}`, `tool ${behavior}`)
    done()
    return
  }
  if (m.type !== 'user') return
  const text = m.message.content
  if (n === 0) out({ type: 'system', subtype: 'init', session_id: session })
  if (text.includes('please ask')) {
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'message_start', message: { id: `m${++n}` } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_start', index: 0, content_block: { type: 'tool_use', id: 'tool-1', name: 'Bash' } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: '{"command":"rm x"}' } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_stop', index: 0 } })
    waiting = 'req-1'
    out({ type: 'control_request', request_id: 'req-1', request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'rm x' } } })
    return
  }
  await ready
  const free = /for your role \(([^)]*)\), reply with exactly PASS/.exec(text)
  if (free && !text.slice(0, free.index).includes(free[1])) {
    say(`m${++n}`, 'PASS')
    return done()
  }
  const acts = mcp ? [...text.matchAll(/^DO: (\w+) (.+)$/gm)].map((a) => [a[1], JSON.parse(a[2])]) : []
  const board = /^Task board: (CLAIM[^\n]*) \(task_id (\w+)\)$/m.exec(text)
  if (mcp && board) acts.push(['claim_task', { task_id: board[2] }])
  if (acts.length) {
    for (const [tool, args] of acts) say(`m${++n}`, await call(tool, args))
    return done()
  }
  say(`m${++n}`, `${resumed && n === 1 ? `resumed:${resumed} ` : ''}echo:${text}`)
  done()
}

// One prompt at a time, like the CLI's own turn loop
let queue = Promise.resolve()
createInterface({ input: process.stdin }).on('line', (line) => {
  const m = JSON.parse(line)
  queue = queue.then(() => take(m))
})
