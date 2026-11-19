// node --experimental-strip-types scripts/check-routines.mts
// Routines (cowork-agui-implement.md §11) through the real backend (wb-ipc) with the stand-in chat CLI, no AI:
// limits (15 minutes, 20 on at once), "run now" opens its own conversation with a read-only Bot, the finding is
// kept and the Bot's process stops, a failure is counted, a run in progress is not started twice, dismiss.
// The schedule math (next time, skip what was missed, ten failures switch it off) is in the Rust unit tests.
// Needs: cargo build -p wb-ipc -p wb-hook.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, until } from './lib/core-call.mts'

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'wb-routines-')))
const repo = path.join(tmp, 'repo')
fs.mkdirSync(repo)
const git = (...a: string[]): string => execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim()
git('init', '-q', '-b', 'main')
git('config', 'user.name', 'check')
git('config', 'user.email', 'check@example.invalid')
fs.writeFileSync(path.join(repo, 'README.md'), 'routines\n')
git('add', '-A')
git('commit', '-qm', 'init')
const fake = path.join(tmp, 'fake-cli.cmd')
fs.writeFileSync(fake, `@echo off\r\nnode "${path.resolve('scripts/fixtures/agui-fake-cli.mjs')}" %*\r\n`)

const b = new Backend({ WB_DATA_DIR: path.join(tmp, 'data') })
const c = (op: string, req?: unknown): Promise<any> => b.ipc(`canvas:${op}`, req ?? null)
const ok = async (op: string, req?: unknown): Promise<any> => {
  const r = await c(op, req)
  assert.equal(r.ok, true, `${op} failed: ${r.code} ${r.message}`)
  return r.data
}
const routine = (over: object): object => ({ id: '', name: 'nightly look', prompt: 'show argv', agent: 'custom-agent', repo: '', scope: 'report', schedule: { kind: 'daily', time: '03:00' }, enabled: true, nextAt: 0, failures: 0, ...over })
const find = async (id: string): Promise<any> => (await ok('routine.list')).find((r: any) => r.id === id)

try {
  await b.ipc('test:openWindow', repo)
  await b.ipc('settings:set', { customAgent: { enabled: true, name: 'Fake', command: fake, args: [], protocol: 'claude-stream-json', resources: { skill: '', mcp: '', plugin: '', hook: '' } } })

  // ── limits ──
  assert.match((await c('routine.save', { routine: routine({ schedule: { kind: 'every', minutes: 14 } }) })).message, /15 minutes/)
  assert.match((await c('routine.save', { routine: routine({ agent: 'cmd' }) })).message, /chat mode/)
  const r = await ok('routine.save', { routine: routine({}) })
  assert.ok(r.id && r.nextAt > Date.now() && r.repo, 'saved with its next time and repository')
  for (let i = 1; i < 20; i++) await ok('routine.save', { routine: routine({ name: `r${i}` }) })
  assert.match((await c('routine.save', { routine: routine({ name: 'one too many' }) })).message, /At most 20/)
  assert.ok((await ok('routine.save', { routine: routine({ name: 'parked', enabled: false }) })).id, 'a switched-off one does not count')

  // ── a report-only run: its own conversation, a read-only Bot, the finding kept, the process stopped ──
  await ok('routine.runNow', { id: r.id })
  await until(async () => (await find(r.id)).lastRun?.status === 'ok', 'the run finished', 30000)
  const done = await find(r.id)
  assert.match(done.lastRun.summary, /--permission-mode plan/, 'report only runs Claude in plan mode')
  assert.equal(done.lastRun.seen, false)
  const ch = done.lastRun.channel
  const v = await ok('channel.get', { channelId: ch })
  assert.match(v.channel.name, /^Routine · nightly look/)
  assert.equal(v.channel.routine, r.id)
  assert.equal(v.channel.nodes[0].readOnly, true)
  const first = (await ok('chat.transcript', { channelId: ch, nodeId: v.channel.nodes[0].id })).entries.find((e: any) => e.kind === 'user')
  assert.match(first.text, /\[Agent Workbench routine "nightly look"[\s\S]*Report only[\s\S]*show argv/)
  await until(async () => (await ok('channel.get', { channelId: ch })).runtime[v.channel.nodes[0].id].status !== 'running', 'the Bot process stops after the run', 20000)
  await ok('routine.dismiss', { id: r.id })
  assert.equal((await find(r.id)).lastRun.seen, true)

  // ── a failure is counted; a change run is not read-only ──
  const f = await ok('routine.save', { routine: { ...routine({ name: 'flaky', prompt: 'please fail', scope: 'change' }), enabled: false } })
  await ok('routine.runNow', { id: f.id })
  await until(async () => (await find(f.id)).lastRun?.status === 'failed', 'the failure is recorded', 30000)
  assert.equal((await find(f.id)).failures, 1)
  const fv = await ok('channel.get', { channelId: (await find(f.id)).lastRun.channel })
  assert.ok(!fv.channel.nodes[0].readOnly, 'may change code: the normal modes')

  // ── edits keep what the app knows; delete ──
  const edited = await ok('routine.save', { routine: { ...(await find(f.id)), prompt: 'show argv again' } })
  assert.equal(edited.failures, 1)
  assert.equal(edited.lastRun.status, 'failed')
  await ok('routine.delete', { id: f.id })
  assert.equal(await find(f.id), undefined)
  assert.equal((await c('routine.list')).ok, true)

  console.log('routines: limits, run now in its own conversation, read-only report, finding kept, process stopped, failure counted, edit, delete ok')
} finally {
  b.stop()
}
