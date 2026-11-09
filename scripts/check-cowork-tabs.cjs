// node scripts/check-cowork-tabs.cjs
// Real renderer, mocked IPC: independent tab state and background meeting updates.
const { app, BrowserWindow } = require('./lib/browser-window.cjs')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const assert = require('node:assert/strict')

const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cowork-tabs-check-'))
app.setPath('userData', path.join(dir, 'user-data'))
app.on('window-all-closed', () => {})

const fixture = `
import React from 'react'
import { createRoot } from 'react-dom/client'
import TerminalPanel from './src/renderer/src/panels/terminal/TerminalPanel'
import SettingsModal from './src/renderer/src/components/SettingsModal'
import { setWorkspaceRoot, setLanguage, setTheme } from './src/renderer/src/store'
import './src/renderer/src/styles.css'
const agents = ['claude', 'codex']
const ok = data => Promise.resolve({ ok: true, data: structuredClone(data) })
const noop = () => () => {}
const runs = new Map()
const listeners = new Set()
window.actions = []
window.publish = (id, phase) => {
  const run = runs.get(id)
  run.phase = phase
  run.revision++
  listeners.forEach(cb => cb(structuredClone(run)))
}
window.listenerCount = () => listeners.size
window.settingsCowork = { participants: agents, chair: 'codex', models: {}, limits: { maxPlanningCalls: 6, maxPlanningMinutes: 20, maxExecutionMinutes: 60 } }
window.setFixtureAppearance = (language, theme) => { setLanguage(language); setTheme(theme) }
window.showCoworkSettings = () => {
  const target = document.createElement('div'); target.id = 'settings-fixture'; document.body.append(target)
  const root = createRoot(target)
  root.render(<SettingsModal isOpen initialTab="cowork" onClose={() => window.closeCoworkSettings()} />)
  window.closeCoworkSettings = () => { root.unmount(); target.remove() }
}
window.api = {
  settings: { get: () => Promise.resolve({ cliPaths: {}, cliEnabled: {}, cowork: window.settingsCowork }), set: settings => { if (settings.cowork) window.settingsCowork = settings.cowork; return Promise.resolve() } },
  ext: { inventory: () => Promise.resolve([]), agents: () => Promise.resolve([]) },
  files: { detectDocTools: () => Promise.resolve({}) },
  updater: { getStatus: () => Promise.resolve({ state: 'idle' }), onStatusChange: noop },
  window: { setTitleBarTheme: () => {} },
  memsync: { pluginState: () => Promise.resolve({ ok: false }) },
  notify: { show: () => {} },
  pty: { launchers: () => Promise.resolve([]), onRemoteSpawned: noop,
    spawn: () => Promise.resolve('mock-pty'), onData: noop, onResized: noop, onExit: noop, resize: () => {}, write: () => {}, kill: () => {} },
  cowork: {
    list: () => ok([...runs.values()]), get: id => ok(runs.get(id) || null),
    onUpdate: cb => { listeners.add(cb); return () => listeners.delete(cb) },
    capabilities: () => ok({ agents: agents.map(agent => ({ agent, enabled: true, planning: true })), baseline: { ok: true, branch: 'master', head: '1234567890', dirty: [], dirtyCount: 0, warnings: [] } }),
    models: () => ok(['claude', 'codex', 'antigravity'].map(agent => ({ agent,
      options: agent === 'claude' ? [{ id: 'opus', label: 'Opus 5.5', efforts: ['medium', 'high'] }, { id: 'sonnet', label: 'Sonnet 4.6', efforts: ['low'] }]
        : [{ id: 'listed-model', label: agent === 'codex' ? 'Codex model with a long version label' : 'Gemini model with a long version label', efforts: ['medium'] }],
      fallback: { model: agent === 'claude' ? 'opus' : 'listed-model', effort: 'medium', source: 'cli-default' } }))),
    start: req => {
      const id = 'run-' + (runs.size + 1), at = Date.now()
      const run = { schemaVersion: 1, id, revision: 1, planRevision: 1, approvedPlanRevision: null, ...req,
        createdAt: at, updatedAt: at, chairExecutes: true, phase: 'meeting', block: null, pending: { step: 'discussion' },
        repo: { root: 'C:/test', commonDir: 'C:/test/.git', sourceBranch: 'master', baseCommit: '', excludedDirty: [] }, snapshotDir: 'C:/isolated/' + id,
        notes: [], r1: null, reviewers: {}, boards: [], calls: [], log: [],
        discussion: { messages: [{ id: 'm1', agent: null, message: req.prompt, replyTo: null, at }], order: agents, cursor: 0, excluded: [], summarizer: req.summarizer, summaries: [] },
        budget: { planningCallsUsed: 0, planningMsUsed: 0, activeSince: null, costUsd: 0, tokens: 0 },
        limits: { maxPlanningCalls: 6, maxPlanningMinutes: 20 } }
      runs.set(id, run)
      window.actions.push('start:' + id)
      return ok(run)
    },
    cancel: id => { window.actions.push('cancel:' + id); window.publish(id, 'cancelled'); return ok(null) },
    setSummarizer: (id, agent) => { runs.get(id).discussion.summarizer = agent; window.publish(id, 'completed'); return ok(null) },
    summarize: (id, conclude) => {
      const r = runs.get(id), d = r.discussion
      if (d.summaries.at(-1)?.through !== d.messages.length) d.summaries.push({ agent: d.summarizer, through: d.messages.length, at: Date.now(), summary: 'Keep the original Draft A goal', consensus: ['Use the agreed approach'], disagreements: ['Claude prefers option B'], questions: ['User must approve'] })
      if (conclude) d.conclusion = { through: d.messages.length, at: Date.now(), message: 'Chair recommendation: keep Draft A and require user approval.' }
      window.actions.push(conclude ? 'conclude:' + id : 'summarize:' + id)
      window.publish(id, 'completed'); return ok(null)
    },
    discuss: (id, message) => { const d = runs.get(id).discussion; d.messages.push({ id: 'm' + (d.messages.length + 1), agent: null, message, replyTo: null, at: Date.now() }); window.publish(id, 'completed'); return ok(null) }
  }
}
setLanguage('en')
setWorkspaceRoot('C:/test')
createRoot(document.getElementById('root')).render(<TerminalPanel />)
`

app.whenReady().then(async () => {
  let win
  try {
    await require('esbuild').build({ stdin: { contents: fixture, resolveDir: process.cwd(), loader: 'tsx' }, bundle: true,
      platform: 'browser', format: 'iife', jsx: 'automatic', outfile: path.join(dir, 'page.js'), define: { 'process.env.NODE_ENV': '"production"' } })
    fs.writeFileSync(path.join(dir, 'index.html'), '<meta charset="UTF-8"><link rel="stylesheet" href="page.css"><div id="root" style="height:100vh"></div><script src="page.js"></script>')
    win = new BrowserWindow({ show: false, width: 1200, height: 850, webPreferences: { offscreen: true, backgroundThrottling: false } })
    const errors = []
    win.webContents.on('console-message', (_event, level, message) => { if (level >= 3) errors.push(message) })
    const run = code => win.webContents.executeJavaScript(code).catch(error => { throw new Error(`${code}\n${error.message}`) })
    const pause = () => win.webContents.settled(250)
    const add = async name => {
      await run('document.querySelector(".term-add-dropdown-btn").click()')
      await pause()
      await run(`[...document.querySelectorAll('.term-popover-item')].find(e => e.querySelector('.term-popover-name')?.textContent === ${JSON.stringify(name)}).click()`)
      await pause()
    }
    const type = async text => {
      await run(`{ const field = document.querySelector('.cw-host:not([hidden]) .cw-start-prompt'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(field, ${JSON.stringify(text)}); field.dispatchEvent(new Event('input', { bubbles: true })); }`)
      await pause()
    }
    const tab = async index => { await run(`document.querySelectorAll('.term-cowork-tab')[${index}].click()`); await pause() }
    const start = async () => { await run('document.querySelector(".cw-host:not([hidden]) .cw-start-footer .primary").click()'); await pause() }
    await win.loadFile(path.join(dir, 'index.html'))
    await pause()
    await add('Cowork')
    await type('Draft A')
    await add('Cowork')
    assert.equal(await run('document.querySelectorAll(".term-cowork-tab").length'), 2)
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-start-prompt").value'), '')
    await type('Draft B')
    await tab(0)
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-start-prompt").value'), 'Draft A')
    await start()
    await tab(1)
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-start-prompt").value'), 'Draft B')
    await start()
    assert.deepEqual(await run('[...document.querySelectorAll(".term-cowork-tab .term-tab-title")].map(e => e.textContent)'), ['Draft A', 'Draft B'])
    await run('window.publish("run-1", "completed")')
    await pause()
    assert.equal(await run('!!document.querySelectorAll(".term-cowork-tab")[0].querySelector(".phase-completed")'), true)
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-picker-label").textContent'), 'Draft B')
    await add('Cowork')
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-start-prompt").value'), '', 'new tab does not restore an active meeting')
    await run('document.querySelectorAll(".term-cowork-tab")[0].querySelector(".term-tab-close").click()')
    await pause()
    assert.equal(await run('document.querySelector(".term-cowork-tab.active .term-tab-title").textContent'), 'Cowork 3')
    assert.equal(await run('window.listenerCount()'), 2)
    assert.deepEqual(await run('window.actions'), ['start:run-1', 'start:run-2'], 'closing a tab does not cancel a meeting')
    await run('document.querySelector(".term-cowork-tab.active .term-tab-close").click()')
    await pause()
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-picker-label").textContent'), 'Draft B')
    await add('PowerShell')
    assert.equal(await run('document.querySelectorAll(".term-unified-tab.active").length'), 1)
    assert.equal(await run('document.querySelectorAll(".cw-host:not([hidden])").length'), 0)
    await tab(0)
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-picker-label").textContent'), 'Draft B')
    await add('Cowork')
    await run('document.querySelector(".cw-host:not([hidden]) .cw-picker-btn").click()')
    await pause()
    await run('[...document.querySelectorAll(".cw-host:not([hidden]) .cw-menu-item")].find(e => e.querySelector(".cw-menu-prompt")?.textContent === "Draft A").click()')
    await pause()
    assert.equal(await run('document.querySelector(".cw-host:not([hidden]) .cw-picker-label").textContent'), 'Draft A')
    assert.equal(await run('[...document.querySelectorAll(".cw-host:not([hidden]) button")].some(e => e.textContent === "Create plan from conclusion")'), false, 'conversion waits for the chair conclusion')
    await run('{ const s = document.querySelector(".cw-host:not([hidden]) .cw-summary-options select"); s.value = "claude"; s.dispatchEvent(new Event("change", { bubbles: true })); }')
    await pause()
    await run('[...document.querySelectorAll(".cw-host:not([hidden]) button")].find(e => e.textContent === "Summarize now").click()')
    await pause()
    assert.match(await run('document.querySelector(".cw-host:not([hidden]) .cw-summary").textContent'), /Claude Code.*Agreements.*Disagreements.*Decisions needed/s)
    await run('[...document.querySelectorAll(".cw-host:not([hidden]) button")].find(e => e.textContent === "Ask chair to conclude").click()')
    await pause()
    assert.match(await run('document.querySelector(".cw-host:not([hidden]) .cw-conclusion").textContent'), /require user approval/)
    fs.writeFileSync(path.join(dir, 'tabs.png'), (await win.webContents.capturePage()).toPNG())
    await run('[...document.querySelectorAll(".cw-host:not([hidden]) button")].find(e => e.textContent === "Create plan from conclusion").click()')
    await pause()
    assert.match(await run('document.querySelector(".cw-host:not([hidden]) .cw-start-prompt").value'), /Keep the original Draft A goal.*Chair recommendation/s)
    assert.equal(await run('window.actions.filter(a => a.startsWith("start:")).length'), 2, 'conversion prepares an editable form and does not execute')
    await run('window.settingsCowork.models = { codex: { model: "legacy-saved-model", effort: "high" } }; window.showCoworkSettings()')
    await pause()
    assert.equal(await run('[...document.querySelectorAll("option")].some(e => /custom|自訂/i.test(e.textContent))'), false, 'neither meeting nor Settings offers custom model entry')
    assert.equal(await run('document.querySelectorAll(".cw-model-custom").length'), 0)
    assert.equal(await run('document.querySelector("#settings-fixture option[value=legacy-saved-model]").disabled'), true, 'saved models remain visible without a custom input')
    assert.ok((await run('document.querySelector("#settings-fixture").innerText')).includes('Review after implementation'), 'the review switch is in Settings')
    assert.ok((await run('document.querySelector("#settings-fixture").innerText')).includes('Worktree location'), 'the worktree location is in Settings')
    await run('{ const row = [...document.querySelectorAll("#settings-fixture .macos-cowork-model-row")].find(e => e.textContent.includes("Codex")); const model = row.querySelector("select"); model.value = "listed-model"; model.dispatchEvent(new Event("change", { bubbles: true })); }')
    await pause()
    assert.equal(await run('[...document.querySelectorAll("#settings-fixture .macos-cowork-model-row")].find(e => e.textContent.includes("Codex")).querySelectorAll("select")[1].value'), '', 'switching model clears unsupported effort')
    assert.deepEqual(await run('[...document.querySelectorAll("#settings-fixture input[type=number]")].map(e => Number(e.value))'), [6, 20, 60])
    await run('{ const e = [...document.querySelectorAll("#settings-fixture input")].find(e => e.getAttribute("aria-label") === "Model calls"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(e, "12"); e.dispatchEvent(new Event("input", { bubbles: true })); }')
    await pause()
    await run('document.querySelector("#settings-fixture .macos-btn-primary").click()')
    await pause()
    assert.equal(await run('window.settingsCowork.limits.maxPlanningCalls'), 12)
    await new Promise(resolve => setTimeout(resolve, 700))
    await run('window.showCoworkSettings()')
    await pause()
    for (const width of [1200, 720]) {
      win.setContentSize(width, 850)
      for (const language of ['en', 'zh-TW']) for (const theme of ['light', 'dark', 'light-morandi', 'dark-morandi']) {
        await run(`window.setFixtureAppearance(${JSON.stringify(language)}, ${JSON.stringify(theme)})`)
        await pause()
        const overflow = await run(`(() => {
          const bad = [], root = document.querySelector('.macos-cowork-settings');
          for (const text of root.querySelectorAll('.macos-row-sub, .macos-row-title, .macos-lang-name')) {
            const container = text.closest('.macos-row, .macos-lang-card, .macos-inset-group'), bounds = container.getBoundingClientRect(), range = document.createRange(); range.selectNodeContents(text);
            for (const rect of range.getClientRects()) if (rect.width && (rect.left < bounds.left - 1 || rect.right > bounds.right + 1)) bad.push(text.textContent);
          }
          for (const row of root.querySelectorAll('.macos-row-main')) {
            const left = row.querySelector('.macos-row-left, .macos-row-info'), right = row.querySelector('.macos-row-right');
            if (left && right) { const l = left.getBoundingClientRect(), r = right.getBoundingClientRect(); if (l.bottom > r.top + 1 && r.bottom > l.top + 1 && l.right > r.left + 1) bad.push('overlap: ' + row.textContent); }
          }
          const canvas = document.createElement('canvas'), ctx = canvas.getContext('2d');
          for (const row of root.querySelectorAll('.macos-cowork-model-row')) {
            const effort = row.querySelectorAll('select')[1], style = getComputedStyle(effort); ctx.font = style.font;
            if (ctx.measureText(effort.selectedOptions[0].textContent).width + 30 > effort.clientWidth) bad.push('effort label clipped');
          }
          if (root.scrollWidth > root.clientWidth + 1) bad.push('horizontal overflow');
          return bad;
        })()`)
        assert.deepEqual(overflow, [], `Settings must fit at ${width}px, ${language}, ${theme}`)
        for (const [name, code] of [['top', 'document.querySelector(".macos-settings-body").scrollTop = 0'], ['models', 'document.querySelector(".macos-cowork-model-row").closest(".macos-section").scrollIntoView({ block: "start" })'], ['budget', 'document.querySelector(".macos-settings-body").scrollTop = 9999']]) {
          await run(code)
          await pause()
          fs.writeFileSync(path.join(dir, `settings-${width}-${language}-${theme}-${name}.png`), (await win.webContents.capturePage()).toPNG())
        }
      }
    }
    await run('window.closeCoworkSettings()')
    assert.deepEqual(errors, [], 'renderer must not throw')
    console.log('Cowork tabs, records, actual Settings save, model choices and layout (2 sizes / 2 languages / 4 themes) passed')
    console.log('Screenshot: ' + path.join(dir, 'tabs.png'))
  } catch (error) { console.error(error); if (win) console.error(await win.webContents.executeJavaScript('document.body.innerText')); process.exitCode = 1 }
  finally { win?.destroy(); app.exit(process.exitCode || 0) }
})
