// node --experimental-strip-types scripts/check-agui.mts
// Chat nodes (cowork-agui-implement.md §9.1) through the real backend (wb-ipc) with a stand-in CLI, no AI:
// the Codex protocol check refuses a fork that lacks a method, a prompt streams back into the transcript, an approval
// is answered once, live events reach the window, and a restart resumes the CLI's own session. A group of chat nodes
// uses the team tools through the real `wb-hook mcp` and pipe.
// Needs: cargo build -p wb-ipc -p wb-hook (wb-hook.exe must sit next to wb-ipc.exe).
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

  // ── a group of chat nodes (§6.2-§6.8, §12.1): brief, @, relay approval, report with memory, board, ask_person, free talk ──
  const sq = (await ok('channel.create', { name: 'squad' })).channel.id
  const bot = async (role: string): Promise<string> => (await ok('node.create', { channelId: sq, agent: 'custom-agent', workspace: 'worktree', mode: 'chat', role })).id
  const L = await bot('planning')
  const M = await bot('tests')
  const sqv = (): Promise<any> => ok('channel.get', { channelId: sq })
  const says = (node: string, re: RegExp, what: string): Promise<any[]> => waitEntries(sq, node, (e) => e.some((x) => x.kind === 'text' && re.test(x.text)), what)
  const approveRelay = async (what: string): Promise<void> => {
    await until(async () => (await sqv()).relays.length === 1, what, 30000)
    await ok('relay.approve', { channelId: sq, messageId: (await sqv()).relays[0].id })
  }
  await ok('chat.send', { channelId: sq, text: 'hi' })
  let le = await says(L, /^echo:/, 'a message without an @ goes to the lead')
  const first = le.find((x) => x.kind === 'text').text
  assert.match(first, /^echo:\[Agent Workbench, group "squad"\] You lead this group\. Your role: planning\..*post_task.*You alone write \.project-memory/)
  assert.ok(first.endsWith('\n\nhi'), 'the brief comes before the first message')
  assert.equal(le.find((x) => x.kind === 'user').text, 'hi', 'the transcript keeps the user words, not the brief')
  await ok('chat.send', { channelId: sq, to: M, text: 'hello member' })
  assert.match((await says(M, /^echo:/, 'an @ reaches only that bot')).find((x) => x.kind === 'text').text, /You are a member; the lead is .*read-only/)

  const report = JSON.stringify({ status: 'done', summary: 'did t1', memory: 'tests need node 22' })
  await ok('chat.send', { channelId: sq, text: `DO: assign_task ${JSON.stringify({ member: M, title: 't1', details: `DO: report ${report}` })}` })
  await says(L, /^assign_task ok .*waiting for the user/, 'approve mode holds the lead task')
  await approveRelay('the task card')
  await says(M, /^report ok .*sent to your lead/, 'the member does the task and reports to its lead')
  await approveRelay('the report waits for the user too')
  le = await waitEntries(sq, L, (e) => e.some((x) => x.kind === 'user' && x.from === M && /did t1[\s\S]*Memory suggestion[\s\S]*tests need node 22/.test(x.text)), 'the lead gets the report with the memory suggestion')
  await says(L, /^echo:\[Agent Workbench\] Message from .*kind report\.[\s\S]*<<<BEGIN MESSAGE>>>/, 'another agent words arrive inside the envelope')
  assert.equal((await sqv()).channel.tasks[M].memory, 'tests need node 22')

  await ok('chat.send', { channelId: sq, text: `DO: post_task ${JSON.stringify({ title: 'CLAIM write tests', done_when: 'npm test passes' })}` })
  await says(L, /^post_task ok .*on the board for 1 member/, 'the lead posts to the board')
  await approveRelay('the board notice is the lead message')
  await says(M, /^claim_task ok The task is yours/, 'the member claims it')
  const bt = (await sqv()).channel.board[0]
  assert.deepEqual([bt.status, bt.claimedBy, (await sqv()).channel.tasks[M].title], ['claimed', M, 'CLAIM write tests'])
  await waitEntries(sq, L, (e) => e.some((x) => x.kind === 'user' && x.from === 'app' && /claimed board task/.test(x.text)), 'the lead hears who claimed it')
  await ok('chat.send', { channelId: sq, to: M, text: `DO: claim_task ${JSON.stringify({ task_id: bt.id })}` })
  await says(M, /^claim_task err Already claimed by/, 'one claim per task')

  await ok('chat.send', { channelId: sq, to: M, text: `DO: ask_person ${JSON.stringify({ question: 'Which DB?', choices: ['pg', 'sqlite'] })}` })
  await says(M, /^ask_person ok/, 'the question goes to the user')
  await until(async () => (await sqv()).asks.length === 1, 'an ask card', 30000)
  const ask = (await sqv()).asks[0]
  assert.deepEqual([ask.node, ask.question, ask.choices], [M, 'Which DB?', ['pg', 'sqlite']])
  await ok('ask.answer', { channelId: sq, askId: ask.id, text: 'pg' })
  assert.equal((await c('ask.answer', { channelId: sq, askId: ask.id, text: 'pg' })).code, 'stale-request', 'answered once')
  await says(M, /^echo:Answer to your question "Which DB\?":\npg$/, 'the answer is the bot next message')

  await ok('team.update', { channelId: sq, freeTalk: true })
  await ok('chat.send', { channelId: sq, text: 'tests: who runs the suite?' })
  const shared = (await says(M, /^echo:tests: who runs the suite\?/, 'free talk: the bot whose role it names answers')).filter((x) => x.kind === 'user').at(-1).id
  await waitEntries(sq, L, (e) => e.some((x) => x.kind === 'user' && x.id === shared), 'the lead got the same message under the same id')
  await until(async () => !(await sqv()).runtime[L].inRun, 'the lead finished its turn', 30000)
  assert.ok(!(await entries(sq, L)).some((x) => x.kind === 'text' && x.text.trim() === 'PASS'), 'a PASS leaves nothing in the chat')

  console.log('agui: protocol check, chat transcript, live events, one-shot approval, resume after restart, chat group (brief, @, relay, report memory, board claim, ask_person, free talk PASS) ok')
} finally {
  b.stop()
}
