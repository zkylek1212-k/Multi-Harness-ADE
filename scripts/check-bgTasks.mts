// 自我檢查：node --experimental-strip-types scripts/check-bgTasks.mts
import assert from 'node:assert'
import { coreCall } from './lib/core-call.mts'

const claudeBgTasks = (t: string): any => coreCall('claudeBgTasks', t)
const codexBgTasks = (t: string): any => coreCall('codexBgTasks', t)
const antigravityBgTasks = (t: string): any => coreCall('antigravityBgTasks', t)

const j = (o: unknown): string => JSON.stringify(o)

// ---------- Claude ----------
const use = (id: string, input: object, ts: string, name = 'Bash'): string =>
  j({ timestamp: ts, message: { content: [{ type: 'tool_use', id, name, input }] } })
const result = (id: string, content: unknown): string =>
  j({ timestamp: 't', message: { content: [{ type: 'tool_result', tool_use_id: id, content }] } })
const notify = (id: string, status: string, ts: string): string =>
  j({
    timestamp: ts,
    message: {
      content: `<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>${id}</tool-use-id>\n<status>${status}</status>\n<summary>Background command "x" ${status}</summary>\n</task-notification>`
    }
  })

assert.deepEqual(
  claudeBgTasks(
    [
      use('toolu_a', { command: 'npm run dev', description: 'Start dev server', run_in_background: true }, 't1'),
      result('toolu_a', 'Command running in background with ID: b1. Output is being written to: x'),
      use('toolu_b', { command: 'npm test' }, 't2'),
      result('toolu_b', 'ok'), // 前景指令不算
      use('toolu_d', { command: 'grep x' }, 't2'),
      result('toolu_d', 'log: Command running in background with ID: b9'), // 輸出裡出現這句也不算
      use('toolu_e', { command: 'grep -o "Async agent launched.*" x' }, 't2'),
      result('toolu_e', 'Async agent launched successfully. (grep 輸出)'), // Bash 輸出開頭剛好是這句也不算
      use('toolu_c', { description: 'Review code' }, 't3', 'Agent'),
      result('toolu_c', [{ type: 'text', text: 'Async agent launched successfully. agentId: a1' }]),
      notify('toolu_c', 'completed', 't4'),
      'not json'
    ].join('\n')
  ),
  [
    { agent: 'claude', id: 'toolu_a', desc: 'Start dev server', startedAt: 't1', status: 'running' },
    { agent: 'claude', id: 'toolu_c', desc: 'Review code', startedAt: 't3', status: 'completed', summary: 'Background command "x" completed', endedAt: 't4' }
  ]
)

// ---------- Codex ----------
const call = (id: string, cmds: string[], ts: string): string =>
  j({
    timestamp: ts,
    type: 'response_item',
    payload: {
      type: 'custom_tool_call',
      name: 'exec',
      call_id: id,
      input: cmds.map((c) => `await tools.exec_command({cmd:${JSON.stringify(c)},yield_time_ms:1000});`).join('\n')
    }
  })
const out = (id: string, ts: string): string =>
  j({ timestamp: ts, type: 'response_item', payload: { type: 'custom_tool_call_output', call_id: id, output: [] } })
const exec = (cmd: string, exit: number, ts: string): string =>
  j({
    timestamp: ts,
    type: 'event_msg',
    payload: {
      type: 'item_completed',
      item: { type: 'CommandExecution', command: ['powershell.exe', '-Command', cmd], status: exit ? 'failed' : 'completed', exit_code: exit }
    }
  })

assert.deepEqual(
  codexBgTasks(
    [
      call('c1', ['git status', 'npm run build'], 't1'),
      exec('git status', 0, 't2'), // 呼叫回傳前就結束＝前景
      out('c1', 't3'), // npm run build 還在跑＝背景
      call('c2', ['npm run dev\nsecond line'], 't4'),
      out('c2', 't5'),
      exec('npm run build', 1, 't6'),
      call('c3', ['rg x'], 't7'),
      j({ timestamp: 't8', payload: { type: 'custom_tool_call_output', call_id: 'c3', output: [{ type: 'input_text', text: 'Script failed\n' }] } }) // 沒跑起來不算
    ].join('\n')
  ),
  [
    { agent: 'codex', id: 'c1-0', desc: 'npm run build', startedAt: 't1', status: 'failed', summary: 'exit code 1', endedAt: 't6' },
    { agent: 'codex', id: 'c2-0', desc: 'npm run dev', startedAt: 't4', status: 'running' }
  ]
)

// ---------- Antigravity ----------
const ag = (type: string, content: string, ts: string): string => j({ type, content, created_at: ts })
assert.deepEqual(
  antigravityBgTasks(
    [
      ag('GENERIC', 'Created At: x\nTool is running as a background task with task id: s/task-2\nTask Description: npm run build\nTask logs…', 't1'),
      ag('GENERIC', 'Created At: x\nTool is running as a background task with task id: s/task-5\nTask Description: npm run dev\n', 't2'),
      ag('SYSTEM_MESSAGE', '<SYSTEM_MESSAGE>\ncontent=Task id "s/task-2" finished with result:\n\nThe command exited with code 1.\nOutput:', 't3'),
      ag('GENERIC', 'Created At: x\nTool is running as a background task with task id: s/task-6\nTask Description: Timer: 2s, Prompt: wait\n', 't2'), // 等待計時器不算
      ag('GENERIC', 'Created At: x\nTool is running as a background task with task id: s/task-7\nTask Description: dir /s\n', 't2'),
      ag('SYSTEM_MESSAGE', 'content=Task id "s/task-7" was canceled with result:\nTool execution was canceled', 't3'),
      ag('GENERIC', 'echo "Tool is running as a background task with task id: fake"', 't4') // 不是固定開頭不算
    ].join('\n')
  ),
  [
    { agent: 'antigravity', id: 's/task-2', desc: 'npm run build', startedAt: 't1', status: 'failed', summary: 'exit code 1', endedAt: 't3' },
    { agent: 'antigravity', id: 's/task-5', desc: 'npm run dev', startedAt: 't2', status: 'running' },
    { agent: 'antigravity', id: 's/task-7', desc: 'dir /s', startedAt: 't2', status: 'killed', endedAt: 't3' }
  ]
)
console.log('check-bgTasks: ok')
