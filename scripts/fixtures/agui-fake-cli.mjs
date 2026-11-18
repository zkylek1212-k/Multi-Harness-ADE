// A stand-in CLI for scripts/check-agui.mts. No AI: it answers in the shapes the real CLIs use (P0 recordings).
//   app-server generate-json-schema --out DIR  -> a Codex schema that lacks thread/resume (a fork too old for chat mode)
//   -p ... stream-json [--resume ID]           -> Claude's stream-json: echoes each prompt, asks once when told "ask"
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

let n = 0
let waiting = null
createInterface({ input: process.stdin }).on('line', (line) => {
  const m = JSON.parse(line)
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
  if (text.includes('ask')) {
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'message_start', message: { id: `m${++n}` } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_start', index: 0, content_block: { type: 'tool_use', id: 'tool-1', name: 'Bash' } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: '{"command":"rm x"}' } } })
    out({ type: 'stream_event', parent_tool_use_id: null, event: { type: 'content_block_stop', index: 0 } })
    waiting = 'req-1'
    out({ type: 'control_request', request_id: 'req-1', request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'rm x' } } })
    return
  }
  say(`m${++n}`, `${resumed && n === 1 ? `resumed:${resumed} ` : ''}echo:${text}`)
  done()
})
