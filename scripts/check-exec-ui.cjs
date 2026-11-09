// node scripts/check-exec-ui.cjs
// 桌面 Cowork 執行介面：真的元件、真的翻譯、假的 window.api。核准內容有沒有顯示，送出時有沒有帶 hash。
const { app, BrowserWindow } = require('./lib/browser-window.cjs')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'workbench-exec-ui-'))
app.setPath('userData', path.join(dir, 'userData'))
app.on('window-all-closed', () => {})

const fixture = `
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ExecLauncher, ExecTasks, ExecTurnCard } from './src/renderer/src/panels/cowork/ExecView'
import CoworkSettings from './src/renderer/src/components/CoworkSettings'
import { useTranslation } from './src/renderer/src/i18n'
import { setLanguage } from './src/renderer/src/store'

window.calls = []
const rec = (name) => async (...args) => { window.calls.push({ name, args }); return window.results?.[name] || { ok: true, data: undefined } }
window.preview = { hash: 'HASH-1', mode: 'sequential', linkDeps: true, bypass: true, baseCommit: 'abc', worktreeRoot: 'C:/repo/.cowork',
  checks: [{ taskId: 't1', argv: ['npm', 'test'], resolved: 'C:/node/npm.cmd' }, { taskId: null, argv: ['npm', 'run', 'lint'], resolved: 'C:/node/npm.cmd' }],
  warnings: ['bypass-on', 'shared-deps', 'checks-run-code', 'cloud-worktree'] }
window.api = { cowork: {
  execPreview: async (...args) => { window.calls.push({ name: 'execPreview', args }); return { ok: true, data: window.preview } },
  execStart: rec('execStart'), execWaive: rec('execWaive'), execWaiveGitConfig: rec('execWaiveGitConfig'), execAcceptScope: rec('execAcceptScope'),
  execRevertScope: rec('execRevertScope'), execMerge: rec('execMerge'), execCleanup: rec('execCleanup'), execRetry: rec('execRetry')
} }

const task = { id: 't1', title: 'Add helper', detail: 'd', scope: ['a.ts'], dependsOn: [], assignee: 'claude', acceptance: ['tests pass'], resources: [] }
const baseRun = (patch) => ({ id: 'r1', phase: 'approved', planRevision: 2, approvedPlanRevision: 2, prompt: 'p', language: 'en', repo: { sourceBranch: 'master', baseCommit: 'abc', root: 'C:/repo' },
  boards: [{ planRevision: 2, tasks: [task], decisions: [], unresolved: [], source: 'chair', at: 1 }], limits: { maxExecutionMinutes: 60 }, ...patch })
const te = (patch) => ({ status: 'blocked', agent: 'claude', worktree: 'main', turns: [], commits: [], ...patch })
const execution = (patch, taskPatch) => ({ mode: 'sequential', gates: true, reviewOn: true, startedAt: 1, bypass: false, linkDeps: true, root: 'C:/repo/.cowork/r1', paused: false, msUsed: 0, activeSince: null,
  worktrees: { main: { path: 'C:/repo/.cowork/r1/main', branch: 'cowork/r1/main' } }, tasks: { t1: te(taskPatch) }, ...patch })

let root
function Host({ kind, run, cw }) {
  const { t } = useTranslation()
  const act = async (p) => { const r = await p; return r.ok }
  const [settings, setSettings] = React.useState(cw)
  if (kind === 'launcher') return <ExecLauncher run={run} t={t} act={act} onChangePlan={() => {}} />
  if (kind === 'tasks') return <ExecTasks run={run} t={t} act={act} now={2} onFollowUp={(id) => window.calls.push({ name: 'followUp', args: [id] })} />
  if (kind === 'turn') return <ExecTurnCard run={run} t={t} act={act} />
  return null
}
window.show = (kind, run) => {
  root?.unmount()
  document.body.innerHTML = '<div id="root"></div>'
  root = createRoot(document.getElementById('root'))
  root.render(<Host kind={kind} run={run} />)
}
window.makeRun = (patch, ex, taskPatch) => baseRun({ ...patch, ...(ex ? { execution: execution(ex, taskPatch) } : {}) })
window.language = setLanguage
window.input = (selector, value) => {
  const el = document.querySelector(selector)
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, value)
  el.dispatchEvent(new Event('input', { bubbles: true }))
}
window.clickText = (text) => {
  const el = [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === text)
  if (!el) throw new Error('no button: ' + text + ' in ' + document.body.innerText)
  el.click()
}
window.btn = (text) => [...document.querySelectorAll('button')].find((b) => b.textContent.trim() === text)
window.ready = true
`

app.whenReady().then(async () => {
  await require('esbuild').build({
    stdin: { contents: fixture, resolveDir: process.cwd(), loader: 'tsx' },
    bundle: true,
    platform: 'browser',
    format: 'iife',
    jsx: 'automatic',
    alias: { '@': path.resolve('src/renderer/src') },
    loader: { '.css': 'empty' },
    define: { 'process.env.NODE_ENV': '"development"' },
    outfile: path.join(dir, 'page.js')
  })
  fs.writeFileSync(path.join(dir, 'index.html'), '<meta charset="UTF-8"><script src="page.js"></script>')
  const win = new BrowserWindow({ show: false, webPreferences: { contextIsolation: false, nodeIntegration: false } })
  const errors = []
  win.webContents.on('console-message', (_e, level, message) => { if (level >= 3) errors.push(message) })
  const run = (code) => win.webContents.executeJavaScript(code)
  const pause = () => win.webContents.settled(120)
  const text = () => run('document.body.innerText')
  const calls = async (name) => run(`window.calls.filter(c => c.name === ${JSON.stringify(name)})`)
  try {
    await win.loadFile(path.join(dir, 'index.html'))
    await pause()
    assert.equal(await run('window.ready'), true)
    await run('language("en")')

    // 開始執行：看得到 Bypass、指令、解析後路徑與警告；送出帶 hash
    await run(`show('launcher', makeRun({}))`)
    await pause()
    let t = await text()
    for (const want of ['npm test', 'C:/node/npm.cmd', 'npm run lint', 'After integration', 'not sandboxed', 'cloud-synced', 'C:/repo/.cowork']) assert.ok(t.includes(want), `start: ${want}\n${t}`)
    assert.ok(!t.includes('cowork.'), `untranslated key: ${t}`)
    await run(`clickText('Start execution')`)
    await pause()
    const started = await calls('execStart')
    assert.equal(started.length, 1)
    assert.deepEqual(started[0].args, ['r1', { mode: 'sequential', linkDeps: true, approval: 'HASH-1' }])
    assert.ok((await calls('execPreview')).length >= 2, 'preview refreshes after a start attempt')

    // 指令解析不到：不能開始
    await run(`window.preview = { ...window.preview, checks: [{ taskId: 't1', argv: ['nope'], resolved: null, error: 'command not found: nope' }] }; show('launcher', makeRun({}))`)
    await pause()
    assert.ok((await text()).includes('command not found: nope'))
    assert.equal(await run(`btn('Start execution').disabled`), true)

    // 完成條件沒過：要寫理由才能豁免
    await run(`window.calls = []; show('tasks', makeRun({ phase: 'executing' }, {}, { gate: { kind: 'acceptance', items: ['#1 not done: tests pass'], followUp: false }, error: 'x' }))`)
    await pause()
    t = await text()
    assert.ok(t.includes('Acceptance items are missing') && t.includes('#1 not done'), t)
    assert.equal(await run(`btn('Accept anyway').disabled`), true)
    await run(`input('.cw-input', 'checked by hand')`)
    await pause()
    await run(`clickText('Accept anyway')`)
    await pause()
    assert.deepEqual((await calls('execWaive'))[0].args, ['r1', 't1', 'checked by hand'])

    // git 設定被改：走桌面專用的操作
    await run(`window.calls = []; show('tasks', makeRun({ phase: 'executing' }, {}, { gate: { kind: 'git-config', items: ['.git/config'], followUp: false }, error: 'x' }))`)
    await pause()
    assert.ok((await text()).includes('git configuration or hooks changed'))
    await run(`input('.cw-input', 'inspected')`)
    await pause()
    await run(`clickText('Accept anyway')`)
    await pause()
    assert.equal((await calls('execWaive')).length, 0)
    assert.deepEqual((await calls('execWaiveGitConfig'))[0].args, ['r1', 't1', 'inspected'])

    // 越界改動：接受時帶內容 hash
    await run(`window.calls = []; show('tasks', makeRun({ phase: 'executing' }, {}, { scopeReview: { files: ['x.txt'], followUp: false, hash: 'SCOPE-H' }, error: 'x' }))`)
    await pause()
    await run(`clickText('Accept all and commit')`)
    await pause()
    assert.deepEqual((await calls('execAcceptScope'))[0].args, ['r1', 't1', 'SCOPE-H'])

    // 豁免紀錄與「agent 自行回報」不能看起來像驗證通過
    await run(`show('tasks', makeRun({ phase: 'review' }, {}, { status: 'done', waiver: { kind: 'checks', reason: 'flaky', items: [], at: 1 } }))`)
    await pause()
    assert.ok((await text()).includes('Accepted without verification: flaky'))
    await run(`show('tasks', makeRun({ phase: 'review' }, {}, { status: 'done' }))`)
    await pause()
    assert.ok((await text()).includes('Agent-reported'))

    // 合併：沒問題時直接可按，帶 commit 與目標 HEAD
    const integ = `{ status: 'ok', branch: 'cowork/r1/main', worktree: 'main', commit: 'C1', patchId: 'P1', stat: '1 file', checks: 'passed', checkRuns: [{ argv: ['npm','test'], ok: true }] }`
    const review = (extra) => `{ status: 'ok', agent: 'codex', independence: 'unconfirmed', implementers: ['claude'], findings: [], commit: 'C1', patchId: 'P1', truncated: false, at: 1, verdict: 'pass', message: 'looks fine', ${extra} }`
    await run(`window.calls = []; show('turn', makeRun({ phase: 'review' }, { integration: ${integ}, targetHead: 'T1', review: ${review('')} }, { status: 'done' }))`)
    await pause()
    t = await text()
    for (const want of ['Integration checks passed', 'Review by Codex', 'independence not confirmed', 'Advisory only', 'looks fine']) assert.ok(t.includes(want), `${want}\n${t}`)
    assert.ok(!/cowork\.[a-zA-Z_]+/.test(t), `untranslated key: ${t}`)
    assert.equal(await run(`btn('Merge into master').disabled`), false)
    await run(`clickText('Merge into master')`)
    await pause()
    assert.deepEqual((await calls('execMerge'))[0].args, ['r1', { commit: 'C1', targetHead: 'T1' }])

    // 高嚴重度：要寫理由才能合併，理由一起送出
    const high = `findings: [{ severity: 'high', file: 'a.ts', line: 3, problem: 'seeded defect', evidence: 'line 3' }], verdict: 'fail'`
    await run(`window.calls = []; show('turn', makeRun({ phase: 'review' }, { integration: ${integ}, targetHead: 'T1', review: ${review(high)} }, { status: 'done' }))`)
    await pause()
    t = await text()
    assert.ok(t.includes('seeded defect') && t.includes('Resolve these'), t)
    assert.equal(await run(`btn('Merge into master').disabled`), true)
    await run(`input('.cw-input', 'accepted risk')`)
    await pause()
    assert.equal(await run(`btn('Merge into master').disabled`), false)
    await run(`clickText('Merge into master')`)
    await pause()
    assert.deepEqual((await calls('execMerge'))[0].args, ['r1', { commit: 'C1', targetHead: 'T1', waive: 'accepted risk' }])

    // 目標分支移動後衝突：不能合併；審查失敗要明確顯示未審查
    await run(`show('turn', makeRun({ phase: 'review' }, { integration: ${integ}, targetHead: 'T2', targetCheck: { head: 'T2', status: 'conflict', at: 1 }, review: ${review('')} }, { status: 'done' }))`)
    await pause()
    assert.ok((await text()).includes('merge now conflicts'))
    assert.equal(await run(`btn('Merge into master').disabled`), true)
    await run(`show('turn', makeRun({ phase: 'review' }, { integration: ${integ}, targetHead: 'T1', review: { status: 'failed', implementers: ['claude'], findings: [], commit: 'C1', patchId: 'P1', truncated: false, at: 1, error: 'quota' } }, { status: 'done' }))`)
    await pause()
    t = await text()
    assert.ok(t.includes('Not reviewed: quota'), t)
    assert.equal(await run(`btn('Merge into master').disabled`), true)

    // 中文也都有翻譯
    await run(`language('zh-TW')`)
    await run(`window.preview = { hash: 'H', mode: 'sequential', linkDeps: true, bypass: false, baseCommit: 'a', worktreeRoot: 'C:/x', checks: [{ taskId: null, argv: ['a'], resolved: 'C:/a.exe' }], warnings: ['shared-deps', 'checks-run-code', 'cloud-worktree'] }; show('launcher', makeRun({}))`)
    await pause()
    t = await text()
    assert.ok(t.includes('沒有沙箱隔離') && t.includes('整合後') && !t.includes('cowork.'), t)
    await run(`show('turn', makeRun({ phase: 'review' }, { integration: ${integ}, targetHead: 'T1', review: ${review(high)} }, { status: 'done' }))`)
    await pause()
    t = await text()
    assert.ok(t.includes('僅供參考') && t.includes('整合檢查通過') && !/cowork\.[a-zA-Z_]+/.test(t), t)

    assert.deepEqual(errors.filter((e) => !/Download the React DevTools/.test(e)), [])
    console.log('exec ui ok')
  } catch (e) {
    console.error(e)
    console.error('console errors:', errors.slice(0, 5))
    process.exitCode = 1
  } finally {
    try {
      fs.rmSync(dir, { recursive: true, force: true })
    } catch {
      /* Windows 偶爾還有 handle */
    }
    app.exit(process.exitCode || 0)
  }
})
