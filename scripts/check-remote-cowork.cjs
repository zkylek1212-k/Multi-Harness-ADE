// npm run build:ui && node scripts/check-remote-cowork.cjs [screenshot-dir]
// 手機版 Cowork：真的手機 bundle（out/renderer/remote.html），假的桌面 WebSocket。
// 驗證開會表單帶出桌面記住的與會者、送出的 op／參數跟桌面 window.api.cowork 一致，以及各階段的按鈕與輸入框。
const { app, BrowserWindow } = require('./lib/browser-window.cjs')
const fs = require('node:fs')
const http = require('node:http')
const os = require('node:os')
const path = require('node:path')
const assert = require('node:assert/strict')

const root = path.join(__dirname, '../out/desktop')
const shots = process.argv[2]
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remote-cowork-check-'))
app.setPath('userData', path.join(dir, 'user-data'))

const preload = path.join(dir, 'preload.js')
fs.writeFileSync(preload, `
const RUN = { id: 'r1', prompt: 'Plan the release', phase: 'meeting', mode: 'discussion', chair: 'codex', participants: ['claude', 'codex'],
  revision: 1, planRevision: 0, updatedAt: Date.now(), createdAt: Date.now(), limits: { maxPlanningCalls: 6, maxPlanningMinutes: 20 },
  reviewers: {}, boards: [], log: [], notes: [], pending: null, block: null, r1: null, repo: { sourceBranch: 'master' },
  discussion: { messages: [], order: ['codex', 'claude'], cursor: 0, excluded: [] } }
window.__ops = []
window.__ws = null
class FakeSocket {
  static OPEN = 1
  constructor() { this.readyState = 0; window.__ws = this; setTimeout(() => { this.readyState = 1; this.onopen && this.onopen() }) }
  emit(m) { setTimeout(() => this.onmessage && this.onmessage({ data: JSON.stringify(m) })) }
  close() { this.readyState = 3 }
  send(raw) {
    const m = JSON.parse(raw)
    if (m.t === 'auth') {
      this.emit({ t: 'authed', deviceId: 'd', deviceName: 'phone', hostName: 'desk' })
      this.emit({ t: 'state', sessions: [], windows: [{ id: 7, workspaceName: 'demo', workspace: 'C:/demo', launchers: [] }], workspaces: [], bypass: false })
    }
    if (m.t !== 'cowork') return
    window.__ops.push({ windowId: m.windowId, op: m.op, args: m.args })
    const data = m.op === 'capabilities'
      ? { agents: [{ agent: 'claude', enabled: true, planning: true, path: 'c' }, { agent: 'codex', enabled: true, planning: true, path: 'x' }, { agent: 'antigravity', enabled: true, planning: false, path: null, reason: 'not-installed' }],
          baseline: { ok: true }, defaults: { chair: 'codex', participants: ['claude', 'codex'], models: { claude: { model: 'opus', effort: 'high' } }, summarizer: null, summarizeEachRound: true, limits: { maxPlanningCalls: 6, maxPlanningMinutes: 10, maxExecutionMinutes: 30 } } }
      : m.op === 'models' ? [{ agent: 'claude', options: [{ id: 'opus', label: 'Opus' }], fallback: { model: '', effort: '', source: 'cli-default' } }]
      // 沒回傳值的操作跟真的桌面一樣不帶 data（JSON 會丟掉 undefined）
      : m.op === 'execPreview' ? { hash: 'preview-hash', mode: m.args[1].mode, linkDeps: true, bypass: true, baseCommit: 'abc', checks: [{ taskId: 't1', argv: ['npm', 'test'], resolved: 'C:/node/npm.cmd' }], worktreeRoot: 'C:/x', warnings: ['bypass-on'] }
      : m.op === 'list' ? [] : m.op === 'start' || m.op === 'get' ? RUN : undefined
    this.emit({ t: 'cowork', reqId: m.reqId, result: { ok: true, data } })
  }
}
window.WebSocket = FakeSocket
window.__push = (run) => window.__ws.emit({ t: 'coworkRun', run: Object.assign({}, RUN, run) })
`)

const server = http.createServer((req, res) => {
  const file = path.join(root, req.url === '/' ? 'remote.html' : decodeURIComponent(req.url.split('?')[0]))
  if (!file.startsWith(root) || !fs.existsSync(file)) return res.writeHead(404).end()
  res.writeHead(200, { 'Content-Type': file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' })
  fs.createReadStream(file).pipe(res)
})

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

app.whenReady().then(async () => {
  await new Promise((r) => server.listen(0, '127.0.0.1', r))
  const origin = `http://127.0.0.1:${server.address().port}`
  const win = new BrowserWindow({ show: false, width: 390, height: 844, webPreferences: { preload, contextIsolation: false, sandbox: false } })
  const js = (code) => win.webContents.executeJavaScript(code)
  const waitFor = async (code, what) => {
    for (let i = 0; i < 100; i++) {
      if (await js(code)) return
      await sleep(50)
    }
    throw new Error(`timed out: ${what}`)
  }
  const hasText = (s) => `document.body.innerText.includes(${JSON.stringify(s)})`
  const click = (selector, label) =>
    js(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.innerText.includes(${JSON.stringify(label)})); if (!b) return false; b.click(); return true })()`)
  const type = (selector, value) =>
    js(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, ${JSON.stringify(value)}); el.dispatchEvent(new Event('input', { bubbles: true })) })()`)
  const lastOp = () => js('window.__ops.at(-1)')
  const shot = async (name) => shots && fs.writeFileSync(path.join(shots, `${name}.png`), (await win.webContents.capturePage()).toPNG())

  try {
    await win.loadURL(origin)
    await js(`localStorage.setItem('aw.remote.lang', 'en'); localStorage.setItem('aw.remote.tokens', JSON.stringify({ 'https://${new URL(origin).host}/': 'tok' })); location.reload()`)
    await waitFor(`!!document.querySelector('.tile')`, 'home tiles')
    // Cowork 跟桌面一樣放在新增終端的選單裡，不再是工作區的獨立方塊
    assert.equal(await click('.tile', 'Cowork'), false, 'no Cowork tile')
    assert.ok(await click('.add-card', 'New Terminal'), 'new terminal')
    await waitFor(`!!document.querySelector('.sheet-layer .launcher')`, 'new terminal sheet')
    await shot('new-sheet')
    assert.ok(await click('.launcher', 'Cowork'), 'Cowork in new terminal sheet')
    await waitFor(hasText('No Cowork meetings'), 'empty list')
    assert.deepEqual(await lastOp(), { windowId: 7, op: 'list', args: [] })

    // 開新會議：預設帶出桌面記住的與會者與主席，不可用的 agent 不能選
    await click('button', 'New Meeting')
    await waitFor(hasText('Not installed'), 'capabilities')
    // 桌面表單有的選項手機都有：模式說明、模型與強度、摘要 agent 與時機、依階段調強度、上限
    await waitFor(`[...document.querySelectorAll('.cw-models select')].some(s => [...s.options].some(o => o.value === 'opus'))`, 'model catalog')
    for (const s of ['Take turns speaking', 'Recorder', 'After every round', 'Adapt Effort to the Stage', 'Limits: 6 model calls', 'Make Chair'])
      assert.ok(await js(hasText(s)), s)
    assert.equal(await js(`document.querySelector('.cw-models select').value`), 'opus', 'remembered model')
    await type('.cw-input', 'Plan the release')
    await shot('new-meeting')
    await click('button.btn', 'Start Discussion')
    await waitFor(hasText('Codex is speaking'), 'run screen')
    const start = (await js('window.__ops')).find((o) => o.op === 'start')
    assert.deepEqual(start.args, [{ prompt: 'Plan the release', chair: 'codex', participants: ['claude', 'codex'], language: 'en', mode: 'discussion',
      models: { claude: { model: 'opus', effort: 'high' }, codex: { model: '', effort: '' } }, autoEffort: true, summarizer: 'codex', summarizeEachRound: true }])
    assert.equal(await js(`!!document.querySelector('.composer')`), false, 'no composer while agents speak')

    // 討論一輪結束：顯示發言、可以接著討論與請主席下結論
    await js(`__push({ phase: 'completed', discussion: { messages: [{ id: 'm1', agent: 'codex', message: '**Ship** on Friday', replyTo: null, at: 1 }], order: ['codex', 'claude'], cursor: 0, excluded: [] } })`)
    await waitFor(hasText('Ship on Friday'), 'discussion message')
    assert.ok(await js(hasText('Ask Chair to Conclude')))
    await type('.composer textarea', 'What about QA?')
    await js(`document.querySelector('.composer').requestSubmit()`)
    await waitFor(`window.__ops.at(-1).op === 'discuss'`, 'discuss op')
    assert.deepEqual((await lastOp()).args, ['r1', 'What about QA?'])
    await shot('discussion')

    // 討論中有人發言失敗：可以略過他繼續；超出預算：提高上限後真的會重試
    await js(`__push({ phase: 'blocked', block: { kind: 'step-failed', message: 'codex failed' }, pending: { step: 'discussion' } })`)
    await waitFor(hasText('Skip Codex and Continue'), 'skip speaker')
    await click('button.btn', 'Skip Codex and Continue')
    await waitFor(`window.__ops.at(-1).op === 'drop'`, 'drop op')
    await js(`__push({ phase: 'blocked', block: { kind: 'budget', message: 'Out of budget' }, pending: null })`)
    await waitFor(hasText('Raise Limits and Retry'), 'raise limits')
    await click('button.btn', 'Raise Limits and Retry')
    await waitFor(`window.__ops.at(-1).op === 'retry'`, 'retry after raiseLimits')

    // 專案規劃等核准：任務板、核准帶版本號、輸入框送修改意見
    await js(`__push({ mode: 'project', discussion: undefined, phase: 'awaiting-approval', planRevision: 2, r1: { summary: 'Two tasks' },
      boards: [{ planRevision: 2, source: 'chair', at: 1, tasks: [{ id: 'T1', title: 'Write notes', detail: 'Changelog', scope: [], dependsOn: [], assignee: 'claude', acceptance: [], resources: [] }], decisions: [], unresolved: [] }] })`)
    await waitFor(hasText('T1 · Write notes'), 'board')
    assert.equal(await js(`document.querySelector('.composer textarea').placeholder`), 'Feedback on the board…')
    await click('button.btn', 'Approve Rev 2')
    await waitFor(`window.__ops.at(-1).op === 'approve'`, 'approve op')
    assert.deepEqual((await lastOp()).args, ['r1', 2])
    await shot('approval')

    // 取消要先確認
    await click('button', 'Cancel Meeting')
    await waitFor(`!!document.querySelector('.action-sheet')`, 'confirm sheet')
    await click('.action-sheet button', 'Cancel Meeting')
    await waitFor(`window.__ops.at(-1).op === 'cancel'`, 'cancel op')

    // 核准後：開始背景執行
    await js(`__push({ phase: 'approved' })`)
    await waitFor(hasText('Run Sequentially'), 'exec buttons')
    await click('button.btn', 'Run Sequentially')
    // 先看要核准的內容（Bypass、會執行的指令），確認後才開始，並帶上內容 hash
    await waitFor(`window.__ops.at(-1).op === 'execPreview'`, 'execPreview op')
    await waitFor(hasText('Bypass is on'), 'bypass state is shown')
    await waitFor(hasText('npm test'), 'checks are shown')
    await click('button.btn', 'Confirm and start')
    await waitFor(`window.__ops.at(-1).op === 'execStart'`, 'execStart op')
    assert.deepEqual((await lastOp()).args, ['r1', { mode: 'sequential', approval: 'preview-hash' }])

    // 結束的會議可以刪除，刪完回清單
    await js(`__push({ phase: 'cancelled' })`)
    await waitFor(hasText('Delete Meeting'), 'delete button')
    await click('button', 'Delete Meeting')
    await waitFor(`!!document.querySelector('.action-sheet')`, 'delete confirm')
    await click('.action-sheet button', 'Delete Meeting')
    await waitFor(`window.__ops.some(o => o.op === 'delete') && window.__ops.at(-1).op === 'list'`, 'delete op')
    await waitFor(hasText('No Cowork meetings'), 'back to list')
    console.log('remote cowork ok')
  } catch (e) {
    console.error(e)
    console.error(await js('document.body.innerText').catch(() => ''))
    process.exitCode = 1
  } finally {
    server.close()
    app.exit(process.exitCode || 0)
  }
})
