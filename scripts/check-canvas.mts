// node --experimental-strip-types scripts/check-canvas.mts
// Cowork 2.0 canvas through the real backend (wb-ipc) on a temporary repo: node lifecycle, worktree data protection,
// screen and hook approvals (one-shot, generation-bound), handoff drafts, safe paste, stop, persistence and damaged files.
// Needs: cargo build -p wb-ipc -p wb-hook (wb-hook.exe must sit next to wb-ipc.exe). No real AI CLI is started.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, exe, until } from './lib/core-call.mts'

const hookExe = path.join(path.dirname(exe), 'wb-hook.exe')
assert.ok(fs.existsSync(hookExe), `build wb-hook first: ${hookExe}`)

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'wb-canvas-')))
const repo = path.join(tmp, 'repo')
const data = path.join(tmp, 'data')
const git = (...a: string[]): string => execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim()
fs.mkdirSync(path.join(repo, 'agents'), { recursive: true })
git('init', '-q', '-b', 'main')
git('config', 'user.name', 'check')
git('config', 'user.email', 'check@example.invalid')
fs.writeFileSync(path.join(repo, 'README.md'), 'canvas\n')
fs.writeFileSync(path.join(repo, '.gitignore'), 'ignored.log\n.workbench/\n')
fs.writeFileSync(path.join(repo, 'agents', 'shell.yaml'), 'launcher:\n  id: shell\n  name: Shell\n  cli: cmd\n')
git('add', '-A')
git('commit', '-qm', 'init')

// The fake "claude": a .cmd shim like npm's, running the fixture
const fakeCmd = path.join(tmp, 'fake-claude.cmd')
fs.writeFileSync(fakeCmd, `@echo off\r\nnode "${path.resolve('scripts/fixtures/canvas-fake-cli.mjs')}" %*\r\n`)

let b = new Backend({ WB_DATA_DIR: data })
type R = { ok: boolean; data?: any; code?: string; message?: string }
const c = (op: string, req?: unknown): Promise<R> => b.ipc(`canvas:${op}`, req ?? null)
const ok = async (op: string, req?: unknown): Promise<any> => {
  const r = await c(op, req)
  assert.equal(r.ok, true, `${op} failed: ${r.code} ${r.message}`)
  return r.data
}
const code = async (op: string, req?: unknown): Promise<string | undefined> => (await c(op, req)).code
const screen = (pty: string): Promise<string> => b.ipc('test:ptyScreen', pty)
const size = { width: 640, height: 400 }
const pos = { x: 0, y: 0 }

let ch = ''
const view = (): Promise<any> => ok('channel.get', { channelId: ch })
const rt = async (node: string): Promise<any> => (await view()).runtime[node]
const waitRt = async (node: string, cond: (r: any) => boolean, what: string, ms = 30000): Promise<any> => {
  let last: any
  await until(async () => cond((last = await rt(node))), `${what} (last: ${JSON.stringify(last)})`, ms)
  return last
}
const running = (node: string): Promise<any> => waitRt(node, (r) => r?.status === 'running' && !!r.ptyId, 'node running')
const waitScreen = async (pty: string, re: RegExp, what: string, ms = 30000): Promise<string> => {
  let s = ''
  await until(async () => re.test((s = await screen(pty))), `${what} (screen: ${JSON.stringify(s.slice(-400))})`, ms)
  return s
}
const type = (pty: string, text: string): Promise<unknown> => b.ipc('pty:write', pty, text)

try {
  await b.ipc('test:openWindow', repo)

  // ── channel ──
  assert.deepEqual(await ok('channels.list'), [])
  ch = (await ok('channel.create', { name: 'general' })).channel.id
  assert.equal((await ok('channels.list')).length, 1)

  // ── worktree node: create, work, protected while running ──
  const n1 = (await ok('node.create', { channelId: ch, agent: 'shell', workspace: 'worktree', position: pos, size })).id
  let r1 = await running(n1)
  const node1 = (await view()).channel.nodes.find((n: any) => n.id === n1)
  assert.equal(node1.worktree.branch, `cowork/${ch}/${n1}`)
  assert.ok(fs.existsSync(node1.worktree.path))
  await waitScreen(r1.ptyId, />\s*$/, 'cmd prompt')
  await type(r1.ptyId, 'echo hi> a.txt\r')
  await until(() => fs.existsSync(path.join(node1.worktree.path, 'a.txt')), 'a.txt written in the worktree')
  assert.ok(!fs.existsSync(path.join(repo, 'a.txt')), 'the repo root is untouched')
  let ch1 = await ok('node.changes', { channelId: ch, nodeId: n1 })
  assert.deepEqual(ch1.untracked.map((f: any) => f.path), ['a.txt'])
  assert.equal(await code('node.commitAll', { channelId: ch, nodeId: n1, files: ['a.txt'], message: 'x' }), 'busy', 'no commit while the process runs')

  // ── paste: generation-bound, never with Enter ──
  await waitRt(n1, (r) => r.inputReady === true, 'input ready')
  assert.equal(await code('node.message', { channelId: ch, nodeId: n1, sessionGeneration: 'old', text: 'echo nope' }), 'stale-request')
  assert.equal(await code('node.message', { channelId: ch, nodeId: n1, sessionGeneration: r1.sessionGeneration, text: 'a\nb' }), 'not-ready', 'cmd has no bracketed paste: no multi-line paste')
  await ok('node.message', { channelId: ch, nodeId: n1, sessionGeneration: r1.sessionGeneration, text: 'echo pasted-line' })
  // Not run: the input line still ends with the text and no output line follows
  await waitScreen(r1.ptyId, /echo pasted-line\s*$/, 'pasted text sits on the input line')
  await type(r1.ptyId, '\x1b')

  // ── close keeps the worktree; then commit, merge with stale protection, cleanup ──
  await ok('node.close', { channelId: ch, nodeId: n1 })
  await waitRt(n1, (r) => r.status === 'pending-cleanup', 'ended with work kept')
  assert.ok(fs.existsSync(node1.worktree.path))
  assert.equal(await code('node.commitAll', { channelId: ch, nodeId: n1, files: ['a.txt', 'README.md'], message: 'x' }), 'stale-preview', 'the list must match the changes exactly')
  await ok('node.commitAll', { channelId: ch, nodeId: n1, files: ['a.txt'], message: 'add a' })
  ch1 = await ok('node.changes', { channelId: ch, nodeId: n1 })
  assert.equal(ch1.clean, true)
  let preview = await ok('node.mergePreview', { channelId: ch, nodeId: n1, target: 'main' })
  assert.equal(preview.target.branch, 'main')
  fs.writeFileSync(path.join(repo, 'README.md'), 'moved\n')
  git('commit', '-qam', 'target moved')
  assert.equal(await code('node.merge', { channelId: ch, nodeId: n1, token: preview.token }), 'stale-preview', 'target HEAD moved after the preview')
  assert.equal(await code('node.merge', { channelId: ch, nodeId: n1, token: preview.token }), 'stale-preview', 'a token is single-use')
  preview = await ok('node.mergePreview', { channelId: ch, nodeId: n1, target: 'main' })
  await ok('node.merge', { channelId: ch, nodeId: n1, token: preview.token })
  assert.equal(git('show', 'main:a.txt').trim(), 'hi')
  await ok('node.cleanup', { channelId: ch, nodeId: n1 })
  assert.ok(!fs.existsSync(node1.worktree.path), 'merged clean worktree removed')
  assert.ok(!(await view()).channel.nodes.some((n: any) => n.id === n1))

  // ── root node: one per repo, never deleted ──
  const root = (await ok('node.create', { channelId: ch, agent: 'shell', workspace: 'root', position: pos, size })).id
  await running(root)
  const ch2 = (await ok('channel.create', { name: 'other' })).channel.id
  assert.equal(await code('node.create', { channelId: ch2, agent: 'shell', workspace: 'root', position: pos, size }), 'root-node-exists')
  assert.equal(await code('node.discardPreview', { channelId: ch, nodeId: root }), 'busy')
  await ok('node.close', { channelId: ch, nodeId: root })
  await waitRt(root, (r) => r.status === 'exited', 'root node ended')
  assert.equal(await code('node.discardPreview', { channelId: ch, nodeId: root }), 'invalid', 'a root node has no discard path')
  await ok('node.cleanup', { channelId: ch, nodeId: root })
  assert.ok(fs.existsSync(path.join(repo, 'README.md')), 'cleanup of a root node deletes nothing')
  await ok('channel.delete', { channelId: ch2 })

  // ── discard lists untracked and ignored data, and re-checks before deleting ──
  const n2 = (await ok('node.create', { channelId: ch, agent: 'shell', workspace: 'worktree', position: pos, size })).id
  const r2 = await running(n2)
  const wt2 = (await view()).channel.nodes.find((n: any) => n.id === n2).worktree.path
  await waitScreen(r2.ptyId, />\s*$/, 'cmd prompt')
  await type(r2.ptyId, 'echo x> u.txt & echo y> ignored.log\r')
  await until(() => fs.existsSync(path.join(wt2, 'u.txt')) && fs.existsSync(path.join(wt2, 'ignored.log')), 'files written')
  assert.equal(await code('node.cleanup', { channelId: ch, nodeId: n2 }), 'busy')
  await ok('node.close', { channelId: ch, nodeId: n2 })
  await waitRt(n2, (r) => r.status === 'pending-cleanup', 'ended')
  assert.notEqual(await code('node.cleanup', { channelId: ch, nodeId: n2 }), undefined, 'cleanup refuses unmerged data')
  let dp = await ok('node.discardPreview', { channelId: ch, nodeId: n2 })
  assert.ok(dp.changes.untracked.some((f: any) => f.path === 'u.txt'))
  assert.ok(dp.ignored.includes('ignored.log'), 'ignored data is listed too')
  fs.appendFileSync(path.join(wt2, 'u.txt'), 'more')
  assert.equal(await code('node.discard', { channelId: ch, nodeId: n2, token: dp.token }), 'stale-preview', 'changed after the preview')
  assert.ok(fs.existsSync(wt2))
  dp = await ok('node.discardPreview', { channelId: ch, nodeId: n2 })
  await ok('node.discard', { channelId: ch, nodeId: n2, token: dp.token })
  assert.ok(!fs.existsSync(wt2))

  // ── handoff: a draft from git, never written to the target ──
  const a = (await ok('node.create', { channelId: ch, agent: 'shell', workspace: 'worktree', position: pos, size })).id
  const bNode = (await ok('node.create', { channelId: ch, agent: 'shell', workspace: 'worktree', position: { x: 700, y: 0 }, size })).id
  await running(a)
  const rb = await running(bNode)
  const edge = await ok('handoff.create', { channelId: ch, from: a, to: bNode, note: 'note-xyz\x1b[31m' })
  assert.match(edge.text, new RegExp(`cowork/${ch}/${a}`))
  assert.match(edge.text, /note-xyz\[31m/, 'control characters are stripped from the draft')
  assert.ok(!(await screen(rb.ptyId)).includes('note-xyz'), 'nothing reaches the target terminal')
  assert.ok((await view()).channel.events.some((e: any) => e.t === 'handoff' && e.edge === edge.id))

  // ── stop: one Esc each, new work refused, stage 2 ends processes, clear ──
  const ra = await rt(a)
  assert.equal((await ok('stop')).requested, true)
  assert.equal(await code('node.create', { channelId: ch, agent: 'shell', workspace: 'worktree', position: pos, size }), 'stopped')
  assert.equal(await code('node.message', { channelId: ch, nodeId: a, sessionGeneration: ra.sessionGeneration, text: 'x' }), 'stopped')
  assert.deepEqual((await ok('stop.killAll')).failed, [])
  await waitRt(a, (r) => r.status === 'pending-cleanup', 'killed node ended')
  await waitRt(bNode, (r) => r.status === 'pending-cleanup', 'killed node ended')
  assert.equal((await ok('stop.clear')).requested, false)

  // ── screen approval and hook approval through the fake Claude ──
  await b.ipc('settings:set', { cliPaths: { claude: fakeCmd } })
  const cl = (await ok('node.create', { channelId: ch, agent: 'claude', workspace: 'worktree', position: pos, size })).id
  let rc = await running(cl)
  await waitScreen(rc.ptyId, /FAKE-CLI ready hook=yes pipe=yes/, 'hook mounted for this session')
  await type(rc.ptyId, 'ask\r')
  let card = (await waitRt(cl, (r) => !!r.approval, 'screen approval card')).approval
  assert.equal(card.source, 'screen')
  assert.equal(card.canApprove, true)
  assert.equal(await code('node.approve', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: 'other' }), 'stale-request')
  await ok('node.approve', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: card.sessionGeneration })
  await waitScreen(rc.ptyId, /ANSWER:yes/, 'Enter reached the menu')
  assert.equal(await code('node.approve', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: card.sessionGeneration }), 'stale-request', 'one-shot')
  await waitRt(cl, (r) => !r.approval, 'card gone')
  await type(rc.ptyId, 'ask\r')
  card = (await waitRt(cl, (r) => !!r.approval, 'second card')).approval
  await ok('node.reject', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: card.sessionGeneration })
  await waitScreen(rc.ptyId, /ANSWER:no/, 'Esc reached the menu')

  await waitRt(cl, (r) => !r.approval && r.inputReady, 'idle again')
  await type(rc.ptyId, 'hook new.txt\r')
  card = (await waitRt(cl, (r) => r.approval?.source === 'hook', 'hook card')).approval
  assert.equal(card.hook.files[0].path, 'new.txt')
  assert.equal(card.hook.files[0].added, 2)
  await ok('node.approve', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: card.sessionGeneration })
  await waitScreen(rc.ptyId, /HOOK:.*"behavior":"allow"/, 'allow reached the CLI')
  await waitRt(cl, (r) => !r.approval && r.inputReady, 'idle again')
  await type(rc.ptyId, 'hook second.txt\r')
  card = (await waitRt(cl, (r) => r.approval?.hook?.files?.[0]?.path === 'second.txt', 'second hook card')).approval
  await ok('node.reject', { channelId: ch, nodeId: cl, requestId: card.requestId, sessionGeneration: card.sessionGeneration, reason: 'not this one' })
  await waitScreen(rc.ptyId, /HOOK:.*"behavior":"deny","message":"not this one"/, 'deny with the reason')
  await waitRt(cl, (r) => !r.approval && r.inputReady, 'idle again')
  await type(rc.ptyId, 'hook third.txt\r')
  await waitRt(cl, (r) => r.approval?.hook?.files?.[0]?.path === 'third.txt', 'third hook card')
  await ok('stop')
  await waitScreen(rc.ptyId, /HOOK:PASS/, 'stop hands the hook back to the CLI, never allows')
  assert.equal((await rt(cl)).approval, null)
  await ok('stop.clear')

  // ── bracketed paste: multi-line only when the CLI asked for it, and not executed ──
  await type(rc.ptyId, 'bracketed\r')
  await until(async () => (await rt(cl)).inputReady === true, 'ready')
  rc = await rt(cl)
  await ok('node.message', { channelId: ch, nodeId: cl, sessionGeneration: rc.sessionGeneration, text: 'line1\nline2' })
  const s = await waitScreen(rc.ptyId, /line2/, 'pasted')
  assert.ok(!s.includes('echo:line1'), 'a pasted newline is not Enter')

  // ── persistence: positions and edges come back; no process is treated as alive ──
  await ok('node.layout', { channelId: ch, nodeId: a, position: { x: 123, y: 45 }, size: { width: 700, height: 420 } })
  b.stop()
  b = new Backend({ WB_DATA_DIR: data })
  await b.ipc('test:openWindow', repo)
  const v = await view()
  const na = v.channel.nodes.find((n: any) => n.id === a)
  assert.deepEqual([na.position, na.size], [{ x: 123, y: 45 }, { width: 700, height: 420 }])
  assert.ok(v.channel.edges.some((e: any) => e.id === edge.id))
  assert.ok(Object.values(v.runtime).every((r: any) => r.status !== 'running' && !r.ptyId), 'restart shows nodes as ended')
  assert.ok(Object.values(v.runtime).every((r: any) => !r.approval), 'pending requests do not survive a restart')

  // ── damaged or newer files are never shown or overwritten ──
  const dir = path.join(repo, '.git', 'cowork', 'channels')
  fs.writeFileSync(path.join(dir, 'broken.json'), '{ not json')
  fs.writeFileSync(path.join(dir, 'future.json'), JSON.stringify({ schemaVersion: 2, id: 'future', name: 'f', nodes: [], edges: [], events: [] }))
  b.stop()
  b = new Backend({ WB_DATA_DIR: data })
  await b.ipc('test:openWindow', repo)
  const ids = (await ok('channels.list')).map((x: any) => x.id)
  assert.ok(!ids.includes('broken') && !ids.includes('future'))
  assert.equal(fs.readFileSync(path.join(dir, 'broken.json'), 'utf8'), '{ not json')
  assert.match(fs.readFileSync(path.join(dir, 'future.json'), 'utf8'), /"schemaVersion":2/)
  assert.equal(await code('channel.get', { channelId: '../x' }), 'not-found')

  console.log('canvas: lifecycle, worktree protection, approvals (screen and hook), handoff, paste, stop, persistence ok')
} finally {
  b.stop()
  await new Promise((r) => setTimeout(r, 500))
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 })
}
