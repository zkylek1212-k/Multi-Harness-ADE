// 自我檢查：node --experimental-strip-types scripts/check-cowork-gates.mts
// 完成條件、實作後審查、失控偵測、git 防護、核准綁定。用假 CLI 在暫存 repo 上跑真的 git。
import './ts-resolve.mjs'
import assert from 'node:assert'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import type * as Shared from '../src/shared/cowork.ts'

// app 的相對 import 不寫副檔名：要等 ts-resolve 掛好才能載入，所以用動態 import
const rust: any = await import('./lib/cowork-rust.mts')
const { shared, resolveCheckCommand, runCheck, CoworkService }: any = rust
const gitx: any = rust.gitm
const { PHONE_AUDITED_OPS, PHONE_COWORK_OPS } = await import('../src/shared/remoteProtocol.ts')

const here = path.dirname(fileURLToPath(import.meta.url))
const fake = path.join(here, 'fixtures', 'cowork-fake-cli.mjs')
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'cowork-gates-'))
const g = (cwd: string, ...args: string[]): string => execFileSync('git', ['-c', 'user.email=t@t', '-c', 'user.name=t', ...args], { cwd, encoding: 'utf8' })
const behave = (cfg: Record<string, unknown>): void => {
  process.env.FAKE_COWORK = JSON.stringify(cfg)
}
const code = async (p: Promise<unknown>, c: string): Promise<void> => {
  try {
    await p
  } catch (e) {
    assert.equal((e as { code: string }).code, c, (e as Error).message)
    return
  }
  assert.fail(`expected ${c}`)
}
function makeRepo(name: string): string {
  const dir = path.join(tmp, name)
  fs.mkdirSync(dir)
  g(dir, 'init', '-q', '-b', 'main')
  g(dir, 'config', 'user.email', 't@t')
  g(dir, 'config', 'user.name', 't')
  g(dir, 'config', 'core.autocrlf', 'false')
  fs.writeFileSync(path.join(dir, 'README.md'), 'hi\n')
  fs.writeFileSync(path.join(dir, '.gitignore'), 'node_modules\n')
  g(dir, 'add', '-A')
  g(dir, 'commit', '-qm', 'init')
  return dir
}

// ── 純邏輯 ──────────────────────────────────────────────────────────
{
  const task = { acceptance: ['a', 'b'] }
  assert.equal(shared.acceptanceGaps(task, undefined).length, 2, 'nothing reported')
  const res = shared.parseExecResult('```cowork-result\n{"acceptance":[{"item":1,"status":"done","evidence":"e"},{"item":2,"status":"not-done"}]}\n```')!
  assert.deepEqual(res.acceptance![0], { item: 1, status: 'done', evidence: 'e' })
  const gaps = shared.acceptanceGaps(task, res)
  assert.equal(gaps.length, 1)
  assert.match(gaps[0], /#2 not done/)
  assert.deepEqual(shared.acceptanceGaps({ acceptance: [] }, undefined), [])

  assert.ok(shared.validChecks([['npm', 'test']]))
  for (const bad of [[[]], [['']], 'npm test', [['a', 1]]]) assert.ok(!shared.validChecks(bad), JSON.stringify(bad))

  // 指紋不看截斷：前 120 字相同但後面不同，是不同的呼叫
  const long = 'x'.repeat(200)
  assert.notEqual(shared.toolFingerprint('Bash', { command: `${long}a` }), shared.toolFingerprint('Bash', { command: `${long}b` }))
  assert.equal(shared.toolFingerprint('Bash', { a: 1, b: 2 }), shared.toolFingerprint('Bash', { b: 2, a: 1 }))

  const m = new shared.RunawayMonitor(0)
  const call = (id: string, cmd: string) => ({ tools: [{ id, name: 'Bash', fingerprint: shared.toolFingerprint('Bash', { cmd }) }] })
  const seen: Shared.RunawaySignal[] = []
  for (let i = 0; i < 8; i++) {
    seen.push(...m.feed(call(`u${i}`, 'npm test'), i))
    seen.push(...m.feed({ toolResults: [{ id: `u${i}`, failed: true }] }, i))
  }
  assert.ok(seen.some((s) => s.signal === 'repeat-failure' && s.canStop), JSON.stringify(seen))
  // 合法輪詢：重複但成功，只提示不終止
  const poll = new shared.RunawayMonitor(0)
  const polled: Shared.RunawaySignal[] = []
  for (let i = 0; i < 12; i++) {
    polled.push(...poll.feed(call(`p${i}`, 'curl health'), i))
    polled.push(...poll.feed({ toolResults: [{ id: `p${i}`, failed: false }] }, i))
  }
  assert.ok(polled.some((s) => s.signal === 'repeat-call' && !s.canStop))
  assert.ok(!polled.some((s) => s.canStop), 'polling must not stop')
  // 長時間無改檔只提示
  const idle = new shared.RunawayMonitor(0, { idleMs: 1000 })
  assert.ok(idle.feed(call('a', 'ls'), 2000).some((s) => s.signal === 'no-file-change' && !s.canStop))

  // 解析：tool_result、codex 退出碼
  const u = shared.parseExecLine('claude', JSON.stringify({ type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 'x', is_error: true }] } }), 'C:/w')!
  assert.deepEqual(u.toolResults, [{ id: 'x', failed: true }])
  const cx = shared.parseExecLine('codex', JSON.stringify({ type: 'item.completed', item: { type: 'command_execution', command: 'npm test', exit_code: 2 } }), 'C:/w')!
  assert.equal(cx.tools![0].failed, true)

  const masked = shared.maskSecrets('token=abcdef123456 ghp_abcdefghijklmnopqrstuvwxyz ok')
  assert.ok(!masked.includes('abcdef123456') && !masked.includes('ghp_abc') && masked.includes('ok'))

  assert.deepEqual(shared.hookPathsIn(['.githooks/pre-commit', 'a.ts', 'tools/hooks/x'], 'tools/hooks'), ['.githooks/pre-commit', 'tools/hooks/x'])
  // 舊紀錄沒有 gates：不套新的合併限制
  assert.deepEqual(shared.mergeBlockers({ integration: { status: 'ok', branch: 'b' } } as any), [])

  // 手機 allowlist：桌面專屬的不開放；高風險操作要稽核
  for (const op of ['editBoard', 'execWaiveGitConfig', 'bypass', 'logDispatch', '__proto__']) assert.ok(!PHONE_COWORK_OPS.has(op), op)
  for (const op of ['execStart', 'execMerge', 'execAcceptScope', 'execWaive']) assert.ok(PHONE_COWORK_OPS.has(op) && PHONE_AUDITED_OPS.has(op), op)
  console.log('pure logic ok')
}

// ── Windows 指令解析 ────────────────────────────────────────────────
{
  const wt = path.join(tmp, 'resolve')
  fs.mkdirSync(wt)
  // agent 在 worktree 放的假 npm 不能取代真的
  fs.writeFileSync(path.join(wt, 'npm.cmd'), '@echo HIJACKED\r\n')
  const npm = await resolveCheckCommand('npm', wt)
  assert.ok('resolved' in npm, JSON.stringify(npm))
  if ('resolved' in npm) {
    assert.ok(path.dirname(npm.resolved).toLowerCase() !== wt.toLowerCase(), `hijacked by ${npm.resolved}`)
    if (process.platform === 'win32') assert.match(npm.resolved, /\.(cmd|exe)$/i)
  }
  const ran = await runCheck(['npm', '--version'], wt, { now: () => 0 })
  assert.ok(ran.ok, JSON.stringify(ran))
  assert.ok(!ran.output.includes('HIJACKED'))
  assert.ok('error' in (await resolveCheckCommand('definitely-not-a-command-xyz', wt)))
  assert.ok('error' in (await resolveCheckCommand('../outside.exe', wt)), 'relative paths cannot leave the worktree')
  const failing = await runCheck([process.execPath, '-e', 'console.error("boom token=abcdef123456"); process.exit(3)'], wt, { now: () => 0 })
  assert.equal(failing.ok, false)
  assert.equal(failing.code, 3)
  assert.ok(failing.output.includes('boom') && !failing.output.includes('abcdef123456'), 'output is masked')
  console.log('command resolution ok')
}

// ── 會議流程 ────────────────────────────────────────────────────────
const dataDir = path.join(tmp, 'data')
const unusable = new Set<string>()
const svc = new CoworkService({
  dataDir,
  resolveCli: (a: string) => (unusable.has(a) ? { error: 'not available' } : { command: process.execPath, prefixArgs: [fake] }),
  resolveExecCli: () => ({ command: process.execPath, prefixArgs: [fake] }),
  emit: () => {}
})
await svc.init()
const wait = async (id: string, cond: (r: any) => boolean, ms = 40000): Promise<any> => {
  const t0 = Date.now()
  for (;;) {
    const r = svc.get(id)!
    if (cond(r)) return r
    if (Date.now() - t0 > ms) throw new Error(`timeout; phase=${r.phase} tasks=${JSON.stringify(r.execution?.tasks, null, 1).slice(0, 1500)} review=${JSON.stringify(r.execution?.review)}`)
    await new Promise((res) => setTimeout(res, 50))
  }
}
const base = {
  prompt: 'Add a helper',
  chair: 'claude' as const,
  participants: ['claude', 'codex'] as ('claude' | 'codex')[],
  chairExecutes: true,
  language: 'en' as const,
  limits: { maxPlanningCalls: 12, maxPlanningMinutes: 10, maxExecutionMinutes: 60 }
}
/** 開會到核准；edit 可以改任務板（加 checks、改 scope、只留一個任務） */
const approved = async (workspace: string, edit?: (t: Shared.CoworkTask[]) => Shared.CoworkTask[]): Promise<any> => {
  const r0 = await svc.start({ ...base, workspace } as any)
  let r = await wait(r0.id, (x) => x.phase !== 'meeting')
  assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
  if (edit) {
    const res = await svc.editBoard(r.id, r.planRevision, edit(shared.currentBoard(r)!.tasks))
    assert.ok(res.ok, JSON.stringify(res))
    r = svc.get(r.id)!
  }
  await svc.approve(r.id, r.planRevision)
  return svc.get(r.id)!
}
const start = async (id: string, o: Record<string, unknown> = {}): Promise<void> => {
  const opts = { mode: 'sequential', linkDeps: false, bypass: false, review: false, ...o } as any
  const p = await svc.execPreview(id, opts)
  await svc.execStart(id, { ...opts, approval: p.hash })
}
const approvalOf = (id: string, waive?: string): Shared.MergeApproval => {
  const ex = svc.get(id)!.execution!
  return { commit: ex.integration?.commit || '', targetHead: ex.targetHead || '', ...(waive ? { waive } : {}) }
}
const finish = async (id: string): Promise<void> => {
  await svc.execCleanup(id)
  await svc.delete(id)
}
const solo = (t: Shared.CoworkTask[]): Shared.CoworkTask[] => t.slice(0, 1).map((x) => ({ ...x, dependsOn: [] }))

// 開始執行的核准：內容變了舊 hash 就失效
{
  const repo = makeRepo('approval')
  behave({})
  const r = await approved(repo, solo)
  const opts = { mode: 'sequential', linkDeps: false, bypass: false } as const
  const p = await svc.execPreview(r.id, opts)
  assert.equal(p.bypass, false)
  await code(svc.execStart(r.id, opts), 'stale')
  await code(svc.execStart(r.id, { ...opts, bypass: true, approval: p.hash }), 'stale')
  await code(svc.execStart(r.id, { ...opts, mode: 'parallel', approval: p.hash }), 'stale')
  const on = await svc.execPreview(r.id, { ...opts, bypass: true })
  assert.notEqual(on.hash, p.hash, 'Bypass is part of what gets approved')
  assert.ok(on.warnings.includes('bypass-on'))
  assert.equal(svc.get(r.id)!.phase, 'approved', 'nothing started')
  await svc.delete(r.id)
  console.log('start approval ok')
}

// 完成條件：漏報 → 追問一次 → 停下；豁免不等於驗證通過
{
  const repo = makeRepo('accept')
  behave({ execSkipAcceptance: true })
  const r = await approved(repo, solo)
  await start(r.id)
  let x = await wait(r.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  const te = x.execution.tasks.t1
  assert.equal(te.gate.kind, 'acceptance')
  assert.equal(te.gateFollowUps, 1, 'asked once, not forever')
  assert.equal(te.commits.length, 0, 'nothing committed while incomplete')
  assert.ok(te.turns.some((t: any) => /completion check/.test(t.text)), 'the follow-up went to the agent')
  await code(svc.execWaive(r.id, 't1', '   '), 'waive-reason-required')
  await svc.execWaive(r.id, 't1', 'checked by hand')
  x = await wait(r.id, (y) => y.phase === 'review')
  assert.equal(x.execution.tasks.t1.status, 'done')
  assert.equal(x.execution.tasks.t1.waiver.reason, 'checked by hand')
  assert.equal(x.execution.tasks.t1.gate, undefined)
  assert.ok(x.log.some((l: any) => l.t === 'task-waived'))
  await finish(r.id)

  // 追問後補齊就過
  behave({ execSkipAcceptance: true, fixOnFollowUp: true })
  const r2 = await approved(repo, solo)
  await start(r2.id)
  x = await wait(r2.id, (y) => y.phase === 'review')
  assert.equal(x.execution.tasks.t1.status, 'done')
  assert.equal(x.execution.tasks.t1.gateFollowUps, 1)
  assert.equal(x.execution.tasks.t1.waiver, undefined)
  await finish(r2.id)
  console.log('acceptance gate ok')
}

// checks：失敗要追問、改到內容要重跑、越界要停、解析不到不能開始
{
  const repo = makeRepo('checks')
  const node = process.execPath
  const withChecks = (checks: string[][]) => (t: Shared.CoworkTask[]) => solo(t).map((x) => ({ ...x, checks }))

  behave({})
  const missing = await approved(repo, withChecks([['definitely-not-a-command-xyz']]))
  const mp = await svc.execPreview(missing.id, { mode: 'sequential', linkDeps: false, bypass: false })
  assert.ok(mp.checks[0].error)
  await code(svc.execStart(missing.id, { mode: 'sequential', linkDeps: false, bypass: false, approval: mp.hash }), 'check-unresolved')
  await svc.delete(missing.id)

  const pass = await approved(repo, withChecks([[node, '-e', 'process.exit(0)']]))
  await start(pass.id)
  let x = await wait(pass.id, (y) => y.phase === 'review')
  assert.equal(x.execution.tasks.t1.status, 'done')
  assert.equal(x.execution.tasks.t1.checkRuns[0].ok, true)
  assert.ok(x.execution.tasks.t1.checkRuns[0].tree, 'records which content was checked')
  await finish(pass.id)

  const fail = await approved(repo, withChecks([[node, '-e', 'process.exit(5)']]))
  await start(fail.id)
  x = await wait(fail.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  assert.equal(x.execution.tasks.t1.gate.kind, 'checks')
  assert.equal(x.execution.tasks.t1.gateFollowUps, 1)
  assert.equal(x.execution.tasks.t1.commits.length, 0)
  await svc.execWaive(fail.id, 't1', 'known flaky')
  await wait(fail.id, (y) => y.phase === 'review')
  await finish(fail.id)

  // check 一直改待提交的內容：結果不可信
  const churn = await approved(repo, (t) => solo(t).map((y) => ({ ...y, scope: [...y.scope, 't1.txt'], checks: [[node, '-e', `require('fs').appendFileSync('t1.txt', String(Date.now()) + Math.random() + '\\n')`]] })))
  await start(churn.id)
  x = await wait(churn.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  assert.equal(x.execution.tasks.t1.gate.kind, 'checks')
  assert.match(x.execution.tasks.t1.gate.items[0], /keep changing/)
  await finish(churn.id)

  // check 改出範圍外的檔：不 commit，等使用者
  const stray = await approved(repo, (t) => solo(t).map((y) => ({ ...y, scope: [...y.scope, 't1.txt'], checks: [[node, '-e', `require('fs').writeFileSync('outside.txt', 'x')`]] })))
  await start(stray.id)
  x = await wait(stray.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  assert.deepEqual(x.execution.tasks.t1.scopeReview.files, ['outside.txt'])
  assert.equal(x.execution.tasks.t1.commits.length, 0)
  await finish(stray.id)
  console.log('task checks ok')
}

// 越界改動的核准：同檔名但內容變了要重新看
{
  const repo = makeRepo('scope')
  behave({ execStray: true })
  const r = await approved(repo, solo)
  await start(r.id)
  let x = await wait(r.id, (y) => y.execution?.tasks.t1.status === 'blocked' && y.execution.tasks.t1.scopeReview)
  const seen = x.execution.tasks.t1.scopeReview.hash
  fs.writeFileSync(path.join(x.execution.worktrees.main.path, 'stray.txt'), 'changed after review\n')
  await code(svc.execAcceptScope(r.id, 't1', seen), 'stale')
  const fresh = svc.get(r.id)!.execution!.tasks.t1.scopeReview!.hash
  assert.notEqual(fresh, seen)
  await svc.execAcceptScope(r.id, 't1', fresh)
  x = await wait(r.id, (y) => y.phase === 'review')
  assert.ok(x.execution.tasks.t1.changedFiles.includes('stray.txt'))
  await finish(r.id)
  console.log('scope approval ok')
}

// hook 信任：這些都實測過 git 真的會執行（Git 2.53）
{
  const repo = makeRepo('hooks')
  const marker = path.join(tmp, 'hook-marker.txt')
  fs.mkdirSync(path.join(repo, '.githooks'))
  fs.writeFileSync(path.join(repo, '.githooks', 'pre-commit'), `#!/bin/sh\necho BASELINE-pre-commit >> "${marker.replace(/\\/g, '/')}"\n`)
  fs.chmodSync(path.join(repo, '.githooks', 'pre-commit'), 0o755)
  g(repo, 'config', 'core.hooksPath', '.githooks')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'hooks')
  fs.writeFileSync(marker, '')
  behave({ execTamperHook: marker.replace(/\\/g, '/') })
  const r = await approved(repo, (t) => solo(t).map((y) => ({ ...y, scope: [...y.scope, '.githooks/'] })))
  await start(r.id)
  const x = await wait(r.id, (y) => y.phase === 'review')
  const seenCommit = fs.readFileSync(marker, 'utf8')
  assert.ok(seenCommit.includes('BASELINE-pre-commit'), `baseline hook still protects commits: ${seenCommit}`)
  assert.ok(!seenCommit.includes('TAMPERED'), `tampered hook must not run: ${seenCommit}`)
  assert.deepEqual([...x.execution.integration.hookChanges].sort(), ['.githooks/post-merge', '.githooks/pre-commit'])
  const blockers = shared.mergeBlockers(x.execution)
  assert.ok(blockers.some((b) => b.includes('hooks changed')), 'hook changes need an explicit decision')
  await code(svc.execMerge(r.id, approvalOf(r.id)), 'merge-needs-waiver')
  await svc.execMerge(r.id, approvalOf(r.id, 'reviewed the hook change'))
  const after = fs.readFileSync(marker, 'utf8')
  assert.ok(!after.includes('PLANTED'), `planted post-merge must not run in the main worktree: ${after}`)
  assert.ok(!after.includes('TAMPERED'))
  assert.ok(svc.get(r.id)!.log.some((l: any) => l.t === 'merge-waived'))
  // 對照組：沒有 hooksDir 時 git 真的會跑到被植入的 post-merge（證明這個測試不是空的）
  g(repo, 'checkout', '-q', '-b', 'plant', 'HEAD~1')
  fs.writeFileSync(path.join(repo, 'plant.txt'), 'x')
  g(repo, 'add', '-A')
  g(repo, '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'plant')
  g(repo, 'checkout', '-q', 'main')
  fs.writeFileSync(marker, '')
  g(repo, 'merge', '-q', '--no-ff', '-m', 'control', 'plant')
  assert.ok(fs.readFileSync(marker, 'utf8').includes('PLANTED'), 'control: unprotected merge runs the planted hook')
  await finish(r.id)
  console.log('hook trust ok')
}

// git 設定被改：在 harness 再跑 git 之前攔下；手機不能放行
{
  const repo = makeRepo('config')
  const marker = path.join(tmp, 'fsmonitor-marker.txt').replace(/\\/g, '/')
  fs.writeFileSync(marker, '')
  behave({ execTamperConfig: `echo FSMONITOR >> "${marker}"` })
  const r = await approved(repo, solo)
  await start(r.id)
  const x = await wait(r.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  assert.equal(x.execution.tasks.t1.gate.kind, 'git-config')
  await code(svc.execWaive(r.id, 't1', 'from the phone'), 'not-allowed')
  assert.equal(fs.readFileSync(marker, 'utf8'), '', 'the harness never ran the injected fsmonitor')
  g(repo, 'config', '--unset', 'core.fsmonitor')
  await svc.execWaive(r.id, 't1', 'inspected on the computer', true)
  await wait(r.id, (y) => y.phase === 'review')
  assert.equal(fs.readFileSync(marker, 'utf8'), '')
  await finish(r.id)
  console.log('git config guard ok')
}

// 共同 checks：parallel 的整合 worktree 要拿得到依賴；內容一致
{
  const repo = makeRepo('integration')
  fs.mkdirSync(path.join(repo, 'node_modules', 'dep'), { recursive: true })
  fs.writeFileSync(path.join(repo, 'node_modules', 'dep', 'index.js'), 'module.exports = 1\n')
  behave({})
  const r0 = await svc.start({ ...base, workspace: repo } as any)
  const r1 = await wait(r0.id, (x) => x.phase !== 'meeting')
  const board = shared.currentBoard(r1)!
  // 任務板層級的共同檢查：模型提出，一併核准。這裡直接放進目前的任務板，模擬模型輸出
  const boardChecks = [[process.execPath, '-e', `require('node:assert').ok(require('fs').existsSync('node_modules/dep/index.js'), 'deps'); require('node:assert').ok(require('fs').existsSync('t1.txt') && require('fs').existsSync('t2.txt'), 'both')`]]
  await svc.testPatch(r1.id, ['boards', '-1', 'checks'], boardChecks)
  await svc.approve(r1.id, r1.planRevision)
  await start(r1.id, { mode: 'parallel', linkDeps: true })
  const x = await wait(r1.id, (y) => y.phase === 'review')
  assert.equal(x.execution.worktrees.integration.depsLinked, true, 'integration worktree links deps')
  assert.equal(x.execution.integration.checks, 'passed', JSON.stringify(x.execution.integration.checkRuns))
  assert.ok(x.log.some((l: any) => l.t === 'integration-checks' && l.status === 'passed'))
  await finish(r1.id)

  // 共同 checks 失敗：不審查，合併要豁免
  const r2 = await svc.start({ ...base, workspace: repo } as any)
  const r3 = await wait(r2.id, (y) => y.phase !== 'meeting')
  const failingChecks = [[process.execPath, '-e', 'process.exit(9)']]
  await svc.testPatch(r3.id, ['boards', '-1', 'checks'], failingChecks)
  await svc.approve(r3.id, r3.planRevision)
  await start(r3.id, { mode: 'sequential', review: true })
  const y = await wait(r3.id, (z) => z.phase === 'review')
  assert.equal(y.execution.integration.checks, 'failed')
  assert.equal(y.execution.review, undefined, 'no paid review of something that already failed')
  await code(svc.execMerge(r3.id, approvalOf(r3.id)), 'merge-needs-waiver')
  await finish(r3.id)
  console.log('integration checks ok')
}

// 實作後審查：選人、標示、失敗不中斷、不能改檔、內容沒變不重審
{
  const repo = makeRepo('review')
  const log = path.join(tmp, 'review-log.jsonl')
  const reviews = (): any[] => fs.readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)).filter((c) => c.step === 'Review')
  fs.writeFileSync(log, '')
  // t1、t2 都由 claude 實作，codex 沒參與：應該由 codex 審
  const onlyClaude = (t: Shared.CoworkTask[]): Shared.CoworkTask[] => t.map((x) => ({ ...x, assignee: 'claude' as const }))
  behave({ logFile: log })
  const r = await approved(repo, onlyClaude)
  await start(r.id, { review: true })
  let x = await wait(r.id, (y) => y.phase === 'review')
  assert.equal(x.execution.review.status, 'ok', JSON.stringify(x.execution.review))
  assert.equal(x.execution.review.agent, 'codex', 'someone who did not implement')
  assert.deepEqual(x.execution.review.implementers, ['claude'])
  assert.equal(x.execution.review.independence, 'unconfirmed', 'different CLIs do not prove different models')
  assert.equal(x.execution.review.commit, x.execution.integration.commit)
  assert.equal(x.execution.review.patchId, x.execution.integration.patchId)
  assert.ok(reviews()[0].head.includes('DATA written by the implementer'))
  assert.ok(reviews()[0].head.includes('t1.txt'), 'the reviewer sees the diff')
  assert.deepEqual(shared.mergeBlockers(x.execution), [])
  assert.equal(svc.get(r.id)!.budget.planningCallsUsed <= base.limits.maxPlanningCalls, true)

  // 目標分支前進：不重新付費審查，但舊核准失效
  const stale = approvalOf(r.id)
  fs.writeFileSync(path.join(repo, 'other.txt'), 'moved\n')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'target moved')
  const newHead = g(repo, 'rev-parse', 'HEAD').trim()
  const before = reviews().length
  await code(svc.execMerge(r.id, stale), 'stale')
  x = svc.get(r.id)!
  assert.equal(x.execution.targetHead, newHead)
  assert.equal(x.execution.targetCheck.status, 'none')
  assert.equal(reviews().length, before, 'same content: no second paid review')
  assert.equal(x.execution.review.status, 'ok')
  await code(svc.execMerge(r.id, stale), 'stale')
  await code(svc.execMerge(r.id, { ...approvalOf(r.id), commit: 'deadbeef' }), 'stale')
  await svc.execMerge(r.id, approvalOf(r.id))
  assert.equal(svc.get(r.id)!.phase, 'completed')
  // 重送同一個核准：不重複合併
  const heads = g(repo, 'rev-parse', 'HEAD').trim()
  await svc.execMerge(r.id, stale)
  assert.equal(g(repo, 'rev-parse', 'HEAD').trim(), heads)
  await finish(r.id)

  // 目標分支前進且衝突：回報，不呼叫模型
  behave({ logFile: log })
  const c = await approved(repo, onlyClaude)
  await start(c.id, { review: true })
  await wait(c.id, (y) => y.phase === 'review')
  fs.writeFileSync(path.join(repo, 't1.txt'), 'conflicting content on main\n')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'conflict')
  const n = reviews().length
  await code(svc.execMerge(c.id, approvalOf(c.id)), 'stale')
  assert.equal(svc.get(c.id)!.execution!.targetCheck!.status, 'conflict')
  await code(svc.execMerge(c.id, approvalOf(c.id)), 'merge-conflict')
  assert.equal(reviews().length, n)
  await finish(c.id)
  g(repo, 'rm', '-q', 't1.txt')
  g(repo, 'commit', '-qm', 'cleanup')

  // 高嚴重度 finding：要豁免才能合併
  behave({ reviewHigh: true, reviewVerdict: 'fail' })
  const h = await approved(repo, onlyClaude)
  await start(h.id, { review: true })
  x = await wait(h.id, (y) => y.phase === 'review')
  assert.ok(shared.mergeBlockers(x.execution).some((b) => b.includes('seeded defect')))
  await code(svc.execMerge(h.id, approvalOf(h.id)), 'merge-needs-waiver')
  await finish(h.id)

  // 審查失敗：流程不中斷，標成未審查
  behave({ reviewFail: true })
  const f = await approved(repo, onlyClaude)
  await start(f.id, { review: true })
  x = await wait(f.id, (y) => y.phase === 'review')
  assert.equal(x.execution.review.status, 'failed')
  assert.ok(shared.mergeBlockers(x.execution).some((b) => b.startsWith('not reviewed')))
  await finish(f.id)

  // 審查者改檔：作廢
  behave({ reviewWrite: true })
  const w = await approved(repo, onlyClaude)
  await start(w.id, { review: true })
  x = await wait(w.id, (y) => y.phase === 'review')
  assert.equal(x.execution.review.status, 'failed')
  assert.match(x.execution.review.error, /read-only|snapshot/)
  assert.ok(!fs.existsSync(path.join(x.execution.worktrees.main.path, 'reviewer-was-here.txt')))
  await finish(w.id)

  // 只有實作者可用：同源審查，要明確標示（另一家在開完會後才不可用）
  behave({})
  const s1 = await approved(repo, onlyClaude)
  unusable.add('codex')
  await start(s1.id, { review: true })
  x = await wait(s1.id, (y) => y.phase === 'review')
  assert.equal(x.execution.review.independence, 'same-source')
  assert.equal(x.execution.review.agent, 'claude')
  unusable.delete('codex')
  await finish(s1.id)
  console.log('post-implementation review ok')
}

// 失控偵測：預設只記錄；開啟後相同呼叫持續失敗才停
{
  const repo = makeRepo('runaway')
  behave({ execLoop: 10 })
  const onlyClaude = (t: Shared.CoworkTask[]): Shared.CoworkTask[] => solo(t).map((x) => ({ ...x, assignee: 'claude' as const }))
  const r = await approved(repo, onlyClaude)
  await start(r.id)
  let x = await wait(r.id, (y) => y.phase === 'review')
  assert.equal(x.execution.tasks.t1.status, 'done', 'observation only by default')
  const seen = x.log.find((l: any) => l.t === 'runaway' && l.signal === 'repeat-failure')
  assert.ok(seen && seen.stopped === false, JSON.stringify(x.log.filter((l: any) => l.t === 'runaway')))
  assert.ok(x.execution.tasks.t1.watch.length > 0)
  await finish(r.id)

  const r2 = await approved(repo, onlyClaude)
  await start(r2.id, { runawayStop: true })
  x = await wait(r2.id, (y) => y.execution?.tasks.t1.status === 'blocked')
  assert.match(x.execution.tasks.t1.error, /Stopped by Cowork/)
  assert.ok(x.log.some((l: any) => l.t === 'runaway' && l.stopped === true))
  await finish(r2.id)
  console.log('runaway monitor ok')
}

// worktree 位置可以放到同步資料夾外；清理只動那個位置
{
  const repo = makeRepo('wtroot')
  const root = path.join(tmp, 'outside-worktrees')
  behave({})
  const r = await approved(repo, solo)
  const p = await svc.execPreview(r.id, { mode: 'sequential', linkDeps: false, bypass: false, worktreeRoot: root })
  assert.equal(p.worktreeRoot, path.resolve(root))
  await svc.execStart(r.id, { mode: 'sequential', linkDeps: false, bypass: false, worktreeRoot: root, review: false, approval: p.hash })
  const x = await wait(r.id, (y) => y.phase === 'review')
  assert.ok(path.resolve(x.execution.worktrees.main.path).startsWith(path.resolve(root)))
  assert.ok(!fs.existsSync(path.join(repo, '.cowork', r.id)))
  const keep = path.join(root, 'someone-elses-file.txt')
  fs.writeFileSync(keep, 'x')
  await finish(r.id)
  assert.ok(fs.existsSync(keep), 'cleanup stays inside the run folder')
  assert.ok(!fs.existsSync(path.join(root, r.id)))
  assert.ok((await svc.execPreview((await approved(repo, solo)).id, { mode: 'sequential', linkDeps: false, bypass: false, worktreeRoot: 'relative/path' })).worktreeRoot.endsWith('.cowork'), 'relative paths are ignored')
  console.log('worktree root ok')
}

// git 工具函式：patch-id 跨基底穩定、merge-tree 回報衝突、hook 副本取自基線 commit
{
  const repo = makeRepo('gitx')
  const base0 = g(repo, 'rev-parse', 'HEAD').trim()
  g(repo, 'checkout', '-q', '-b', 'f')
  fs.writeFileSync(path.join(repo, 'f.txt'), 'f\n')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'f')
  const f = g(repo, 'rev-parse', 'HEAD').trim()
  const id = await gitx.patchId(repo, base0, f)
  assert.ok(id)
  assert.equal(await gitx.patchId(repo, base0, base0), '')
  const ok = await gitx.mergeTree(repo, 'main', f)
  assert.ok(ok.ok)
  g(repo, 'checkout', '-q', 'main')
  fs.writeFileSync(path.join(repo, 'f.txt'), 'main\n')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'm')
  const bad = await gitx.mergeTree(repo, 'main', f)
  assert.equal(bad.ok, false)
  assert.equal(g(repo, 'status', '--porcelain'), '', 'merge-tree leaves the working copy alone')

  g(repo, 'config', 'core.hooksPath', '.githooks')
  fs.mkdirSync(path.join(repo, '.githooks'))
  fs.writeFileSync(path.join(repo, '.githooks', 'pre-commit'), 'v1\n')
  g(repo, '-c', 'core.hooksPath=/dev/null', 'add', '-A')
  g(repo, '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'hook')
  const commit = g(repo, 'rev-parse', 'HEAD').trim()
  fs.writeFileSync(path.join(repo, '.githooks', 'pre-commit'), 'v2-working-copy\n')
  const dest = await gitx.exportBaselineHooks(repo, commit, path.join(tmp, 'hook-copy'))
  assert.equal(fs.readFileSync(path.join(dest, 'pre-commit'), 'utf8'), 'v1\n', 'from the commit, not the working copy')

  const state = await gitx.gitGuardState(repo)
  assert.deepEqual(gitx.diffGuard(state, state), [])
  g(repo, 'config', 'filter.evil.clean', 'evil')
  assert.equal(gitx.diffGuard(state, await gitx.gitGuardState(repo)).length, 1)
  console.log('git helpers ok')
}

try {
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3 })
} catch {
  /* Windows 偶爾還有 handle 沒放 */
}
console.log('cowork gates ok')
rust.stop()
