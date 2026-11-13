// node --experimental-strip-types scripts/check-team.mts
// Cowork teams through the real backend (wb-ipc), the real `wb-hook mcp` over the real pipe, and a fake agent that
// talks MCP like Claude does (cowork-team-implement.md). Covers: the notice reaches only an idle CLI and carries no
// message text, read_inbox, relay modes (approve, lead-auto, auto), routing, roles, caps, merge into the lead's
// worktree (asked and automatic), stop and persistence.
// Needs: cargo build -p wb-ipc -p wb-hook (wb-hook.exe must sit next to wb-ipc.exe).
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, exe, until } from './lib/core-call.mts'

assert.ok(fs.existsSync(path.join(path.dirname(exe), 'wb-hook.exe')), 'build wb-hook first')

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'wb-team-')))
const repo = path.join(tmp, 'repo')
const data = path.join(tmp, 'data')
const git = (cwd: string, ...a: string[]): string => execFileSync('git', a, { cwd, encoding: 'utf8' }).trim()
fs.mkdirSync(repo, { recursive: true })
git(repo, 'init', '-q', '-b', 'main')
git(repo, 'config', 'user.name', 'check')
git(repo, 'config', 'user.email', 'check@example.invalid')
fs.writeFileSync(path.join(repo, 'README.md'), 'team\n')
fs.writeFileSync(path.join(repo, '.gitignore'), '.workbench/\n')
git(repo, 'add', '-A')
git(repo, 'commit', '-qm', 'init')
const fakeCmd = path.join(tmp, 'fake-claude.cmd')
fs.writeFileSync(fakeCmd, `@echo off\r\nnode "${path.resolve('scripts/fixtures/team-fake-cli.mjs')}" %*\r\n`)

let b = new Backend({ WB_DATA_DIR: data })
type R = { ok: boolean; data?: any; code?: string; message?: string }
const c = (op: string, req?: unknown): Promise<R> => b.ipc(`canvas:${op}`, req ?? null)
const ok = async (op: string, req?: unknown): Promise<any> => {
  const r = await c(op, req)
  assert.equal(r.ok, true, `${op} failed: ${r.code} ${r.message}`)
  return r.data
}
const screen = (pty: string): Promise<string> => b.ipc('test:ptyScreen', pty)
const size = { width: 640, height: 400 }
let ch = ''
const view = (): Promise<any> => ok('channel.get', { channelId: ch })
const rt = async (n: string): Promise<any> => (await view()).runtime[n]
const type = async (n: string, text: string): Promise<void> => {
  await until(async () => (await rt(n))?.inputReady === true, `${n} ready to type`)
  await b.ipc('pty:write', (await rt(n)).ptyId, text + '\r')
}
// What the fake agent of a node recorded (scripts/fixtures/team-fake-cli.mjs): "<tool> ok|err <text>" or the inbox text
const cwdOf = new Map<string, string>()
const lines = async (n: string, kind: 'RESULT' | 'INBOX'): Promise<string[]> => {
  const cwd = cwdOf.get(n)!
  const file = path.join(os.tmpdir(), 'wb-team-fake', Buffer.from(cwd.toLowerCase()).toString('hex').slice(-120) + '.jsonl')
  if (!fs.existsSync(file)) return []
  return fs.readFileSync(file, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)).filter((r: any) => r.kind === kind).map((r: any) => (kind === 'RESULT' ? `${r.tool} ${r.ok ? 'ok' : 'err'} ${r.text}` : r.text))
}
const waitLine = async (n: string, kind: 'RESULT' | 'INBOX', re: RegExp, what: string, ms = 30000): Promise<string> => {
  let last: string[] = []
  await until(async () => (last = await lines(n, kind)).some((l) => re.test(l)), `${what} (seen: ${JSON.stringify(last.slice(-3))})`, ms)
  return last.find((l) => re.test(l))!
}
const callTool = async (n: string, tool: string, args: object, re: RegExp, what: string): Promise<string> => {
  const before = (await lines(n, 'RESULT')).length
  await type(n, `call ${tool} ${JSON.stringify(args)}`)
  let got: string[] = []
  try {
    await until(async () => (got = (await lines(n, 'RESULT')).slice(before)).length > 0, `${what}: a result`, 30000)
  } catch (e) {
    console.log(`--- ${n} screen\n${await screen((await rt(n)).ptyId)}\n--- runtime ${JSON.stringify(await rt(n))}`)
    throw e
  }
  assert.match(got[0], re, what)
  return got[0]
}

try {
  await b.ipc('test:openWindow', repo)
  await b.ipc('settings:set', { cliPaths: { claude: fakeCmd } })
  ch = (await ok('channel.create', { name: 'team' })).channel.id
  const node = async (): Promise<string> => (await ok('node.create', { channelId: ch, agent: 'claude', workspace: 'worktree', position: { x: 0, y: 0 }, size })).id
  const [L, A, B] = [await node(), await node(), await node()]
  for (const n of [L, A, B]) {
    await until(async () => (await rt(n))?.status === 'running', `${n} running`)
    await until(async () => /FAKE-TEAM ready tools=team_info,read_inbox,send_message,assign_task,report,get_member_changes,request_merge/.test(await screen((await rt(n)).ptyId)), `${n} got all seven tools from wb-hook mcp`)
  }
  const wt = (n: string) => view().then((v) => v.channel.nodes.find((x: any) => x.id === n).worktree)
  for (const n of [L, A, B]) cwdOf.set(n, (await wt(n)).path)

  // ── outside a team: only read_inbox and report ──
  await callTool(A, 'assign_task', { member: B, title: 'x' }, /^assign_task err .*not available/, 'no lead tools outside a team')
  await callTool(A, 'report', { status: 'working', summary: 'warming up' }, /^report ok/, 'report works without a team')
  assert.equal((await rt(A)).task.summary, 'warming up', 'the Feed card learns the task')

  // ── team, start: the lead gets the kickoff through read_inbox ──
  const team = await ok('team.create', { channelId: ch, name: 'T', goal: 'make it work', leadId: L, memberIds: [L, A, B], relay: 'approve', bounds: { x: 0, y: 0, width: 900, height: 600 } })
  assert.equal((await c('team.create', { channelId: ch, name: 'T2', goal: 'g', leadId: A, memberIds: [A], relay: 'auto', bounds: { x: 0, y: 0, width: 9, height: 9 } })).code, 'invalid', 'a node is in one team only')
  assert.deepEqual([(await rt(L)).role, (await rt(A)).role], ['lead', 'member'])
  await ok('team.start', { channelId: ch, teamId: team.id })
  const kickoff = await waitLine(L, 'INBOX', /You are the lead/, 'the kickoff arrives')
  assert.match(kickoff, /make it work/)
  assert.match(kickoff, new RegExp(`- ${A} \\(claude\\)`))
  const notice = await screen((await rt(L)).ptyId)
  assert.ok(notice.includes('[Agent Workbench] You have 1 new message from Agent Workbench'), 'a fixed notice line')
  assert.ok(!/make it work.*\[Agent Workbench\]|\[Agent Workbench\][^\n]*make it work/.test(notice.replace(/INBOX \S+/g, '')), 'the notice carries no message text')

  // ── approve mode: the lead's task waits for the user, then reaches the member, who reports back ──
  const done = JSON.stringify({ status: 'done', summary: 'did t1' })
  await callTool(L, 'assign_task', { member: A, title: 't1', details: `DO: report ${done}` }, /^assign_task ok .*waiting for the user/, 'held for approval')
  let v = await view()
  assert.equal(v.relays.length, 1, 'one relay card')
  assert.equal((await rt(A)).unread, 0, 'not delivered before approval')
  await ok('relay.approve', { channelId: ch, messageId: v.relays[0].id })
  assert.equal((await c('relay.approve', { channelId: ch, messageId: v.relays[0].id })).code, 'stale-request', 'one approval per message')
  await waitLine(A, 'INBOX', /Task from your team lead: t1/, 'the member reads its task')
  await waitLine(A, 'RESULT', /^report ok .*sent to your lead/, 'the member ran the task and reported')
  assert.equal((await rt(A)).task.status, 'done')
  await until(async () => (await view()).relays.length === 1, 'the report is held too (approve mode)')
  await ok('relay.approve', { channelId: ch, messageId: (await view()).relays[0].id })
  await waitLine(L, 'INBOX', /\[done\] did t1/, 'the lead gets the report')

  // ── reject: the sender hears about it ──
  await callTool(L, 'send_message', { to: B, text: 'hello B' }, /^send_message ok/, 'held')
  await ok('relay.reject', { channelId: ch, messageId: (await view()).relays[0].id })
  await waitLine(L, 'INBOX', /The user rejected your message/, 'the sender learns of the rejection')
  assert.ok(!(await lines(B, 'INBOX')).some((l) => l.includes('hello B')), 'a rejected message never arrives')

  // ── routing and roles ──
  await callTool(A, 'send_message', { to: B, text: 'psst' }, /^send_message err .*cannot message/, 'members need a line the user drew')
  await callTool(A, 'request_merge', { member: B }, /^request_merge err .*not available/, 'lead tools stay with the lead')

  // ── lead-auto: the lead's messages flow, members' are still held ──
  await ok('team.update', { channelId: ch, teamId: team.id, relay: 'lead-auto' })
  await callTool(L, 'send_message', { to: B, text: 'hello again' }, /^send_message ok .*queued/, 'the lead is not held')
  await waitLine(B, 'INBOX', /hello again/, 'delivered without a card')
  await callTool(B, 'send_message', { to: L, text: 'hi lead' }, /^send_message ok .*waiting/, 'a member still is')
  await ok('relay.approve', { channelId: ch, messageId: (await view()).relays[0].id })
  await waitLine(L, 'INBOX', /hi lead/, 'approved member message arrives')

  // ── merge into the lead's worktree: asked in lead-auto, done by the lead in auto ──
  const wa = await wt(A)
  const wl = await wt(L)
  fs.writeFileSync(path.join(wa.path, 'a.txt'), 'from A\n')
  git(wa.path, 'add', 'a.txt')
  git(wa.path, '-c', 'user.name=a', '-c', 'user.email=a@example.invalid', 'commit', '-qm', 'A work')
  await callTool(L, 'get_member_changes', { member: A }, /^get_member_changes ok [\s\S]*A work[\s\S]*a\.txt/, 'the lead sees the member commit, from git')
  await callTool(L, 'request_merge', { member: A }, /^request_merge ok .*waiting for the user/, 'asked first outside auto')
  v = await view()
  assert.equal(v.mergeRequests.length, 1)
  assert.ok(!fs.existsSync(path.join(wl.path, 'a.txt')), 'nothing merged before approval')
  await ok('teammerge.approve', { channelId: ch, requestId: v.mergeRequests[0].id })
  assert.equal(fs.readFileSync(path.join(wl.path, 'a.txt'), 'utf8').trim(), 'from A', 'merged into the lead worktree')
  assert.ok(!fs.existsSync(path.join(repo, 'a.txt')), 'never into the user branch')
  await waitLine(L, 'INBOX', /Merged .* into your worktree/, 'the lead learns the result')
  await ok('team.update', { channelId: ch, teamId: team.id, relay: 'auto' })
  fs.writeFileSync(path.join(wa.path, 'b.txt'), 'more\n')
  git(wa.path, 'add', 'b.txt')
  git(wa.path, '-c', 'user.name=a', '-c', 'user.email=a@example.invalid', 'commit', '-qm', 'A more')
  await callTool(L, 'request_merge', { member: A }, /^request_merge ok .*Merged/, 'in auto the lead decides')
  assert.ok(fs.existsSync(path.join(wl.path, 'b.txt')))
  fs.writeFileSync(path.join(wl.path, 'dirty.txt'), 'x')
  await callTool(L, 'request_merge', { member: A }, /^request_merge err .*uncommitted/, 'never over the lead uncommitted work')
  fs.rmSync(path.join(wl.path, 'dirty.txt'))

  // ── caps (auto): a burst pauses the team, nothing is sent ──
  // The lead already sent some messages in the last 10 minutes: allow exactly two more
  const sent = (await ok('team.messages', { channelId: ch, teamId: team.id })).filter((m: any) => m.from === L).length
  await ok('team.update', { channelId: ch, teamId: team.id, limits: { maxMessages: 200, perMemberPer10Min: sent + 2 } })
  for (let i = 0; i < 2; i++) await callTool(L, 'send_message', { to: B, text: `burst ${i}` }, /^send_message ok/, 'within the cap')
  await callTool(L, 'send_message', { to: B, text: 'too many' }, /^send_message err .*paused/, 'the cap pauses the team')
  v = await view()
  assert.equal(v.teams[0].status, 'paused')
  assert.match(v.teams[0].pausedReason, /10 minutes/)
  await callTool(L, 'send_message', { to: B, text: 'still?' }, /^send_message err .*not running/, 'nothing goes out while paused')
  await ok('team.update', { channelId: ch, teamId: team.id, limits: { maxMessages: 200, perMemberPer10Min: 100 } })
  await ok('team.resume', { channelId: ch, teamId: team.id })

  // ── stop pauses the team; clearing the stop does not resume it ──
  await ok('stop')
  assert.equal((await view()).teams[0].status, 'paused')
  await ok('stop.clear')
  assert.equal((await view()).teams[0].status, 'paused', 'the user resumes each team')
  assert.equal((await c('team.update', { channelId: ch, teamId: team.id, memberIds: [L, A], leadId: L })).ok, true, 'members can change while paused')
  await ok('team.resume', { channelId: ch, teamId: team.id })
  assert.equal((await c('team.update', { channelId: ch, teamId: team.id, goal: 'other' })).code, 'busy', 'not while running')

  // ── the lead finishes: the team is done ──
  await callTool(L, 'report', { status: 'done', summary: 'all done' }, /^report ok .*team is marked done/, 'lead done')
  assert.equal((await view()).teams[0].status, 'done')

  // ── persistence: team, tasks and messages come back ──
  b.stop()
  b = new Backend({ WB_DATA_DIR: data })
  await b.ipc('test:openWindow', repo)
  v = await view()
  assert.equal(v.teams[0].id, team.id)
  assert.equal(v.channel.tasks[A].status, 'done')
  const msgs = await ok('team.messages', { channelId: ch, teamId: team.id })
  assert.ok(msgs.some((m: any) => m.text.includes('did t1')) && msgs.every((m: any) => m.state !== 'pending-approval' || m.from !== 'app'))
  assert.ok(v.channel.events.some((e: any) => e.t === 'message' && !('text' in e)), 'events keep lengths, not text')

  console.log('team: tools over wb-hook mcp, notice then read_inbox, relay modes, routing, roles, caps, merge into the lead, stop, persistence ok')
} finally {
  b.stop()
}
