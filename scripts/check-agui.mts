// node --experimental-strip-types scripts/check-agui.mts
// Chat nodes (cowork-agui-implement.md §9.1) through the real backend (wb-ipc) with a stand-in CLI, no AI:
// the Codex protocol check refuses a fork that lacks a method, a prompt streams back into the transcript, an approval
// is answered once, live events reach the window, and a restart resumes the CLI's own session.
// Needs: cargo build -p wb-ipc.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, until } from './lib/core-call.mts'

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'wb-agui-')))
const repo = path.join(tmp, 'repo')
const data = path.join(tmp, 'data')
fs.mkdirSync(repo)
const git = (...a: string[]): string => execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim()
git('init', '-q', '-b', 'main')
git('config', 'user.name', 'check')
git('config', 'user.email', 'check@example.invalid')
fs.writeFileSync(path.join(repo, 'README.md'), 'agui\n')
git('add', '-A')
git('commit', '-qm', 'init')
const fake = path.join(tmp, 'fake-cli.cmd')
fs.writeFileSync(fake, `@echo off\r\nnode "${path.resolve('scripts/fixtures/agui-fake-cli.mjs')}" %*\r\n`)
const custom = (protocol: string): unknown => ({ customAgent: { enabled: true, name: 'Fake', command: fake, args: [], protocol, resources: { skill: '', mcp: '', plugin: '', hook: '' } } })

let b = new Backend({ WB_DATA_DIR: data })
type R = { ok: boolean; data?: any; code?: string; message?: string }
const c = (op: string, req?: unknown): Promise<R> => b.ipc(`canvas:${op}`, req ?? null)
const ok = async (op: string, req?: unknown): Promise<any> => {
  const r = await c(op, req)
  assert.equal(r.ok, true, `${op} failed: ${r.code} ${r.message}`)
  return r.data
}
const entries = async (channelId: string, nodeId: string): Promise<any[]> => (await ok('chat.transcript', { channelId, nodeId })).entries
const waitEntries = async (channelId: string, nodeId: string, cond: (e: any[]) => boolean, what: string): Promise<any[]> => {
  let last: any[] = []
  await until(async () => cond((last = await entries(channelId, nodeId))), `${what} (last: ${JSON.stringify(last).slice(0, 600)})`, 30000)
  return last
}

try {
  await b.ipc('test:openWindow', repo)

  // ── a Codex fork without thread/resume never starts a chat ──
  await b.ipc('settings:set', custom('codex-app-server'))
  const old = (await ok('channel.create', { name: 'old fork' })).channel.id
  const refused = await c('node.create', { channelId: old, agent: 'custom-agent', workspace: 'worktree', mode: 'chat' })
  assert.equal(refused.ok, false)
  assert.match(refused.message || '', /lacks: thread\/resume/)

  // ── chat: a prompt streams back; the transcript keeps whole messages ──
  await b.ipc('settings:set', custom('claude-stream-json'))
  const ch = (await ok('channel.create', { name: 'chat' })).channel.id
  const nd = (await ok('node.create', { channelId: ch, agent: 'custom-agent', workspace: 'worktree', mode: 'chat' })).id
  await ok('chat.prompt', { channelId: ch, nodeId: nd, text: 'hello' })
  await waitEntries(ch, nd, (e) => e.some((x) => x.kind === 'text' && x.text === 'echo:hello'), 'answer in the transcript')
  assert.ok(b.events.some((m) => m.event === 'canvas:agui'), 'live events reach the window')
  const list = await ok('channels.list')
  assert.equal(list.find((x: any) => x.id === ch).lead.mode, 'chat')

  // ── approval: shown, answered once, the tool goes on ──
  await ok('chat.prompt', { channelId: ch, nodeId: nd, text: 'please ask' })
  const asked = await waitEntries(ch, nd, (e) => e.some((x) => x.kind === 'approval' && x.status === 'pending'), 'approval card')
  const card = asked.find((x) => x.kind === 'approval')
  assert.equal(card.summary, 'rm x')
  assert.equal((await ok('channels.list')).find((x: any) => x.id === ch).pendingApprovals, 1)
  await ok('chat.answer', { channelId: ch, nodeId: nd, approvalId: card.id, allow: true })
  assert.equal((await c('chat.answer', { channelId: ch, nodeId: nd, approvalId: card.id, allow: false })).code, 'stale-request', 'answered once')
  const after = await waitEntries(ch, nd, (e) => e.some((x) => x.kind === 'text' && x.text === 'tool allow'), 'the tool ran after the answer')
  assert.equal(after.find((x) => x.kind === 'approval').status, 'allowed')
  assert.equal(after.find((x) => x.kind === 'tool').output, 'removed')

  // ── restart: the node resumes its own session ──
  const session = (await ok('channel.get', { channelId: ch })).channel.nodes[0].session
  assert.equal(session, 'fake-session-1')
  b.stop()
  b = new Backend({ WB_DATA_DIR: data })
  await b.ipc('test:openWindow', repo)
  const v = await ok('channel.get', { channelId: ch })
  assert.notEqual(v.runtime[nd].status, 'running', 'no process survives a restart')
  assert.ok((await entries(ch, nd)).some((x) => x.text === 'echo:hello'), 'the transcript survives a restart')
  await ok('node.restart', { channelId: ch, nodeId: nd })
  await ok('chat.prompt', { channelId: ch, nodeId: nd, text: 'again' })
  await waitEntries(ch, nd, (e) => e.some((x) => x.kind === 'text' && x.text === 'resumed:fake-session-1 echo:again'), 'resumed session')

  console.log('agui: protocol check, chat transcript, live events, one-shot approval, resume after restart ok')
} finally {
  b.stop()
}
