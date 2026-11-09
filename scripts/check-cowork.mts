// 自我檢查：node --experimental-strip-types scripts/check-cowork.mts
// Cowork P1：純邏輯、runner、原子寫入、git 快照，以及用假 CLI 在暫存 repo 上跑完整的會議流程。
// 對應 cowork.md §10 的測試重點：假 JSON、無效 DAG／越界路徑、單一覆核者的反對不能被繞過、
// 失敗／預算／取消時不能假裝完成、舊版本不能核准、重啟後暫停。
import './ts-resolve.mjs'
import assert from 'node:assert'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const rust: any = await import('./lib/cowork-rust.mts')
const { shared, gitm, CoworkService, CoworkError }: any = rust

const tmp = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), 'cowork-check-')))
const cleanup: string[] = [tmp]

// ── 純邏輯 ─────────────────────────────────────────────────────────

{
  const n = shared.normalizeScopePath
  assert.deepEqual(n('src\\main\\a.ts'), { ok: true, path: 'src/main/a.ts' })
  assert.deepEqual(n('./src//b.ts'), { ok: true, path: 'src/b.ts' })
  assert.deepEqual(n('src/dir/'), { ok: true, path: 'src/dir/' })
  for (const bad of ['', '/etc/passwd', 'C:/x', '..\\x', 'a/../../b', '.git/config', '.GIT/hooks', 'src/*.ts', 'a:stream']) {
    assert.equal(n(bad).ok, false, `should reject ${JSON.stringify(bad)}`)
  }

  const ctx = { assignable: ['claude', 'codex'] as const, maxTasks: 3 }
  const t = (o: Record<string, unknown>) => ({ id: 't1', title: 'x', detail: 'd', scope: ['a.ts'], dependsOn: [], assignee: 'claude', assignmentReason: 'owns a.ts', acceptance: [], resources: [], ...o })
  const ok = shared.checkTasks([t({}), t({ id: 't2', dependsOn: ['t1'], scope: ['b\\c.ts'] })], { ...ctx, assignable: [...ctx.assignable] })
  assert.ok(ok.ok)
  if (ok.ok) assert.deepEqual(ok.value[1].scope, ['b/c.ts'])
  const errs = (raw: unknown) => {
    const r = shared.checkTasks(raw, { ...ctx, assignable: [...ctx.assignable] })
    return r.ok ? [] : r.errors.join('\n')
  }
  assert.match(String(errs([t({ assignee: 'antigravity' })])), /assignee/)
  assert.match(String(errs([t({}), t({})])), /duplicate task id/)
  assert.match(String(errs([t({ dependsOn: ['t9'] })])), /unknown task/)
  assert.match(String(errs([t({ dependsOn: ['t2'] }), t({ id: 't2', dependsOn: ['t1'] })])), /cycle/)
  assert.match(String(errs([t({ scope: ['../x'] })])), /not allowed/)
  assert.match(String(errs([t({}), t({ id: 't2' }), t({ id: 't3' }), t({ id: 't4' })])), /too many tasks/)
  assert.match(String(errs([])), /must not be empty/)
  assert.match(String(errs([t({ scope: ['src/'] }), t({ id: 't2', scope: ['src/a.ts'] })])), /overlapping scopes/)
  assert.equal(shared.checkTasks([t({ scope: ['src/'] }), t({ id: 't2', scope: ['src/a.ts'], dependsOn: ['t1'] })], { ...ctx, assignable: [...ctx.assignable] }).ok, true)
  assert.equal(shared.checkTasks([t({ scope: ['src/'] }), t({ id: 't2', scope: ['src2/a.ts'] })], { ...ctx, assignable: [...ctx.assignable] }).ok, true)

  const res = shared.checkResolution(
    { tasks: [t({})], decisions: [{ issueId: 'codex.o1', verdict: 'accept', reason: 'ok' }], unresolved: [] },
    { assignable: ['claude'], maxTasks: 5, requiredIssues: ['codex.o1', 'claude.q1'] }
  )
  assert.ok(!res.ok && res.errors.some((e) => e.includes('claude.q1')), 'every issue must be handled')
  const res2 = shared.checkResolution(
    { tasks: [t({})], decisions: [{ issueId: 'codex.o1', verdict: 'accept', reason: 'ok' }], unresolved: [{ issueId: 'claude.q1', text: '?' }] },
    { assignable: ['claude'], maxTasks: 5, requiredIssues: ['codex.o1', 'claude.q1'] }
  )
  assert.ok(res2.ok, 'unresolved counts as handled')

  const r1 = shared.checkR1(
    { summary: 's', framing: 'f', tasks: [t({})], questions: [{ id: 'zz', text: 'q?' }, { id: 'zz', text: 'q2?' }], risks: [] },
    { assignable: ['claude'], maxTasks: 5 }
  )
  assert.ok(r1.ok)
  if (r1.ok) assert.deepEqual(r1.value.questions.map((q) => q.id), ['q1', 'q2'], 'question ids are renumbered')

  // 指派理由：模型產生的任務必填；使用者改板可以留空
  const noReason = shared.checkR1({ summary: 's', framing: 'f', tasks: [t({ assignmentReason: '  ' })], questions: [], risks: [] }, { assignable: ['claude'], maxTasks: 5 })
  assert.ok(!noReason.ok && noReason.errors.some((e) => /assignmentReason/.test(e)), 'model tasks need a reason')
  assert.ok(shared.checkTasks([t({ assignmentReason: undefined })], { ...ctx, assignable: [...ctx.assignable] }).ok, 'user edits may omit the reason')
  // 主席的假設：只收合法項目，不合法的不讓規劃失敗
  const withAssumptions = shared.checkR1(
    { summary: 's', framing: 'f', assumptions: [{ agent: 'codex', text: 'good at tests' }, { agent: 'nobody', text: 'x' }, 'junk'], tasks: [t({})], questions: [], risks: [] },
    { assignable: ['claude'], maxTasks: 5 }
  )
  assert.ok(withAssumptions.ok)
  if (withAssumptions.ok) assert.deepEqual(withAssumptions.value.assumptions, [{ agent: 'codex', text: 'good at tests' }])

  // 越界檢查：目錄 scope、沒有斜線的項目、大小寫、未指定 scope
  assert.deepEqual(shared.filesOutsideScope(['src/', 'README.md'], ['src/a.ts', 'readme.md', 'src2/x.ts', 'docs/a.md']), ['src2/x.ts', 'docs/a.md'])
  assert.deepEqual(shared.filesOutsideScope(['src/lib'], ['src/lib/a.ts', 'src/lib', 'src/library.ts']), ['src/library.ts'])
  assert.deepEqual(shared.filesOutsideScope([], ['a.ts']), ['a.ts'], 'no declared scope means any change is outside')
  // 會改依賴的任務
  assert.equal(shared.touchesDependencies(['src/a.ts']), false)
  assert.equal(shared.touchesDependencies(['package.json']), true)
  assert.equal(shared.touchesDependencies(['apps/linux/package-lock.json']), true)
  assert.equal(shared.touchesDependencies(['node_modules/']), true)
  // 結構化結果：取最後一個區塊，格式錯誤不丟例外
  const res3 = shared.parseExecResult('x\n```cowork-result\n{"interfaces":["a"]}\n```\ny\n```cowork-result\r\n{"interfaces":["b"],"verified":["v"],"unverified":[],"notes":"n"}\r\n```')
  assert.deepEqual(res3, { interfaces: ['b'], verified: ['v'], unverified: [], notes: 'n' })
  assert.equal(shared.parseExecResult('```cowork-result\n{not json}\n```'), undefined)
  assert.equal(shared.parseExecResult('no block'), undefined)
  const r2 = shared.checkR2({ agree: [], objections: [{ id: 'o1', target: '', reason: 'r', alternative: 'a' }, { id: 'o1', target: 't1', reason: 'r2', alternative: '' }], missing: [], claims: [], answers: [] })
  assert.ok(r2.ok)
  if (r2.ok) assert.deepEqual(r2.value.objections.map((o) => [o.id, o.target]), [['o1', 'framing'], ['o2', 't1']])
  assert.equal(shared.checkR2('not json').ok, false)

  // claude 外層格式
  const env = (o: Record<string, unknown>) => JSON.stringify({ type: 'result', subtype: 'success', is_error: false, structured_output: { a: 1 }, total_cost_usd: 0.5, usage: { input_tokens: 1, cache_creation_input_tokens: 2, cache_read_input_tokens: 3, output_tokens: 4 }, ...o })
  const pc = shared.parseClaudeOutput(env({}))
  assert.ok(pc.ok)
  if (pc.ok) assert.deepEqual(pc.value, { a: 1 })
  assert.equal(pc.usage?.costUsd, 0.5)
  assert.equal(shared.parseClaudeOutput(env({ is_error: true, result: 'auth failed' })).ok, false)
  assert.equal(shared.parseClaudeOutput(env({ structured_output: undefined })).ok, false)
  assert.equal(shared.parseClaudeOutput('nope').ok, false)

  // codex 事件 + -o 檔
  const ev = (...e: unknown[]) => e.map((x) => JSON.stringify(x)).join('\n')
  const usageEv = { type: 'turn.completed', usage: { input_tokens: 100, cached_input_tokens: 60, output_tokens: 7 } }
  const okCmd = { type: 'item.completed', item: { type: 'command_execution', status: 'completed', exit_code: 0, aggregated_output: '' } }
  const denied = { type: 'item.completed', item: { type: 'command_execution', status: 'failed', exit_code: 1, aggregated_output: "Access to the path 'C:\\x' is denied." } }
  const px = shared.parseCodexOutput(ev(okCmd, usageEv), '{"b":2}')
  assert.ok(px.ok)
  if (px.ok) assert.deepEqual(px.value, { b: 2 })
  assert.deepEqual(px.usage, { inputTokens: 40, cachedInputTokens: 60, outputTokens: 7 })
  assert.equal(shared.parseCodexOutput(ev(denied, usageEv), '{"b":2}').ok, false, 'sandbox could not read anything')
  assert.ok(shared.parseCodexOutput(ev(denied, okCmd, usageEv), '{"b":2}').ok, 'one denial among successes is fine')
  assert.equal(shared.parseCodexOutput(ev({ type: 'turn.failed', error: { message: 'boom' } }), '{"b":2}').ok, false)
  assert.equal(shared.parseCodexOutput(ev(usageEv), null).ok, false)
  assert.equal(shared.parseCodexOutput(ev(usageEv), 'not json').ok, false)

  // 規劃用參數：唯讀設定不能被改掉
  const inv = shared.plannerInvocation('claude', { cwd: '.', schema: {}, schemaFile: 's', outFile: 'o' })
  assert.ok(inv.args.join(' ').includes('--tools Read,Grep,Glob') && inv.args.includes('--restricted') && inv.args.includes('--strict-mcp-config'))
  assert.ok(!inv.args.some((a) => /bypass|dangerously/i.test(a)))
  const cx = shared.plannerInvocation('codex', { cwd: 'C:/snap', schema: {}, schemaFile: 's.json', outFile: 'o.txt', windowsSandbox: 'elevated' })
  assert.ok(cx.args.includes('read-only') && cx.args.includes('--ignore-user-config') && cx.args.includes('windows.sandbox="elevated"'))
  assert.equal(cx.args[cx.args.length - 1], '-', 'prompt goes through stdin')
  // antigravity：prompt 走 stream-json stdin（-p 一定要接值，所以是 -p=）、即時工具白名單、較長逾時
  const ag = shared.plannerInvocation('antigravity', { cwd: '.', schema: {}, schemaFile: 's.json', outFile: 'o' })
  assert.ok(ag.args.includes('stream-json') && ag.args.includes('--sandbox') && ag.args.includes('s.json'))
  assert.equal(ag.args[ag.args.length - 1], '-p=')
  assert.ok(!ag.args.some((a) => /dangerously|skip-permissions/i.test(a)))
  assert.deepEqual(JSON.parse(ag.stdin!('中文 "q"\nline')), { event: 'user', message: { role: 'user', content: '中文 "q"\nline' } })
  assert.ok((ag.minTimeoutSec || 0) >= 600)
  const step = (tool: string, state = 'ACTIVE') => JSON.stringify({ event: 'step_update', step_update: { state, step_type: 'tool', tool_name: tool } })
  assert.equal(ag.guard!(step('view_file')), null)
  assert.equal(ag.guard!(step('grep_search')), null)
  assert.equal(ag.guard!(step('run_command')), null, 'run_command is refused by the deny rule, not killed')
  for (const bad of ['schedule', 'invoke_subagent', 'open_browser_url', 'write_to_file', 'search_web', 'send_message']) {
    assert.match(String(ag.guard!(step(bad))), new RegExp(bad), `${bad} must be stopped`)
  }
  assert.equal(ag.guard!(step('schedule', 'DONE')), null, 'only the ACTIVE event triggers')
  assert.equal(ag.guard!('not json "tool_name"'), null)
  const deny = (shared.agyIsolationSettings('C:\\iso\\home') as any).permissions
  for (const d of ['command(*)', 'write_file(*)', 'mcp(*)', 'read_url(*)', 'execute_url(*)']) assert.ok(deny.deny.includes(d), d)
  assert.deepEqual(deny.allow, ['read_file(C:/iso/home/.gemini/antigravity-cli/builtin/**)'])
  const agEv = (r: Record<string, unknown>) =>
    [JSON.stringify({ event: 'init' }), JSON.stringify({ event: 'result', result: { status: 'SUCCESS', usage: { input_tokens: 10, output_tokens: 2, cache_read_tokens: 5 }, ...r } })].join('\n')
  const pa = shared.parseAgyOutput(agEv({ structured_output: { c: 3 } }))
  assert.ok(pa.ok)
  if (pa.ok) assert.deepEqual(pa.value, { c: 3 })
  assert.deepEqual(pa.usage, { inputTokens: 10, cachedInputTokens: 5, outputTokens: 2 })
  const noOut = shared.parseAgyOutput(agEv({ denied_actions: [{ action: 'read_file' }] }))
  assert.ok(!noOut.ok && /read_file/.test(noOut.error))
  assert.equal(shared.parseAgyOutput(agEv({ status: 'ERROR', error: 'boom' })).ok, false)
  assert.equal(shared.parseAgyOutput('garbage').ok, false)
  // 模型與強度：各家用自己的參數；不指定就不帶（agy 例外：強度預設 medium）
  const argsFor = (agent: 'claude' | 'codex' | 'antigravity', model?: string, effort?: string) =>
    shared.plannerInvocation(agent, { cwd: '.', schema: {}, schemaFile: 's.json', outFile: 'o', model, effort }).args.join(' ')
  assert.ok(argsFor('claude', 'opus', 'high').includes('--model opus --effort high'))
  assert.ok(!argsFor('claude').includes('--model') && !argsFor('claude').includes('--effort'))
  assert.ok(argsFor('codex', 'gpt-6.1-sol', 'xhigh').includes('-m gpt-6.1-sol -c model_reasoning_effort="xhigh"'))
  assert.ok(!argsFor('codex').includes(' -m ') && !argsFor('codex').includes('model_reasoning_effort'))
  // agy：ID 本身帶強度。模型＋強度 → 組成完整 ID；只有模型 → 不加 --effort；都沒有 → --effort medium（實測規則）
  assert.ok(argsFor('antigravity', 'gemini-3.8-flash', 'low').includes('--model gemini-3.8-flash-low') && !argsFor('antigravity', 'gemini-3.8-flash', 'low').includes('--effort'))
  assert.ok(argsFor('antigravity', 'claude-sonnet-4-6').includes('--model claude-sonnet-4-6') && !argsFor('antigravity', 'claude-sonnet-4-6').includes('--effort'))
  assert.ok(argsFor('antigravity').includes('--effort medium') && !argsFor('antigravity').includes('--model'))
  assert.ok(argsFor('antigravity', '', 'high').includes('--effort high'))
  const grouped = shared.groupAgyModels([
    { id: 'gemini-3.8-flash-high', label: 'Gemini 3.8 Flash (High)' },
    { id: 'gemini-3.8-flash-low', label: 'Gemini 3.8 Flash (Low)' },
    { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6 (Thinking)' },
    { id: 'gpt-oss-120b-medium', label: 'GPT-OSS 120B (Medium)' }
  ])
  assert.deepEqual(grouped, [
    { id: 'gemini-3.8-flash', label: 'Gemini 3.8 Flash', efforts: ['high', 'low'] },
    { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6 (Thinking)', efforts: [] },
    { id: 'gpt-oss-120b', label: 'GPT-OSS 120B', efforts: ['medium'] }
  ])
  // 不安全的值一律丟掉（會接進命令列，codex 的強度還會被當成 TOML）
  assert.ok(!argsFor('codex', 'x" ; rm -rf', 'high"').includes('rm') && !argsFor('codex', 'x', 'hi"gh').includes('hi"gh'))
  assert.deepEqual(shared.sanitizeModelChoice({ model: ' opus ', effort: 'HIGH' }), { model: 'opus', effort: '' })
  assert.deepEqual(shared.sanitizeCoworkSettings({ models: { claude: { model: 'opus', effort: 'high' }, codex: { model: '', effort: '' }, bogus: {} } }).models, {
    claude: { model: 'opus', effort: 'high' }
  })
  // claude 回報實際用的模型
  assert.equal(shared.parseClaudeOutput(env({ modelUsage: { 'claude-opus-5-5': {} } })).model, 'claude-opus-5-5')
  const catalogResponse = JSON.stringify({ type: 'control_response', response: { request_id: 'cowork-models', response: { models: [
    { value: 'default', resolvedModel: 'claude-opus-5-5' },
    { value: 'opus', resolvedModel: 'claude-opus-5-5', displayName: 'Opus', description: 'Opus 5.5 · Best for everyday tasks', supportedEffortLevels: ['low', 'high', 'bad"value'] },
    { value: 'haiku', resolvedModel: 'claude-haiku-4-5', description: 'Haiku 4.5 · Fast', supportsEffort: false },
    // Claude Code 2.1.296 的格式：名稱在 displayName，description 只有說明
    { value: 'sonnet', resolvedModel: 'claude-sonnet-5-5', displayName: 'Sonnet 5.5', description: 'Most efficient for simpler tasks' },
    { value: 'bad"value' }
  ] } } })
  const catalog = shared.parseClaudeModelCatalog('not json\n' + catalogResponse, 'opus')
  assert.equal(catalog.fallback.model, 'opus')
  assert.equal(catalog.fallback.source, 'user-config')
  assert.deepEqual(catalog.options, [
    { id: 'opus', resolvedModel: 'claude-opus-5-5', label: 'Opus 5.5', efforts: ['low', 'high'] },
    { id: 'haiku', resolvedModel: 'claude-haiku-4-5', label: 'Haiku 4.5', efforts: [] },
    { id: 'sonnet', resolvedModel: 'claude-sonnet-5-5', label: 'Sonnet 5.5' }
  ])
  assert.equal(shared.parseClaudeModelCatalog(catalogResponse, '').fallback.model, 'claude-opus-5-5')
  assert.throws(() => shared.parseClaudeModelCatalog('{}', ''), /model list/)

  // 工具說明插在步驟標記之後，假 CLI 與真 CLI 都還認得步驟
  const tn = shared.withToolNote('Cowork step: R1 (chair opening)\nrest', 'antigravity')
  assert.ok(tn.startsWith('Cowork step: R1') && tn.includes('list_dir') && tn.endsWith('\nrest'))

  // schema 相容 codex strict：每個 object 都 additionalProperties:false 且 required 列全
  const walk = (s: any): void => {
    if (s && typeof s === 'object') {
      if (s.type === 'object') {
        assert.equal(s.additionalProperties, false)
        assert.deepEqual([...s.required].sort(), Object.keys(s.properties).sort())
      }
      Object.values(s).forEach(walk)
    }
  }
  for (const st of ['r1', 'r2', 'r34', 'revise'] as const) walk(shared.schemaFor(st, ['claude', 'codex']))

  assert.equal(shared.sanitizeCoworkSettings({ chair: 'nope', participants: ['codex', 'codex', 'x'], limits: { maxPlanningCalls: 9999 } }).limits.maxPlanningCalls, 30)
  assert.deepEqual(shared.sanitizeCoworkSettings({ participants: ['codex', 'codex', 'x'] }).participants, ['codex'])
  console.log('cowork pure ok')
}

// ── git：基線、快照、副作用、刪除守衛、symlink 逃逸 ──────────────────

const g = (cwd: string, ...args: string[]): string =>
  execFileSync('git', ['-c', 'user.email=t@t', '-c', 'user.name=t', ...args], { cwd, encoding: 'utf8' })

function makeRepo(name: string): string {
  const repo = path.join(tmp, name)
  fs.mkdirSync(path.join(repo, 'src'), { recursive: true })
  g(repo, 'init', '-q')
  fs.writeFileSync(path.join(repo, 'src', 'a.ts'), 'export const a = 1\n')
  fs.writeFileSync(path.join(repo, '.gitignore'), 'out/\n')
  g(repo, 'add', '-A')
  g(repo, 'commit', '-qm', 'init')
  return repo
}

{
  const repo = makeRepo('repo-git')
  fs.writeFileSync(path.join(repo, 'dirty.txt'), 'x')
  const base = await gitm.readBaseline(path.join(repo, 'src'))
  assert.equal(path.resolve(base.root), path.resolve(repo))
  assert.deepEqual(base.dirty, ['dirty.txt'])
  assert.match(base.head, /^[0-9a-f]{40}$/)

  const snap = gitm.snapshotDirFor(base.commonDir, 'rtest')
  await gitm.createSnapshot(base.root, snap, base.head)
  assert.ok(fs.existsSync(path.join(snap, 'src', 'a.ts')))
  assert.ok(!fs.existsSync(path.join(snap, 'dirty.txt')), 'uncommitted files are not in the snapshot')
  assert.deepEqual(await gitm.snapshotChanges(snap, base.head), [])
  fs.mkdirSync(path.join(snap, 'out'))
  fs.writeFileSync(path.join(snap, 'out', 'gen.js'), 'x')
  assert.ok((await gitm.snapshotChanges(snap, base.head)).some((l) => l.includes('out/')), 'ignored writes count as side effects')

  if (process.platform === 'win32') {
    const outside = path.join(tmp, 'outside')
    fs.mkdirSync(outside)
    fs.symlinkSync(outside, path.join(snap, 'link'), 'junction')
    assert.ok(gitm.scopeEscapes(snap, 'link/new.ts'), 'junction escaping the tree is caught')
  }
  assert.equal(gitm.scopeEscapes(snap, 'src/new/deep/file.ts'), null)

  assert.throws(() => gitm.removeSnapshot(repo, path.join(base.commonDir, 'cowork')), /refusing/)
  gitm.removeSnapshot(snap, path.join(base.commonDir, 'cowork'))
  assert.ok(!fs.existsSync(path.join(base.commonDir, 'cowork')), 'empty cowork dir is removed too')
  assert.ok(fs.existsSync(path.join(repo, 'src', 'a.ts')), 'repo untouched')

  await assert.rejects(gitm.readBaseline(tmp), /not-a-git-repo/)
  console.log('cowork git ok')
}

// ── orchestrator：假 CLI 跑完整會議 ──────────────────────────────────

const fake = path.join(here, 'fixtures', 'cowork-fake-cli.mjs')
const repo = makeRepo('repo-e2e')
const dataDir = path.join(tmp, 'data')
const emitted: string[] = []
const agyHome = path.join(tmp, 'agy-home')
const ineligible = new Set<string>()
const deps = {
  dataDir,
  resolveCli: (agent: string) =>
    ineligible.has(agent)
      ? { error: 'not available' }
      : agent === 'antigravity'
        ? { command: process.execPath, prefixArgs: [fake], env: { USERPROFILE: agyHome, HOME: agyHome } }
        : agent === 'codex'
          ? // 模擬使用者 config.toml 的 model／model_reasoning_effort
            { command: process.execPath, prefixArgs: [fake], defaultModel: 'gpt-user', defaultEffort: 'low' }
          : { command: process.execPath, prefixArgs: [fake] },
  emit: (run: { id: string; phase: string }) => emitted.push(`${run.id}:${run.phase}`)
}
const behave = (cfg: Record<string, unknown>): void => {
  process.env.FAKE_COWORK = JSON.stringify(cfg)
}
const svc = new CoworkService(deps)
await svc.init()

const base = {
  workspace: repo,
  prompt: 'Add a helper and use it',
  chair: 'claude' as const,
  participants: ['claude', 'codex'] as ('claude' | 'codex')[],
  chairExecutes: true,
  language: 'en' as const,
  limits: { maxPlanningCalls: 6, maxPlanningMinutes: 10, maxExecutionMinutes: 60 }
}

async function waitFor(id: string, cond: (r: any) => boolean, ms = 30000, service = svc): Promise<any> {
  const t0 = Date.now()
  for (;;) {
    const r = service.get(id)
    if (r && cond(r)) return r
    if (Date.now() - t0 > ms) throw new Error(`timeout waiting; phase=${r?.phase} block=${JSON.stringify(r?.block)}`)
    await new Promise((res) => setTimeout(res, 50))
  }
}
const settled = (r: any) => r.phase !== 'meeting'
const code = async (p: Promise<unknown> | (() => unknown), c: string): Promise<void> => {
  try {
    await (typeof p === 'function' ? p() : p)
  } catch (e) {
    assert.ok(e instanceof CoworkError, String(e))
    assert.equal((e as InstanceType<typeof CoworkError>).code, c)
    return
  }
  assert.fail(`expected CoworkError ${c}`)
}

// 1. 正常流程：主席 claude、覆核 codex，有反對與補充 → 全部處置 → 待核准 → 核准
{
  behave({ r2Objection: true })
  const run0 = await svc.start({ ...base, mode: 'project', autoEffort: true })
  assert.equal(run0.phase, 'meeting')
  const r = await waitFor(run0.id, settled)
  assert.equal(r.calls.find((c: any) => c.step === 'r1')?.effort, 'medium')
  assert.equal(r.calls.find((c: any) => c.step === 'r2')?.effort, 'high')
  assert.equal(r.calls.find((c: any) => c.step === 'r34')?.effort, 'high')
  assert.ok(r.reviewers.codex.output.message && shared.currentBoard(r)!.message, 'public review and chair response survive validation')
  assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
  const board = shared.currentBoard(r)!
  assert.deepEqual(board.decisions.map((d: any) => d.issueId).sort(), ['claude.q1', 'codex.m1', 'codex.o1'])
  assert.deepEqual(board.tasks[1].scope, ['src/b.ts', 't2.txt'], 'scope normalized')
  assert.equal(r.budget.planningCallsUsed, 3, 'N+1 calls for two agents')
  assert.ok(r.budget.costUsd > 0 && r.budget.tokens > 0)
  assert.equal(r.budget.activeSince, null)
  assert.ok(r.log.some((e: any) => e.t === 'round' && e.round === 3))
  assert.ok(fs.existsSync(r.snapshotDir))
  assert.equal(g(repo, 'status', '--porcelain'), '', 'the real repo was never touched')

  await code(async () => await svc.approve(r.id, r.planRevision + 1), 'stale-revision')
  await svc.approve(r.id, r.planRevision)
  const a = svc.get(r.id)!
  assert.equal(a.phase, 'approved')
  assert.equal(a.approvedPlanRevision, 1)
  assert.ok(!fs.existsSync(a.snapshotDir), 'snapshot removed after approval')
  await svc.logDispatch(r.id, 't1', 'Codex #1')
  const text = shared.taskDispatchText(a, shared.currentBoard(a)!.tasks[1])
  assert.ok(text.includes('src/b.ts') && text.includes('t1') && text.includes('b uses the helper'))
  assert.ok(fs.readdirSync(path.join(dataDir)).length === 1)
  // 磁碟上的 manifest 與記憶體一致
  const onDisk = JSON.parse(fs.readFileSync(path.join(dataDir, fs.readdirSync(dataDir)[0], r.id, 'run.json'), 'utf8'))
  assert.equal(onDisk.phase, 'approved')
  console.log('e2e happy path ok')
}

// 2. 單一覆核者的反對不能被繞過：主席沒處置 → 修正一次仍不處置 → blocked，不能核准
{
  behave({ r2Objection: true, r34OmitDecisions: true })
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'blocked')
  assert.equal(r.block.kind, 'step-failed')
  assert.match(r.block.message, /still invalid/)
  assert.equal(r.budget.planningCallsUsed, 4, 'R1 + R2 + R34 + one repair')
  assert.equal(r.boards.length, 0)
  await code(async () => await svc.approve(r.id, r.planRevision), 'not-allowed')
  // 改好之後重試：通過
  behave({ r2Objection: true })
  await svc.retry(r.id)
  const r2 = await waitFor(r.id, settled)
  assert.equal(r2.phase, 'awaiting-approval')
  await svc.cancel(r.id)
  console.log('e2e objection cannot be bypassed ok')
}

// 3. 輸出不合格 → 一次修正就過
{
  behave({ invalidOnce: true })
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
  assert.ok(r.calls.some((c: any) => c.repair && c.step === 'r1' && c.status === 'ok'))
  await svc.cancel(r.id)
  console.log('e2e repair ok')
}

// 4. 覆核者失敗：不能當成同意；唯一的覆核者失敗時不能「少一位繼續」
{
  behave({ failAgent: 'codex' })
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'blocked')
  assert.equal(r.block.kind, 'reviewers-failed')
  assert.equal(r.reviewers.codex.status, 'failed')
  await code(svc.dropFailedReviewers(r.id), 'no-reviewer-left')
  behave({})
  await svc.retry(r.id)
  const r2 = await waitFor(r.id, settled)
  assert.equal(r2.phase, 'awaiting-approval')
  await svc.cancel(r.id)
  console.log('e2e reviewer failure ok')
}

// 5. 主席換成 codex、唯一的覆核者 claude 失敗：一樣不能「少一位繼續」把會議變成主席一人說了算
{
  behave({ failAgent: 'claude', failStep: 'R2' })
  const run0 = await svc.start({ ...base, chair: 'codex' })
  const r = await waitFor(run0.id, settled)
  assert.equal(r.block.kind, 'reviewers-failed')
  await code(svc.dropFailedReviewers(r.id), 'no-reviewer-left')
  await svc.cancel(r.id)
  console.log('e2e drop guard ok')
}

// 6. 副作用：CLI 在規劃時寫檔 → 這次規劃作廢、保留 diff；重試時重建乾淨快照
{
  behave({ writeFile: 'claude' })
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'blocked')
  assert.equal(r.block.kind, 'side-effects')
  assert.ok(r.block.details.some((l: string) => l.includes('pwned.txt')))
  assert.equal(r.r1, null, 'the tainted R1 is not used')
  behave({})
  await svc.retry(r.id)
  const r2 = await waitFor(r.id, settled)
  assert.equal(r2.phase, 'awaiting-approval', JSON.stringify(r2.block))
  assert.ok(!fs.existsSync(path.join(r2.snapshotDir, 'pwned.txt')))
  await svc.cancel(r.id)
  console.log('e2e side effects ok')
}

// 7. 預算：呼叫數用完就停在 blocked(budget)；使用者明確提高上限後重試
{
  behave({})
  const run0 = await svc.start({ ...base, limits: { ...base.limits, maxPlanningCalls: 2 } })
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'blocked')
  assert.equal(r.block.kind, 'budget')
  assert.equal(r.budget.planningCallsUsed, 2)
  await svc.raiseLimits(r.id, { maxPlanningCalls: 1 })
  assert.equal(svc.get(r.id)!.limits.maxPlanningCalls, 2, 'limits never shrink')
  await svc.raiseLimits(r.id, { maxPlanningCalls: 3 })
  await svc.retry(r.id)
  const r2 = await waitFor(r.id, settled)
  assert.equal(r2.phase, 'awaiting-approval')
  await svc.cancel(r.id)
  console.log('e2e budget ok')
}

// 8. 取消：行程被終止、狀態 cancelled、快照收掉
{
  behave({ slowMs: 20000 })
  const run0 = await svc.start(base)
  await waitFor(run0.id, (r) => r.calls.some((c: any) => c.status === 'running'))
  const t0 = Date.now()
  await svc.cancel(run0.id)
  const r = svc.get(run0.id)!
  assert.equal(r.phase, 'cancelled')
  assert.ok(Date.now() - t0 < 10000, 'cancel does not wait for the slow call')
  await new Promise((res) => setTimeout(res, 300))
  assert.equal(r.calls[0].status, 'cancelled')
  assert.ok(!fs.existsSync(r.snapshotDir))
  assert.equal(r.phase, 'cancelled', 'a late result cannot revive a cancelled run')
  console.log('e2e cancel ok')
}

// 9. 改板：舊版本拒絕、無效 DAG 拒絕、成功則 revision 加一；核准綁定版本
{
  behave({})
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'awaiting-approval')
  const tasks = shared.currentBoard(r)!.tasks
  await code(async () => await svc.editBoard(r.id, 99, tasks), 'stale-revision')
  const bad = await svc.editBoard(r.id, 1, [{ ...tasks[0], dependsOn: ['t2'] }, tasks[1]])
  assert.ok(!bad.ok && bad.errors.some((e) => /cycle/.test(e)))
  const evil = await svc.editBoard(r.id, 1, [{ ...tasks[0], scope: ['../../etc'] }])
  assert.ok(!evil.ok)
  assert.equal(svc.get(r.id)!.planRevision, 1, 'rejected edits change nothing')
  const good = await svc.editBoard(r.id, 1, [{ ...tasks[0], title: 'Renamed' }])
  assert.ok(good.ok)
  const r2 = svc.get(r.id)!
  assert.equal(r2.planRevision, 2)
  assert.equal(shared.currentBoard(r2)!.source, 'user')
  await code(async () => await svc.approve(r.id, 1), 'stale-revision')
  await svc.approve(r.id, 2)
  // 核准後回饋：重開一版要重新核准（快照自動重建）
  behave({})
  await svc.feedback(r.id, 'please drop t2')
  const r3 = await waitFor(r.id, (x) => x.phase !== 'meeting')
  assert.equal(r3.phase, 'awaiting-approval')
  assert.equal(r3.planRevision, 3)
  assert.equal(r3.approvedPlanRevision, 2, 'the old approval stays on record')
  await svc.approve(r.id, 3)
  console.log('e2e edit & revisions ok')
}

// 10. 待定項目：不能核准；回饋讓主席處置後才進入待核准；或使用者明確放下
{
  behave({ r2Objection: true, r34Unresolved: true })
  const run0 = await svc.start(base)
  const r = await waitFor(run0.id, settled)
  assert.equal(r.phase, 'blocked')
  assert.equal(r.block.kind, 'unresolved')
  await code(async () => await svc.approve(r.id, r.planRevision), 'not-allowed')
  await code(svc.retry(r.id), 'not-retryable')
  behave({})
  await svc.feedback(r.id, 'go with a.ts')
  const r2 = await waitFor(r.id, settled)
  assert.equal(r2.phase, 'awaiting-approval', JSON.stringify(r2.block))
  assert.equal(shared.currentBoard(r2)!.unresolved.length, 0)
  assert.ok(shared.currentBoard(r2)!.decisions.some((d: any) => d.reason === 'user said so'))
  await svc.cancel(r.id)

  behave({ r2Objection: true, r34Unresolved: true })
  const run1 = await svc.start(base)
  const s = await waitFor(run1.id, settled)
  assert.equal(s.block.kind, 'unresolved')
  await svc.dismissUnresolved(s.id)
  const s2 = svc.get(s.id)!
  assert.equal(s2.phase, 'awaiting-approval')
  assert.equal(s2.planRevision, 2)
  await svc.cancel(s.id)
  console.log('e2e unresolved ok')
}

// 11. 重啟：會議進行中 app 結束 → 下次載入是 paused，可以續跑
{
  behave({ slowMs: 20000 })
  const run0 = await svc.start(base)
  await waitFor(run0.id, (r) => r.calls.some((c: any) => c.status === 'running'))
  await svc.shutdown()
  await new Promise((res) => setTimeout(res, 800))
  const svc2 = new CoworkService(deps)
  await svc2.init()
  const r = svc2.get(run0.id)!
  assert.equal(r.phase, 'paused')
  assert.equal(r.block?.kind, 'restart')
  assert.ok(r.calls.every((c) => c.status !== 'running'))
  behave({})
  await svc2.retry(r.id)
  const t0 = Date.now()
  while (svc2.get(r.id)!.phase === 'meeting' && Date.now() - t0 < 30000) await new Promise((res) => setTimeout(res, 50))
  assert.equal(svc2.get(r.id)!.phase, 'awaiting-approval')
  await svc2.cancel(r.id)
  console.log('e2e restart ok')
}

// 12. 輸入檢查與語言
{
  await code(svc.start({ ...base, prompt: '   ' } as any), 'empty-prompt')
  await code(svc.start({ ...base, participants: ['claude'] } as any), 'need-two-participants')
  await code(svc.start({ ...base, chair: 'codex', participants: ['claude', 'antigravity'] } as any), 'chair-not-participant')
  ineligible.add('codex')
  await code(svc.start(base), 'agent-not-eligible')
  ineligible.clear()
  await code(svc.start({ ...base, prompt: 'x', workspace: tmp } as any), 'not-a-git-repo')
}

// 13–15 用新的 service（11 把 svc 關了）
const svc3 = new CoworkService(deps)
await svc3.init()
async function settle3(id: string): Promise<any> {
  const t0 = Date.now()
  while (svc3.get(id)!.phase === 'meeting' && Date.now() - t0 < 30000) await new Promise((r) => setTimeout(r, 50))
  return svc3.get(id)!
}
const three = { ...base, participants: ['claude', 'codex', 'antigravity'] as any }

// 13. 三位與會者：agy 以隔離家目錄執行、prompt 走 stream-json，反對意見照樣要處置
{
  behave({ r2Objection: true, expectHome: agyHome })
  const run0 = await svc3.start(three)
  const r = await settle3(run0.id)
  assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
  assert.equal(r.reviewers.antigravity.status, 'ok')
  const ids = shared.currentBoard(r)!.decisions.map((d: any) => d.issueId)
  assert.ok(ids.includes('antigravity.o1') && ids.includes('codex.o1') && ids.includes('antigravity.m1'))
  assert.equal(r.budget.planningCallsUsed, 4, 'N+1 calls for three agents')
  const agCall = r.calls.find((c: any) => c.agent === 'antigravity')
  assert.ok(agCall.timeoutMs >= 600_000 && agCall.usage.inputTokens === 900)
  await svc3.cancel(r.id)
  console.log('e2e antigravity reviewer ok')
}

// 14. agy 用了白名單外的工具：立刻終止，不能等它跑完；可以明確選擇不等它
{
  behave({ r2Objection: true, agyBadTool: 'schedule' })
  const run0 = await svc3.start(three)
  const t0 = Date.now()
  const r = await settle3(run0.id)
  assert.ok(Date.now() - t0 < 20000, 'the bad tool is killed, not waited out')
  assert.equal(r.block.kind, 'reviewers-failed')
  assert.match(r.reviewers.antigravity.error, /schedule/)
  assert.equal(r.reviewers.codex.status, 'ok')
  behave({ r2Objection: true })
  await svc3.dropFailedReviewers(r.id)
  const r2 = await settle3(r.id)
  assert.equal(r2.phase, 'awaiting-approval')
  assert.equal(r2.planRevision, 2)
  assert.ok(!shared.currentBoard(r2)!.decisions.some((d: any) => d.issueId.startsWith('antigravity.')))
  await svc3.cancel(r.id)
  console.log('e2e antigravity tool guard ok')
}

// 16. 模型與強度：會議指定的值傳到 CLI；codex 沒指定時沿用使用者 config.toml；開會後改設定不影響這場
{
  const log = path.join(tmp, 'argv.log')
  behave({ logFile: log })
  const models = { claude: { model: 'opus', effort: 'high' }, codex: { model: '', effort: '' }, antigravity: { model: 'gemini-x', effort: '' } }
  const run0 = await svc3.start({ ...three, models } as any)
  assert.deepEqual(run0.models, { claude: { model: 'opus', effort: 'high' }, antigravity: { model: 'gemini-x', effort: '' } })
  const r = await settle3(run0.id)
  assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
  const calls = fs.readFileSync(log, 'utf8').trim().split('\n').map((l) => JSON.parse(l))
  const argsOf = (me: string) => calls.filter((c) => c.me === me).map((c) => c.argv.join(' '))
  assert.ok(argsOf('claude').every((a) => a.includes('--model opus --effort high')))
  assert.ok(argsOf('codex').every((a) => a.includes('-m gpt-user -c model_reasoning_effort="low"')), 'codex falls back to the user config')
  assert.ok(argsOf('antigravity').every((a) => a.includes('--model gemini-x') && !a.includes('--effort')), 'a chosen agy model carries its own effort')
  const byAgent = (a: string) => r.calls.find((c: any) => c.agent === a)
  assert.equal(byAgent('claude').model, 'opus')
  assert.equal(byAgent('codex').model, 'gpt-user')
  assert.equal(byAgent('codex').effort, 'low')
  assert.equal(byAgent('antigravity').model, 'gemini-x')
  assert.equal(byAgent('antigravity').effort, undefined)
  // 磁碟上的 run 也記住了這場的選擇；舊 run 沒有 models 欄位也能讀
  const file = path.join(dataDir, fs.readdirSync(dataDir).find((d) => fs.existsSync(path.join(dataDir, d, r.id)))!, r.id, 'run.json')
  assert.deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')).models, run0.models)
  await svc3.cancel(r.id)
  const legacy = JSON.parse(fs.readFileSync(file, 'utf8'))
  delete legacy.models
  fs.writeFileSync(file, JSON.stringify(legacy))
  const svc4 = new CoworkService(deps)
  await svc4.init()
  assert.deepEqual(svc4.get(r.id)!.models, {})
  console.log('e2e models ok')
}

// 15. agy 因權限中止而沒有輸出：當成失敗，訊息說明原因
{
  behave({ agyNoOutput: true })
  const run0 = await svc3.start(three)
  const r = await settle3(run0.id)
  assert.equal(r.block.kind, 'reviewers-failed')
  assert.match(r.reviewers.antigravity.error, /permission/)
  await svc3.cancel(r.id)
  console.log('e2e antigravity no output ok')
}

// ── 背景執行（§12.5）：worktree、依序／同時、追問接續、失敗重試、停止、重啟、合併、清理 ──
{
  const erepo = makeRepo('repo-exec')
  fs.writeFileSync(path.join(erepo, 'CLAUDE.md'), 'PROJECT RULE: keep it small\n')
  g(erepo, 'add', '-A')
  g(erepo, 'commit', '-qm', 'instructions')
  const log = path.join(tmp, 'exec-argv.log')
  const execDeps = {
    ...deps,
    // 執行用使用者平常的 CLI：agy 不帶隔離家目錄
    resolveExecCli: (agent: string) => (ineligible.has(agent) ? { error: 'not available' } : { command: process.execPath, prefixArgs: [fake] })
  }
  const svc5 = new CoworkService(execDeps)
  await svc5.init()
  const wait5 = async (id: string, cond: (r: any) => boolean, ms = 30000): Promise<any> => {
    const t0 = Date.now()
    for (;;) {
      const r = svc5.get(id)!
      if (cond(r)) return r
      if (Date.now() - t0 > ms) throw new Error(`timeout; phase=${r.phase} tasks=${JSON.stringify(r.execution?.tasks)}`)
      await new Promise((res) => setTimeout(res, 50))
    }
  }
  const meeting = async (extra: Record<string, unknown> = {}): Promise<any> => {
    const run0 = await svc5.start({ ...three, workspace: erepo, ...extra } as any)
    const r = await wait5(run0.id, (x) => x.phase !== 'meeting')
    assert.equal(r.phase, 'awaiting-approval', JSON.stringify(r.block))
    await svc5.approve(r.id, r.planRevision)
    return r
  }
  const calls = () => fs.readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l))
  // 使用者看過 preview 才開始；這裡的舊流程不測審查（check-cowork-gates.mts 測）
  const startExec = async (id: string, o: { mode: any; linkDeps: boolean; bypass: boolean }): Promise<void> => {
    const p = await svc5.execPreview(id, o)
    await svc5.execStart(id, { ...o, approval: p.hash, review: false })
  }
  const approvalOf = (id: string, waive?: string): any => {
    const ex = svc5.get(id)!.execution!
    return { commit: ex.integration?.commit || '', targetHead: ex.targetHead || '', ...(waive ? { waive } : {}) }
  }

  // 規劃參考資料：基線的 CLAUDE.md 與勾選的 skill 進了 prompt
  behave({ logFile: log })
  const skills = [{ key: 'my-skill', name: 'My Skill', content: 'SKILL BODY' }]
  const r = await meeting({ skills })
  assert.deepEqual(r.context.instructions, [{ file: 'CLAUDE.md', content: 'PROJECT RULE: keep it small\n' }])
  assert.equal(r.context.skills[0].name, 'My Skill')
  const r1call = calls().find((c) => c.step === 'R1')
  assert.ok(r1call.head.includes('PROJECT RULE') && r1call.head.includes('--- skill: My Skill ---') && r1call.head.includes('SKILL BODY'))

  // 依序：一個 worktree，t1（claude）→ t2（agy）；各一個 commit；主工作區保持乾淨
  fs.writeFileSync(log, '')
  await startExec(r.id, { mode: 'sequential', linkDeps: true, bypass: true })
  await code(() => svc5.cancel(r.id), 'not-allowed')
  let x = await wait5(r.id, (y) => y.phase === 'review')
  const wt = x.execution.worktrees.main
  assert.equal(path.resolve(wt.path), path.resolve(erepo, '.cowork', r.id, 'main'))
  assert.equal(wt.branch, `cowork/${r.id}/main`)
  assert.ok(fs.existsSync(path.join(wt.path, 't1.txt')) && fs.existsSync(path.join(wt.path, 't2.txt')))
  assert.equal(x.execution.tasks.t1.status, 'done')
  assert.equal(x.execution.tasks.t1.commits.length, 1)
  assert.equal(x.execution.tasks.t1.sessionId, 'sess-claude-t1')
  assert.equal(x.execution.tasks.t2.agent, 'antigravity')
  assert.equal(x.execution.integration.status, 'ok')
  assert.match(x.execution.integration.stat, /t1\.txt/)
  assert.equal(g(erepo, 'status', '--porcelain').trim(), '', '.cowork/ is excluded from the main worktree status')
  assert.match(g(erepo, 'log', '--format=%s', wt.branch), /t2 Use the helper[\s\S]*t1 Add the helper/)
  const ex1 = calls()
  assert.ok(ex1.find((c) => c.me === 'claude').argv.join(' ').includes('--permission-mode bypassPermissions'))
  const agyExec = ex1.find((c) => c.me === 'antigravity')
  assert.ok(agyExec.argv.includes('--dangerously-skip-permissions'))
  assert.ok(agyExec.head.includes('Prerequisite tasks') && agyExec.head.includes('Done t1.'), 'dependent task sees the prerequisite reply')
  const agentTurn = x.execution.tasks.t2.turns.find((t: any) => t.role === 'agent')
  assert.equal(shared.stripResultBlock(agentTurn.text), 'Done t2.')
  assert.ok(agentTurn.progress[0].startsWith('write_to_file t2.txt'), agentTurn.progress[0])

  // 追問：接續同一個 agy conversation，改動另成一個 commit，整合重算
  fs.writeFileSync(log, '')
  await code(svc5.execMessage(r.id, 't2', '   '), 'empty-feedback')
  await svc5.execMessage(r.id, 't2', 'also add a note')
  // 追問寫的 followup.txt 不在 t2 的 scope：先不 commit，等使用者決定
  x = await wait5(r.id, (y) => y.execution.tasks.t2.status === 'blocked')
  assert.deepEqual(x.execution.tasks.t2.scopeReview.files, ['followup.txt'])
  assert.equal(x.execution.tasks.t2.scopeReview.followUp, true)
  assert.equal(x.execution.tasks.t2.commits.length, 1, 'out-of-scope changes are not committed')
  assert.ok(x.log.some((l: any) => l.t === 'task-scope' && l.taskId === 't2'))
  await code(svc5.execAcceptScope(r.id, 't1'), 'not-allowed')
  await code(svc5.execAcceptScope(r.id, 't2'), 'stale')
  await svc5.execAcceptScope(r.id, 't2', svc5.get(r.id)!.execution!.tasks.t2.scopeReview!.hash)
  x = await wait5(r.id, (y) => y.phase === 'review' && y.execution.tasks.t2.commits.length === 2)
  assert.equal(x.execution.tasks.t2.scopeReview, undefined)
  assert.ok(x.execution.tasks.t2.changedFiles.includes('followup.txt'))
  assert.match(g(wt.path, 'log', '-1', '--format=%s'), /\(follow-up\)/)
  const resumed = calls().find((c) => c.me === 'antigravity')
  assert.equal(resumed.argv[resumed.argv.indexOf('--conversation') + 1], 'sess-antigravity-t2')
  assert.ok(fs.existsSync(path.join(wt.path, 'followup.txt')))
  assert.equal(x.execution.integration.commit, g(wt.path, 'rev-parse', 'HEAD').trim())

  // 有 worktree 不能刪紀錄；合併回來源分支；清理後 worktree 不在、分支還在
  await code(async () => await svc5.delete(r.id), 'has-worktrees')
  // 已追蹤的檔案有改動就不合併；未追蹤的（例如 app 的 .workbench/）不擋
  fs.writeFileSync(path.join(erepo, 'CLAUDE.md'), 'changed\n')
  await code(svc5.execMerge(r.id), 'stale')
  await code(svc5.execMerge(r.id, approvalOf(r.id)), 'merge-dirty')
  g(erepo, 'checkout', '--', 'CLAUDE.md')
  fs.mkdirSync(path.join(erepo, '.workbench'))
  fs.writeFileSync(path.join(erepo, '.workbench', 'state.json'), '{}')
  await svc5.execMerge(r.id, approvalOf(r.id))
  x = svc5.get(r.id)!
  assert.equal(x.phase, 'completed')
  assert.ok(fs.existsSync(path.join(erepo, 't1.txt')) && fs.existsSync(path.join(erepo, 'followup.txt')))
  await svc5.execCleanup(r.id)
  assert.ok(!fs.existsSync(path.join(erepo, '.cowork')))
  assert.ok(g(erepo, 'branch', '--list', `cowork/${r.id}/main`).trim(), 'branch kept after cleanup')
  await svc5.delete(r.id)
  console.log('e2e exec sequential ok')

  // 同時：每家一個 worktree；t1 失敗 → t2 等著；重試後 t2 先套用 t1 的 commit；最後整合成一條分支
  behave({ execFail: 't1' })
  const p = await meeting()
  await startExec(p.id, { mode: 'parallel', linkDeps: false, bypass: false })
  x = await wait5(p.id, (y) => y.execution.tasks.t1.status === 'failed')
  assert.equal(x.execution.tasks.t2.status, 'pending')
  assert.equal(x.phase, 'executing')
  assert.deepEqual(Object.keys(x.execution.worktrees).sort(), ['antigravity', 'claude'])
  await code(svc5.execMessage(p.id, 't2', 'hi'), 'busy')
  behave({ logFile: log })
  fs.writeFileSync(log, '')
  await svc5.execRetry(p.id, 't1')
  x = await wait5(p.id, (y) => y.phase === 'review')
  assert.ok(fs.existsSync(path.join(x.execution.worktrees.antigravity.path, 't1.txt')), 't1 commit applied before t2')
  assert.equal(x.execution.integration.status, 'ok')
  assert.equal(x.execution.integration.branch, `cowork/${p.id}/integration`)
  const intDir = x.execution.worktrees.integration.path
  assert.ok(fs.existsSync(path.join(intDir, 't1.txt')) && fs.existsSync(path.join(intDir, 't2.txt')))
  // 沒開 Bypass：claude 只自動接受改檔
  assert.ok(calls().find((c) => c.me === 'claude').argv.join(' ').includes('--permission-mode acceptEdits'))
  await svc5.execCleanup(p.id)
  assert.equal(svc5.get(p.id)!.phase, 'completed')
  console.log('e2e exec parallel ok')

  // 停止與重啟：跑到一半停止 → 任務標成被中斷；app 重啟時還在跑的任務也一樣，等使用者繼續
  behave({ execSlowMs: 4000 })
  const s = await meeting()
  await startExec(s.id, { mode: 'sequential', linkDeps: false, bypass: true })
  await wait5(s.id, (y) => y.execution.tasks.t1.status === 'running')
  await svc5.execPause(s.id)
  x = await wait5(s.id, (y) => y.execution.tasks.t1.status === 'cancelled')
  assert.equal(x.execution.paused, true)
  const runFile = path.join(dataDir, fs.readdirSync(dataDir).find((d) => fs.existsSync(path.join(dataDir, d, s.id)))!, s.id, 'run.json')
  const onDisk = JSON.parse(fs.readFileSync(runFile, 'utf8'))
  onDisk.execution.tasks.t1.status = 'running'
  onDisk.execution.paused = false
  fs.writeFileSync(runFile, JSON.stringify(onDisk))
  const svc6 = new CoworkService(execDeps)
  await svc6.init()
  const rec = svc6.get(s.id)!
  assert.equal(rec.execution!.tasks.t1.status, 'cancelled')
  assert.equal(rec.execution!.pausedReason, 'restart')
  behave({})
  await svc5.execResume(s.id)
  x = await wait5(s.id, (y) => y.phase === 'review')
  assert.equal(x.execution.tasks.t2.status, 'done')
  await svc5.execCleanup(s.id)
  console.log('e2e exec pause/restart ok')

  // Independent tasks still serialize an explicitly shared resource across agent worktrees.
  behave({ execSlowMs: 800 })
  const resourceRun = await svc5.start({ ...three, workspace: erepo })
  const ready = await wait5(resourceRun.id, (y) => y.phase === 'awaiting-approval')
  const independent = shared.currentBoard(ready)!.tasks.map((t) => ({ ...t, dependsOn: [], resources: ['port:5173'] }))
  await svc5.editBoard(ready.id, ready.planRevision, independent)
  await svc5.approve(ready.id, svc5.get(ready.id)!.planRevision)
  const readyPreview = await svc5.execPreview(ready.id, { mode: 'parallel', linkDeps: false, bypass: false })
  const starting = svc5.execStart(ready.id, { mode: 'parallel', linkDeps: false, bypass: false, approval: readyPreview.hash, review: false })
  await code(svc5.execStart(ready.id, { mode: 'parallel', linkDeps: false, bypass: false, approval: readyPreview.hash }), 'active-run-exists')
  await starting
  const first = await wait5(ready.id, (y) => y.execution.tasks.t1.status === 'running')
  assert.equal(first.execution.tasks.t2.status, 'pending', 'shared port is not used concurrently')
  const finished = await wait5(ready.id, (y) => y.phase === 'review')
  assert.ok(finished.execution.tasks.t2.startedAt >= finished.execution.tasks.t1.endedAt)
  // Planning another meeting is allowed; starting another execution before cleanup is not.
  behave({})
  const secondProject = await meeting()
  await code(svc5.execStart(secondProject.id, { mode: 'sequential', linkDeps: false, bypass: false, approval: (await svc5.execPreview(secondProject.id, { mode: 'sequential', linkDeps: false, bypass: false })).hash }), 'active-run-exists')
  await svc5.execCleanup(ready.id)
  console.log('e2e exclusive resources serialize independent tasks ok')

  // 越界改動還原 + 結構化交接 + 會改依賴的任務不共用 node_modules
  await code(svc5.execRevertScope(secondProject.id, 't1'), 'task-not-found')
  behave({ execStray: true, execResult: true, logFile: log })
  const sr0 = await svc5.start({ ...three, workspace: erepo } as any)
  const sready = await wait5(sr0.id, (y) => y.phase === 'awaiting-approval')
  assert.equal(shared.currentBoard(sready)!.tasks[0].assignmentReason, 'Owns the helper module')
  assert.deepEqual(sready.r1.assumptions, [{ agent: 'claude', text: 'good at small modules' }])
  const withDeps = shared.currentBoard(sready)!.tasks.map((t) => (t.id === 't1' ? { ...t, scope: [...t.scope, 'package.json'] } : t))
  assert.ok((await svc5.editBoard(sready.id, sready.planRevision, withDeps)).ok)
  const sedit = svc5.get(sready.id)!
  assert.equal(shared.currentBoard(sedit)!.tasks[0].assignmentReason, 'Owns the helper module', 'editing keeps the reason when the assignee is unchanged')
  await svc5.approve(sedit.id, sedit.planRevision)
  fs.writeFileSync(log, '')
  await startExec(sedit.id, { mode: 'sequential', linkDeps: true, bypass: true })
  x = await wait5(sedit.id, (y) => y.execution.tasks.t1.status === 'blocked')
  assert.deepEqual(x.execution.depsLinkSkippedFor, ['t1'])
  assert.equal(x.execution.worktrees.main.depsLinked, false)
  assert.deepEqual(x.execution.tasks.t1.scopeReview.files, ['stray.txt'])
  assert.equal(x.execution.tasks.t2.status, 'pending', 'dependents wait while a task is held for scope review')
  assert.deepEqual(x.execution.tasks.t1.result, { interfaces: ['t1.txt holds the marker'], verified: ['npm test -> pass'], unverified: ['Windows paths'], notes: 'note from t1', acceptance: [{ item: 1, status: 'done', evidence: 'fake evidence' }] })
  const swt = x.execution.worktrees.main.path
  assert.ok(calls().find((c) => c.me === 'claude').head.includes('NOT installed'), 'agent is told dependencies are not installed')
  await svc5.execRevertScope(sedit.id, 't1')
  x = await wait5(sedit.id, (y) => y.execution.tasks.t1.status === 'done')
  assert.ok(!fs.existsSync(path.join(swt, 'stray.txt')), 'reverted file removed')
  assert.ok(fs.existsSync(path.join(swt, 't1.txt')), 'in-scope change kept')
  assert.deepEqual(x.execution.tasks.t1.changedFiles, ['t1.txt'])
  assert.ok(!g(swt, 'show', '--name-only', '--format=', 'HEAD').includes('stray.txt'))
  // t2 也寫了 stray.txt：一樣停下；它的交辦裡有 t1 的結構化交接
  x = await wait5(sedit.id, (y) => y.execution.tasks.t2.status === 'blocked')
  const t2brief = calls().find((c) => c.me === 'antigravity').head
  assert.ok(t2brief.includes('changed files (from git): t1.txt'), t2brief)
  assert.ok(t2brief.includes('t1.txt holds the marker') && t2brief.includes('NOT verified') && t2brief.includes('Windows paths'))
  assert.ok(!t2brief.includes('"t1.txt holds the marker"'), 'raw result block is not repeated in the excerpt')
  await svc5.execRevertScope(sedit.id, 't2')
  x = await wait5(sedit.id, (y) => y.phase === 'review')
  assert.equal(x.execution.integration.status, 'ok')
  await svc5.execCleanup(sedit.id)
  console.log('e2e scope review, structured handoff and dependency linking ok')
}

// Discussion: real sequential context, no snapshot or project instructions, retry/skip, next round and budget.
{
  const discussionDeps = { ...deps, dataDir: path.join(tmp, "discussion-data") }
  const discussionSvc = new CoworkService(discussionDeps)
  await discussionSvc.init()
  const wait = (id: string, cond: (r: any) => boolean) => waitFor(id, cond, 30000, discussionSvc)
  behave({})
  const plain = path.join(tmp, 'discussion-folder')
  fs.mkdirSync(plain)
  const before = await gitm.git(repo, ['worktree', 'list', '--porcelain'])
  const opts = { ...base, workspace: plain, mode: 'discussion' as const, participants: ['codex', 'claude', 'antigravity'] as any, chair: 'codex' as const, autoEffort: true, skills: [{ key: 'secret', name: 'Secret', content: 'PROJECT_ONLY' }] }
  const run0 = await discussionSvc.start(opts)
  const r = await wait(run0.id, settled)
  assert.equal(r.phase, 'completed', JSON.stringify(r.block))
  assert.equal(r.context, undefined)
  assert.equal(r.repo.baseCommit, '')
  assert.ok(!fs.existsSync(path.join(r.snapshotDir, '.git')))
  assert.equal(await gitm.git(repo, ['worktree', 'list', '--porcelain']), before)
  assert.deepEqual(r.discussion.messages.map((m: any) => m.agent), [null, 'codex', 'claude', 'antigravity'])
  assert.deepEqual(r.discussion.messages.slice(1).map((m: any) => m.replyTo), ['m1', 'm2', 'm3'])
  assert.match(r.discussion.messages[2].message, /codex replies/)
  assert.equal(r.calls.length, 3)
  assert.ok(r.calls.every((c: any) => c.effort === 'low' && c.spawnedAt >= c.startedAt && c.firstOutputAt >= c.spawnedAt && c.endedAt >= c.firstOutputAt))
  assert.ok((await discussionSvc.list(plain)).some((s) => s.id === r.id))
  const prompt = shared.buildDiscussionPrompt(r, 'codex')
  assert.ok(!prompt.includes('PROJECT_ONLY') && !prompt.includes('CLAUDE.md'))
  assert.equal(shared.checkDiscussion({ message: 'hi', replyTo: 'fake' }, r.discussion.messages).ok, false)
  await discussionSvc.discuss(r.id, 'Now respond to the previous speaker')
  const round2 = await wait(r.id, settled)
  assert.equal(round2.discussion.messages.length, 8)
  assert.equal(round2.calls.length, 6)
  await discussionSvc.discuss(r.id, 'Third round')
  const blocked = await wait(r.id, settled)
  assert.equal(blocked.block.kind, 'budget')
  await discussionSvc.raiseLimits(r.id, { maxPlanningCalls: 9 })
  await discussionSvc.retry(r.id)
  assert.equal((await wait(r.id, settled)).phase, 'completed')
  const restored = new CoworkService(discussionDeps)
  await restored.init()
  assert.equal(restored.get(r.id)?.discussion?.messages.length, 12)
  await restored.shutdown()
  await discussionSvc.delete(r.id)

  behave({ failAgent: 'claude', failStep: 'Discussion' })
  const fail = await discussionSvc.start(opts)
  const f = await wait(fail.id, settled)
  assert.equal(f.phase, 'blocked')
  assert.equal(f.discussion.messages.length, 2, 'successful chair reply survives a later failure')
  behave({})
  await discussionSvc.retry(f.id)
  const retried = await wait(f.id, settled)
  assert.equal(retried.phase, 'completed')
  assert.equal(retried.calls.filter((c: any) => c.agent === 'codex').length, 1, 'retry does not repeat earlier speakers')
  await discussionSvc.delete(f.id)

  behave({ failAgent: 'codex', failStep: 'Discussion' })
  const skip = await discussionSvc.start(opts)
  await wait(skip.id, settled)
  await discussionSvc.dropFailedReviewers(skip.id)
  const skipped = await wait(skip.id, settled)
  assert.equal(skipped.phase, 'completed')
  assert.deepEqual(skipped.discussion.excluded, ['codex'])
  assert.deepEqual(skipped.discussion.messages.map((m: any) => m.agent), [null, 'claude', 'antigravity'])
  behave({})
  await discussionSvc.discuss(skip.id, 'Keep going')
  assert.deepEqual((await wait(skip.id, settled)).discussion.order, ['claude', 'antigravity'])
  await discussionSvc.delete(skip.id)
  console.log('discussion context, isolated workspace, timings, rounds, recovery, retry, skip and budget ok')

  // Recorder checkpoint -> chair recommendation, with no implicit project approval/execution.
  behave({})
  const recorded = await discussionSvc.start({ ...opts, summarizer: 'claude', summarizeEachRound: true })
  const rec = await wait(recorded.id, settled)
  assert.equal(rec.calls.length, 4)
  assert.equal(rec.calls.at(-1).agent, 'claude')
  assert.equal(rec.calls.at(-1).step, 'summary')
  assert.equal(rec.calls.at(-1).effort, 'medium')
  assert.equal(rec.discussion.summaries[0].through, 4)
  assert.deepEqual(rec.discussion.summaries[0].questions, ['User must approve execution'])
  const memory = shared.buildDiscussionPrompt(rec, 'codex')
  assert.match(memory, /EARLY_REQUIREMENT/)
  assert.ok(!memory.includes('"speaker":'), 'summarized messages are replaced by the record, not repeated')
  await discussionSvc.summarizeDiscussion(rec.id, true)
  const concluded = await wait(rec.id, settled)
  assert.equal(concluded.calls.length, 5, 'a fresh summary is reused for the chair call')
  assert.equal(concluded.calls.at(-1).agent, 'codex')
  assert.equal(concluded.calls.at(-1).effort, 'high')
  assert.match(concluded.discussion.conclusion.message, /approval/)
  assert.equal(concluded.approvedPlanRevision, null)
  assert.equal(concluded.execution, undefined)
  assert.match(shared.discussionProjectPrompt(concluded), /EARLY_REQUIREMENT/)
  const saved = new CoworkService(discussionDeps)
  await saved.init()
  assert.deepEqual(saved.get(rec.id)?.discussion?.summaries, rec.discussion.summaries)
  assert.equal(saved.get(rec.id)?.discussion?.conclusion?.through, 4)
  await saved.shutdown()
  await discussionSvc.discuss(rec.id, 'New direction after conclusion')
  const capped = await wait(rec.id, settled)
  assert.equal(capped.block.kind, 'budget')
  assert.ok(capped.discussion.conclusion.through < capped.discussion.messages.length, 'new discussion makes the old conclusion stale')
  assert.equal(capped.discussion.summaries.length, 1, 'budget failure cannot advance summary coverage')
  await discussionSvc.delete(rec.id)

  behave({})
  const failure = await discussionSvc.start({ ...opts, summarizer: 'claude' })
  await wait(failure.id, settled)
  behave({ failAgent: 'claude', failStep: 'Summary' })
  await discussionSvc.summarizeDiscussion(failure.id, true)
  const badSummary = await wait(failure.id, settled)
  assert.equal(badSummary.pending.step, 'summary')
  assert.equal(badSummary.discussion.summaries.length, 0)
  assert.equal(badSummary.discussion.conclusion, undefined)
  await code(discussionSvc.dropFailedReviewers(failure.id), 'not-allowed')
  await discussionSvc.setSummarizer(failure.id, 'antigravity')
  behave({})
  await discussionSvc.retry(failure.id)
  const fixed = await wait(failure.id, settled)
  assert.equal(fixed.phase, 'completed')
  assert.equal(fixed.discussion.summaries[0].agent, 'antigravity')
  assert.equal(fixed.calls.filter((c: any) => c.step === 'discussion').length, 3, 'retry does not repeat the discussion')
  await discussionSvc.delete(failure.id)

  // Restart/retry resumes the chair call, keeping the already completed recorder call.
  const finalFailure = await discussionSvc.start({ ...opts, summarizer: 'claude', limits: { ...opts.limits, maxPlanningCalls: 12 } })
  await wait(finalFailure.id, settled)
  behave({ failAgent: 'codex', failStep: 'Conclusion' })
  await discussionSvc.summarizeDiscussion(finalFailure.id, true)
  const noConclusion = await wait(finalFailure.id, settled)
  assert.equal(noConclusion.pending.step, 'conclusion')
  assert.equal(noConclusion.discussion.summaries.length, 1)
  assert.equal(noConclusion.discussion.conclusion, undefined)
  const recover = new CoworkService(discussionDeps)
  await recover.init()
  behave({})
  await recover.retry(finalFailure.id)
  const recovered = await waitFor(finalFailure.id, settled, 30000, recover)
  assert.equal(recovered.discussion.summaries.length, 1)
  assert.equal(recovered.calls.filter((c: any) => c.step === 'summary').length, 1)
  assert.equal(recovered.discussion.conclusion.through, 4)
  await recover.shutdown()
  await discussionSvc.delete(finalFailure.id)

  const interrupted = await discussionSvc.start({ ...opts, summarizer: 'claude' })
  await wait(interrupted.id, settled)
  behave({ slowMs: 5000 })
  await discussionSvc.summarizeDiscussion(interrupted.id, true)
  await wait(interrupted.id, (r) => r.calls.some((c: any) => c.step === 'summary' && c.status === 'running'))
  await discussionSvc.cancel(interrupted.id)
  const cancelledSummary = discussionSvc.get(interrupted.id)!
  assert.equal(cancelledSummary.phase, 'cancelled')
  assert.equal(cancelledSummary.discussion!.summaries!.length, 0)
  assert.equal(cancelledSummary.discussion!.conclusion, undefined)
  await discussionSvc.delete(interrupted.id)
  behave({})

  // Long historical records are consumed oldest-first without dropping a prefix.
  const long = await discussionSvc.start({ ...opts, limits: { ...opts.limits, maxPlanningCalls: 30 } })
  const history = await wait(long.id, settled)
  for (let i = 0; i < 29; i++) history.discussion.messages.push({ id: `m${history.discussion.messages.length + 1}`, agent: null, message: `Historical requirement ${i}`, replyTo: null, at: Date.now() })
  await (discussionSvc as any).testPatch(long.id, ['discussion', 'messages'], history.discussion.messages)
  assert.equal(shared.discussionNeedsSummary(history), true)
  await discussionSvc.discuss(long.id, 'Keep all earlier requirements')
  const compressed = await wait(long.id, settled)
  assert.equal(compressed.phase, 'completed')
  assert.equal(compressed.discussion.summaries[0].through, 20)
  assert.match(compressed.discussion.summaries[0].summary, /m1, m2/)
  const speakerPrompt = shared.buildDiscussionPrompt(compressed, 'claude')
  assert.match(speakerPrompt, /EARLY_REQUIREMENT/)
  assert.match(speakerPrompt, /Historical requirement 28/)
  await discussionSvc.summarizeDiscussion(long.id, true)
  const fullyRecorded = await wait(long.id, settled)
  assert.equal(fullyRecorded.discussion.summaries.at(-1).through, fullyRecorded.discussion.messages.length)
  assert.equal(fullyRecorded.discussion.messages.length, 37, 'original messages are retained')
  assert.equal(shared.checkDiscussionSummary({ summary: 'x', consensus: [], disagreements: [null], questions: [] }).ok, false)
  assert.equal(shared.checkDiscussionSummary({ summary: 'x'.repeat(6001), consensus: [], disagreements: [], questions: [] }).ok, false)
  assert.ok(shared.discussionProjectPrompt({ ...fullyRecorded, prompt: 'x'.repeat(20000) }).length > 20000, 'conversion preserves the record for explicit editing instead of silently truncating it')
  assert.equal(shared.sanitizeCoworkSettings({ summarizer: 'evil', summarizeEachRound: 'yes', limits: { maxPlanningCalls: 999 } }).summarizer, null)
  await discussionSvc.delete(long.id)
  console.log('recorder selection, rolling context, original history, summary retries/budget, chair conclusion and approval boundary ok')

  behave({ slowMs: 150 })
  const [a, b] = await Promise.all([
    discussionSvc.start({ ...opts, prompt: 'Meeting A' }),
    discussionSvc.start({ ...opts, prompt: 'Meeting B' })
  ])
  assert.notEqual(a.id, b.id)
  assert.notEqual(a.snapshotDir, b.snapshotDir)
  await wait(a.id, (r) => r.calls.some((c: any) => c.status === 'running'))
  await discussionSvc.cancel(a.id)
  assert.equal((await wait(b.id, settled)).phase, 'completed', 'cancelling A does not stop B')
  assert.ok(b.discussion!.messages.every((m) => !m.message.includes('Meeting A')), 'meeting context stays isolated')
  const c = await discussionSvc.start({ ...opts, prompt: 'Meeting C' })
  await discussionSvc.discuss(b.id, 'Continue Meeting B')
  assert.equal((await wait(c.id, settled)).phase, 'completed')
  assert.equal((await wait(b.id, settled)).phase, 'completed', 'follow-up allowed while another meeting runs')
  const projectOpts = { ...base, mode: 'project' as const }
  const [p1, p2] = await Promise.all([discussionSvc.start(projectOpts), discussionSvc.start(projectOpts)])
  assert.notEqual(p1.snapshotDir, p2.snapshotDir)
  assert.equal((await wait(p1.id, settled)).phase, 'awaiting-approval')
  assert.equal((await wait(p2.id, settled)).phase, 'awaiting-approval')
  for (const r of [a, b, c, p1, p2]) await discussionSvc.delete(r.id)
  console.log('concurrent discussions/project planning, isolated context and cancellation ok')
}

for (const d of cleanup) {
  try {
    fs.rmSync(d, { recursive: true, force: true })
  } catch {
    /* Windows 偶爾還有 handle 沒放 */
  }
}
assert.ok(emitted.length > 0)
console.log('cowork ok')
rust.stop()
