// Run with: node scripts/check-terminal-ui.cjs
// Uses headless Edge and real xterm, no test framework.
const { app, BrowserWindow } = require('./lib/browser-window.cjs')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { version } = require('../package.json')
app.on('window-all-closed', () => {})

const fixture = `
import React from 'react'
import { createRoot } from 'react-dom/client'
import AppleAlertDialog from '/src/renderer/src/components/AppleAlertDialog.tsx'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { trackComposition, isImeKey } from '/src/renderer/src/panels/terminal/imeGuard.ts'
import { looksLikeApprovalPrompt, readApprovalScreen } from '/src/shared/approvalDetect.ts'
import TerminalView from '/src/renderer/remote/TerminalView.tsx'
import { Home, SettingsSheet } from '/src/renderer/remote/App.tsx'
import '/src/renderer/remote/remote.css'
window.mountDialog = () => {
  window.homeRoot?.unmount()
  window.mobileRoot?.unmount()
  document.body.innerHTML = '<button id="trigger">Open</button><div id="dialog"></div>'
  document.getElementById('trigger').focus()
  window.dialogActions = []
  function Dialog() {
    const [open, setOpen] = React.useState(true)
    window.openDialog = () => setOpen(true)
    return <AppleAlertDialog isOpen={open} title="Cancel meeting" confirmLabel="Cancel meeting" cancelLabel="Keep meeting"
      onClose={() => { window.dialogActions.push('keep'); setOpen(false) }}
      onConfirm={() => { window.dialogActions.push('cancel'); setOpen(false) }}/>
  }
  window.dialogRoot = createRoot(document.getElementById('dialog'))
  window.dialogRoot.render(<Dialog/> )
}
window.makeTerminal = (guarded) => {
  window.term?.dispose()
  document.body.innerHTML = '<div id="desktop" style="width:600px;height:300px"></div>'
  const term = new Terminal()
  term.open(document.getElementById('desktop'))
  window.term = term
  window.sent = []
  term.onData(d => window.sent.push(d))
  if (guarded) trackComposition(term)
  term.attachCustomKeyEventHandler(e => {
    if (guarded && isImeKey(term, e)) return false
    return true
  })
  term.focus()
}
window.desktopNeedsApproval = () => looksLikeApprovalPrompt(readApprovalScreen(window.term))
const messages = new Set(), states = new Set()
const write = Terminal.prototype.write
Terminal.prototype.write = function(...args) { if (this.options.disableStdin) window.mobileTerm = this; return write.apply(this, args) }
window.requests = []
window.deliver = m => messages.forEach(fn => fn(m))
const conn = {
  state: 'open',
  send(m) {
    window.requests.push(m)
    if (m.t === 'resize') setTimeout(() => window.deliver({...m, t:'resized'}), 0)
  },
  onMessage(fn) { messages.add(fn); return () => messages.delete(fn) },
  onState(fn) { states.add(fn); return () => states.delete(fn) }
}
window.renderMobile = (session = {}) => window.mobileRoot.render(<TerminalView conn={conn}
  session={{id:'check', windowId:7, devPort:5173, title:'A'.repeat(300), workspaceName:'workspace', cols:160, rows:40, ...session}}
  hostName="desktop" onBack={() => {}} />)
window.mountMobile = () => {
  window.mobileRoot?.unmount()
  window.term?.dispose()
  window.term = null
  document.body.innerHTML = '<div id="root"></div>'
  window.mobileRoot = createRoot(document.getElementById('root'))
  window.renderMobile()
}
window.mountHome = (activeHost = 'https://desktop:47600/', firstId = 1, workspace = 'C:/projects/business') => {
  window.homeRoot?.unmount()
  window.mobileRoot?.unmount()
  window.mobileRoot = null
  window.mobileTerm = null
  document.body.innerHTML = '<div id="root"></div>'
  window.opened = []; window.newWindows = []
  window.homeRoot = createRoot(document.getElementById('root'))
  window.homeRoot.render(<Home host="DESKTOP-B6JV938" connState="open" loaded activeHost={activeHost}
    sessions={[{id:'long', windowId:firstId, title:'@claude: <command-message>' + 'long-session-name'.repeat(40), launcherKey:'claude', startTime:Date.now(), needsApproval:false},
      {id:'waiting', windowId:2, workspaceName:'Design sandbox', title:'Needs approval', launcherKey:'codex', startTime:Date.now(), needsApproval:true}]}
    windows={[{id:firstId, workspaceName:'Business harness', workspace},
      {id:2, workspaceName:'Design sandbox', workspace:'C:/projects/design'},
      {id:3, workspaceName:'Business harness', workspace:'D:/other/business'}]} workspaces={[]} hosts={[]} tails={{}} busy={{}}
    onOpen={v => window.opened.push(v)} onNew={id => window.newWindows.push(id)} onSettings={() => {}} />)
}
window.mountSettings = () => {
  document.body.innerHTML = '<div id="root"></div>'
  createRoot(document.getElementById('root')).render(<SettingsSheet token="test" host="desktop" connState="open"
    hosts={[]} activeHost="https://localhost:47600/" onHostsChange={() => {}} onSwitchHost={() => {}}
    onSetBypass={() => {}} onClose={() => {}} onUnpair={() => {}} />)
}
window.ready = true
`

app.whenReady().then(async () => {
  const cacheDir = fs.mkdtempSync(path.join(os.tmpdir(), 'workbench-terminal-check-'))
  await require('esbuild').build({
    stdin: { contents: fixture.replaceAll("'/src/", "'./src/"), resolveDir: process.cwd(), loader: 'tsx' },
    bundle: true, platform: 'browser', format: 'iife', jsx: 'automatic',
    plugins: [{ name: 'home-check', setup(build) { build.onLoad({filter: /remote[\\/]App\.tsx$/}, args => ({
      contents: fs.readFileSync(args.path, 'utf8') + '\nexport { Home, SettingsSheet }', loader: 'tsx', resolveDir: path.dirname(args.path)
    })) } }],
    define: { 'process.env.NODE_ENV': '"development"' }, outfile: path.join(cacheDir, 'page.js')
  })
  fs.writeFileSync(path.join(cacheDir, 'index.html'), '<meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="page.css"><script src="page.js"></script>')
  const win = new BrowserWindow({ show: false, width: 390, height: 844, useContentSize: true, webPreferences: { backgroundThrottling: false, offscreen: true } })
  const run = code => win.webContents.executeJavaScript(code)
  const pause = () => new Promise(r => setTimeout(r, 400))
    win.webContents.on('console-message', (_event, level, message) => { if (level >= 2) console.log(message) })
  try {
    await win.loadFile(path.join(cacheDir, 'index.html'))
    const deadline = Date.now() + 30000
    while (!await run('window.ready') && Date.now() < deadline) await new Promise(r => setTimeout(r, 100))
    assert.equal(await run('window.ready'), true, 'fixture loaded')
    for (const guarded of [false, true]) {
      await run(`window.makeTerminal(${guarded})`)
      const result = await run(`(async () => {
        const ta = term.textarea
        ta.dispatchEvent(new CompositionEvent('compositionstart', {bubbles:true}))
        ta.value = '你好'
        ta.dispatchEvent(new CompositionEvent('compositionupdate', {data:'你好', bubbles:true}))
        await new Promise(r => setTimeout(r, 10))
        ta.dispatchEvent(new KeyboardEvent('keydown', {key:' ', keyCode:32, isComposing:true, bubbles:true, cancelable:true}))
        ta.dispatchEvent(new CompositionEvent('compositionend', {data:'你好', bubbles:true}))
        await new Promise(r => setTimeout(r, 20))
        return sent
      })()`)
      console.log(guarded ? 'guarded IME:' : 'stock IME:', JSON.stringify(result))
      if (guarded) assert.deepEqual(result, ['你好'], 'one IME commit, no command Enter or duplicate')
      else assert.deepEqual(result, ['你好', '你好'], 'reproduce duplicate in stock xterm')
    }
    // Native Chromium composition, including a retained field and repeated commits.
    win.webContents.debugger.attach('1.3')
    await run('window.makeTerminal(true); term.textarea.value = "OLD TEXT "')
    for (const text of ['中文測試', '中文測試', '，', '😀']) {
      await win.webContents.debugger.sendCommand('Input.imeSetComposition', {text, selectionStart:text.length, selectionEnd:text.length})
      await win.webContents.debugger.sendCommand('Input.insertText', {text})
      await pause()
    }
    assert.deepEqual(await run('sent'), ['中文測試', '中文測試', '，', '😀'], 'native compositions exactly once')
    // 微軟注音組字緩衝區滿了：先送出前段，後段繼續組字。
    await run('sent.length = 0')
    const ime = (cmd, args) => win.webContents.debugger.sendCommand(cmd, args)
    await ime('Input.imeSetComposition', {text:'一二三四五六', selectionStart:6, selectionEnd:6})
    await pause()
    assert.equal(await run('document.querySelector(".composition-view.active")?.textContent'), '一二三四五六', 'composition preview visible')
    await ime('Input.insertText', {text:'一二三'})
    await ime('Input.imeSetComposition', {text:'四五六七', selectionStart:4, selectionEnd:4})
    await pause()
    await ime('Input.insertText', {text:'四五六七'})
    await pause()
    assert.deepEqual(await run('sent'), ['一二三', '四五六七'], 'IME partial commit sends each part once')
    assert.equal(await run('term.textarea.value'), '', 'no retained IME text')
    // IME passthrough punctuation must not diff/re-emit previously typed content.
    assert.deepEqual(await run(`(async () => {
      sent.length = 0
      const ta = term.textarea
      ta.value = 'already sent'
      ta.dispatchEvent(new KeyboardEvent('keydown', {key:'Process', keyCode:229, bubbles:true, cancelable:true}))
      ta.value = 'already sen。'
      ta.dispatchEvent(new InputEvent('input', {data:'。', inputType:'insertText', bubbles:true, composed:true}))
      await new Promise(r => setTimeout(r, 20))
      return sent
    })()`), ['。'], 'only new IME punctuation, never the old field')
    await run('sent.length = 0; term.textarea.dispatchEvent(new KeyboardEvent("keydown", {key:"Enter", keyCode:13, bubbles:true, cancelable:true}))')
    assert.deepEqual(await run('sent'), ['\r'], 'Enter still submits outside composition')
    await run('sent.length = 0; term.paste("paste once")')
    assert.deepEqual(await run('sent'), ['paste once'], 'paste unchanged')
    console.log('native IME / passthrough / Enter / paste: passed')

    await run(`(async () => {
      await new Promise(r => term.write('\\x1b[H\\x1b[2JI will approve the changes after testing.', r))
    })()`)
    assert.equal(await run('window.desktopNeedsApproval()'), false, 'desktop does not turn ordinary approval prose into a prompt')
    await run(`(async () => {
      await new Promise(r => term.write('\\x1b[H\\x1b[2JAllow command?\\r\\n❯ 1. Yes\\r\\n  2. No\\r\\nEsc to cancel', r))
    })()`)
    assert.equal(await run('window.desktopNeedsApproval()'), true, 'desktop recognizes the actual rendered choice menu')
    await run(`(async () => {
      await new Promise(r => term.write('\\x1b[H\\x1b[2JDone.\\r\\n❯ ', r))
    })()`)
    assert.equal(await run('window.desktopNeedsApproval()'), false, 'desktop clears approval when the prompt is erased')
    console.log('desktop rendered-screen approval / ordinary prose / automatic clearing: passed')

    await run('window.mountMobile()')
    await win.webContents.debugger.sendCommand('Emulation.setTouchEmulationEnabled', {enabled:true})
    await pause()
    await run('Array.from(document.querySelectorAll(".segmented button"))[1].click()')
    await pause()
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40, data:'before switching'}); window.originalMobileTerm = mobileTerm; void 0`)
    await pause()
    await run('window.renderMobile({cols:120, rows:34})')
    await pause()
    assert.equal(await run('mobileTerm === originalMobileTerm'), true, 'state dimension update keeps the terminal parser')
    assert.ok(await run('mobileTerm.buffer.active.getLine(0).translateToString(true).includes("before switching")'), 'state update preserves screen content')
    await run(`window.deliver({t:'resized', id:'check', cols:120, rows:34})`)
    await pause()
    assert.deepEqual(await run('({cols:mobileTerm.cols, rows:mobileTerm.rows})'), {cols:120, rows:34}, 'authoritative desktop resize updates parser without remounting')
    await run('window.renderMobile({id:"other", cols:120, rows:34})')
    await pause()
    await run(`window.deliver({t:'snapshot', id:'other', cols:120, rows:34, data:'other session'})`)
    await pause()
    assert.equal(await run('document.querySelectorAll(".terminal-text").length'), 1, 'switching sessions keeps one output view')
    assert.ok(await run('mobileTerm.buffer.active.getLine(0).translateToString(true).includes("other session")'), 'switched session renders its own snapshot')
    assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("other session")'), 'switched session is visible')
    await run('window.renderMobile()')
    await pause()
    for (const mode of [0, 2, 3]) {
      await run(`document.querySelectorAll('.segmented button')[${mode}].click()`)
      await pause()
      await run(`window.deliver({t:'resized', id:'check', cols:160, rows:40}); window.deliver({t:'snapshot', id:'check', cols:160, rows:40, data:'restored screen'})`)
      await pause()
      await run(`document.querySelectorAll('.segmented button')[1].click()`)
      await pause()
      assert.ok(await run('mobileTerm.buffer.active.getLine(0).translateToString(true).includes("restored screen")'), 'tab return restores snapshot')
      assert.ok(await run('document.querySelector(".terminal-text").getBoundingClientRect().width > 100'), 'tab return keeps full output width')
      assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("restored screen")'), 'tab return shows the restored output')
    }
    await run('window.mountMobile()')
    await pause()
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40, data:'reopened session'}); window.deliver({t:'data', id:'other', d:'wrong session'})`)
    await pause()
    assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("reopened session") && !document.querySelector(".terminal-text").textContent.includes("wrong session")'), 'leaving and reopening a terminal restores only its own snapshot')
    console.log('mobile session switching / state dimensions / hidden tab restore: passed')
    const mobileText = '中文測試 😀 ' + '長文字'.repeat(70) + 'https://example.test/' + 'path'.repeat(80)
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40, data:${JSON.stringify(mobileText)}})`)
    await pause()
    assert.ok(await run(`document.querySelector('.terminal-text').textContent.includes(${JSON.stringify(mobileText)})`), 'CJK, emoji and long URLs survive canonical soft-wraps')
    assert.equal(await run('document.querySelector(".xterm-scroll").scrollWidth'), await run('document.querySelector(".xterm-scroll").clientWidth'), 'long unbroken text wraps without horizontal overflow')
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40,
      data:'\\x1b[?25l\\x1b[31;1mRED\\x1b[0m \\x1b[38;2;12;34;56mRGB\\x1b[0m \\x1b[38;5;196mPALETTE\\x1b[0m <img src=x>\\r\\n' + '─'.repeat(160) + '\\r\\nold progress\\r\\nready'})`)
    await pause()
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[1A\\r\\x1b[2Knew progress\\x1b[1B'})`)
    await pause()
    const styled = await run(`(() => {
      const out = document.querySelector('.terminal-text')
      const spans = [...out.querySelectorAll('span')]
      const style = text => { const s=spans.find(s => s.textContent===text); return s && {color:s.style.color, weight:s.style.fontWeight} }
      return {text:out.textContent, red:style('RED'), rgb:style('RGB'), palette:style('PALETTE'), images:out.querySelectorAll('img').length,
        rule:document.querySelector('.terminal-rule')?.getBoundingClientRect().height ?? -1, lineHeight:parseFloat(getComputedStyle(out).lineHeight),
        cursors:spans.filter(s=>s.style.boxShadow).length}
    })()`)
    assert.ok(styled.text.includes('new progress') && !styled.text.includes('old progress'), 'relative ANSI redraw happens before mobile line wrapping')
    assert.equal(styled.red.weight, 'bold', 'ANSI bold retained')
    assert.equal(styled.rgb.color, 'rgb(12, 34, 56)', 'ANSI truecolor retained')
    assert.equal(styled.palette.color, 'rgb(255, 0, 0)', 'ANSI 256-color palette retained')
    assert.equal(styled.images, 0, 'terminal text cannot inject HTML')
    assert.equal(styled.cursors, 0, 'CLI can hide cursor')
    assert.ok(styled.rule > 0 && styled.rule <= styled.lineHeight + 1, 'desktop separator stays one mobile line: ' + JSON.stringify(styled))
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[?25h'})`)
    await pause()
    assert.equal(await run('[...document.querySelectorAll(".terminal-text span")].filter(s=>s.style.boxShadow).length'), 1, 'CLI can restore cursor')
    fs.writeFileSync(path.join(cacheDir, 'wrapped-terminal.png'), (await win.webContents.capturePage()).toPNG())
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[?1049h\\x1b[2J\\x1b[40;1HALTERNATE SCREEN'})`)
    await pause()
    assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("ALTERNATE SCREEN") && !document.querySelector(".terminal-text").textContent.includes("new progress")'), 'alternate screen uses canonical row coordinates and replaces normal output')
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[?1049l'})`)
    await pause()
    assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("new progress") && !document.querySelector(".terminal-text").textContent.includes("ALTERNATE SCREEN")'), 'leaving alternate screen restores normal output')
    console.log('mobile wrapped CJK / long text / ANSI styles / relative redraw / cursor: passed')
    await run(`window.renderMobile({needsApproval:true})`)
    await pause()
    // Claude Code parks the cursor on its spinner row, above the permission prompt.
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40,
      data:'\x1b[?25l\x1b[3;1H● Writing\x1b[20;2HDo you want to proceed?\x1b[21;2H❯ 1. Yes\x1b[22;4H2. Yes, and always allow\x1b[23;4H3. No\x1b[25;2HEsc to cancel\x1b[3;3H'})`)
    await pause()
    const choices = await run('[...document.querySelectorAll(".approval .choice")].map(b => b.textContent)')
    assert.deepEqual(choices, ['1Yes', '2Yes, and always allow', '3No'], 'approval reads options below a parked cursor: ' + JSON.stringify(choices))
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40,
      data:'Which login method?\\r\\n● Email link\\r\\n○ Company account\\r\\nUse arrow keys to choose'})`)
    await pause()
    assert.equal(await run('document.querySelectorAll(".approval .choice").length'), 0, 'unrecognized formats never show unlabeled 1/2/3 buttons')
    assert.ok(await run('document.querySelector(".approval-details").textContent.includes("● Email link")'), 'unrecognized menu retains original option text')
    assert.ok(await run('document.getElementById(document.querySelector(".approval").getAttribute("aria-labelledby"))'), 'fallback dialog retains an accessible title')
    await run('requests.length = 0; [...document.querySelectorAll(".keys button")].find(b => b.getAttribute("aria-label") === "Down").click(); document.querySelector(".send").click()')
    assert.deepEqual(await run('requests.filter(m => m.t === "input").map(m => m.data)'), ['\x1b[B', '\r'], 'fallback supports arrow selection and Enter confirmation')

    // A full-height question with blank rows and multi-line descriptions exceeds the old 24-row tail.
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40,
      data:'\x1b[?25l\x1b[3;2H要使用哪種登入方式？\x1b[5;2H❯ 1. 電子郵件（建議）\x1b[6;6H使用信箱收取登入連結。\x1b[7;6H不需要記住密碼，\x1b[8;6H適合一般使用者。\x1b[19;4H2. 公司帳號\x1b[20;6H使用公司提供的單一登入。\x1b[32;4H3. 其他方式\x1b[33;6H輸入你偏好的方式。\x1b[38;2HEnter to select · Tab/Arrow keys to navigate · Esc to cancel\x1b[2;1H'})`)
    await pause()
    assert.equal(await run('document.querySelector(".approval-q").textContent'), '要使用哪種登入方式？', 'full-height question is visible')
    assert.deepEqual(await run('[...document.querySelectorAll(".approval .choice")].map(b => b.textContent)'), [
      '1電子郵件（建議） 使用信箱收取登入連結。 不需要記住密碼， 適合一般使用者。',
      '2公司帳號 使用公司提供的單一登入。', '3其他方式 輸入你偏好的方式。'
    ], 'spaced question choices retain labels and descriptions across the full screen')
    await run('requests.length = 0; document.querySelectorAll(".approval .choice")[1].click()')
    await pause()
    assert.deepEqual(await run('requests.filter(m => m.t === "input").map(m => m.data)'), ['2'], 'labeled option sends its original CLI key')
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40,
      data:'第二個問題？\\r\\n❯ 1. 繼續\\r\\n  2. 返回\\r\\nEsc to cancel'})`)
    await pause()
    assert.equal(await run('document.querySelector(".approval-q")?.textContent'), '第二個問題？', 'consecutive questions remain answerable without toggling the waiting state')
    await run(`window.renderMobile()`)
    await pause()
    console.log('approval parked cursor / spaced questions / full-height screen / fallback text and input: passed')
    for (const scale of [1, 2, 3]) {
      for (const width of [320, 390, 768, 320]) {
        win.setContentSize(width, 844)
        await win.webContents.debugger.sendCommand('Emulation.setDeviceMetricsOverride', {width, height:844, deviceScaleFactor:scale, mobile:true})
        await pause()
        await win.webContents.capturePage()
        const bounds = await run(`(() => {
          const host = document.querySelector('.terminal-text')
          const screen = host.getBoundingClientRect()
          return {width:innerWidth, page:document.documentElement.scrollWidth,
            box:host.getBoundingClientRect().right, right:screen.right, screenWidth:screen.width, cols:mobileTerm.cols,
            font:parseFloat(getComputedStyle(host).fontSize),
            scroll:document.querySelector('.xterm-scroll').scrollWidth,
            client:document.querySelector('.xterm-scroll').clientWidth}
        })()`)
        assert.equal(bounds.width, width)
        assert.ok(bounds.page <= width, JSON.stringify(bounds))
        assert.ok(bounds.screenWidth > 0, 'terminal really rendered')
        assert.ok(bounds.screenWidth <= bounds.client + 1, 'terminal screen fits within mobile container: ' + JSON.stringify(bounds))
        assert.ok(bounds.font >= 13, 'terminal stays readable instead of scaling desktop text down')
        assert.equal(bounds.cols, 160, 'phone preserves desktop ANSI column coordinates')
        assert.equal(await run('mobileTerm.rows'), 40, 'phone preserves desktop ANSI row coordinates')
        assert.equal(bounds.scroll, bounds.client, 'terminal never requires horizontal scrolling')
        const keys = await run(`(() => {
          const toolbar = document.querySelector('.keys'), box = toolbar.getBoundingClientRect()
          return {scroll:toolbar.scrollWidth, client:toolbar.clientWidth,
            tops:Array.from(toolbar.children).map(b => b.getBoundingClientRect().top),
            buttons:Array.from(toolbar.children).map(b => {
              const r = b.getBoundingClientRect()
              return r.left >= box.left && r.right <= box.right && r.height >= 44 && r.width >= 24 && r.bottom <= box.bottom && b.scrollWidth <= b.clientWidth
            })}
        })()`)
        assert.equal(keys.scroll, keys.client, 'keys never require horizontal scrolling')
        assert.equal(keys.buttons.length, 8)
        assert.equal(new Set(keys.tops).size, 1, 'all eight keys stay in one row')
        assert.ok(keys.buttons.every(Boolean), 'all eight keys visible and touch-sized')
        console.log(`mobile ${width}px @${scale}x: ${bounds.cols} ANSI columns, readable wrapped text without horizontal scroll`)
      }
    }
    const cols = await run('mobileTerm.cols')
    await run(`window.deliver({t:'snapshot', id:'check', cols:160, rows:40, data:'你好 '+ 'x'.repeat(300)})`)
    await pause()
    assert.equal(await run('mobileTerm.cols'), 160, 'snapshot preserves desktop dimensions')
    assert.ok(await run(`Array.from({length:mobileTerm.buffer.active.length}, (_,i)=>mobileTerm.buffer.active.getLine(i).translateToString(true)).join('').includes('x'.repeat(300))`), 'preserves all output without discarding text')
    assert.ok(await run('document.querySelector(".terminal-text").textContent.includes("x".repeat(300))'), 'desktop soft-wrapped lines join before mobile wrapping')
    await run(`window.deliver({t:'resized', id:'check', cols:160, rows:40})`)
    await pause()
    assert.equal(await run('mobileTerm.cols'), 160, 'desktop remains the source of terminal dimensions')
    assert.equal(await run('requests.filter(m => m.t === "resize").length'), 0, 'viewing/rotation/reconnect never resize desktop PTY')
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[40;150HLAST ROW'})`)
    await pause()
    assert.equal(await run('mobileTerm.buffer.active.getLine(39).translateToString(true).slice(149)'), 'LAST ROW', 'cursor-addressed redraw uses desktop coordinates')
    // Opening the keyboard reduces the display without changing CLI geometry or hiding the last row.
    await win.webContents.debugger.sendCommand('Emulation.setDeviceMetricsOverride', {width:320, height:400, deviceScaleFactor:3, mobile:true})
    await pause()
    const clipped = await run(`(() => {
      const scroll = document.querySelector('.xterm-scroll')
      const screen = document.querySelector('.terminal-line:last-child').getBoundingClientRect()
      return {bottom:screen.bottom, visible:scroll.getBoundingClientRect().bottom, top:scroll.scrollTop}
    })()`)
    assert.ok(clipped.bottom <= clipped.visible, 'latest rows visible with keyboard open: ' + JSON.stringify(clipped))
    await win.webContents.debugger.sendCommand('Emulation.setDeviceMetricsOverride', {width:320, height:844, deviceScaleFactor:3, mobile:true})
    await pause()
    console.log('mobile reconnect / desktop resize: passed')
    await run(`window.deliver({t:'data', id:'check', d:'\\r\\n' + Array.from({length:200}, (_,i) => 'line ' + i).join('\\r\\n')})`)
    await pause()
    const swipe = async yDistance => {
      await win.webContents.debugger.sendCommand('Input.dispatchTouchEvent', {type:'touchStart', touchPoints:[{x:160, y:350}]})
      for (let step = 1; step <= 12; step++) {
        await win.webContents.debugger.sendCommand('Input.dispatchTouchEvent', {
          type:'touchMove', touchPoints:[{x:160, y:350 + yDistance * step / 12}]
        })
        await new Promise(r => setTimeout(r, 25))
      }
      await win.webContents.debugger.sendCommand('Input.dispatchTouchEvent', {type:'touchEnd', touchPoints:[]})
    }
    const position = () => run('document.querySelector(".xterm-scroll").scrollTop')
    for (const mouseMode of [false, true]) {
      await run(`window.deliver({t:'data', id:'check', d:'\\x1b[?1000${mouseMode ? 'h' : 'l'}'}); document.querySelector('.xterm-scroll').scrollTop = 1e9`)
      await pause()
      const bottom = await position()
      await swipe(180)
      await pause()
      const up = await position()
      assert.ok(up < bottom, 'finger swipe reaches history, mouse mode=' + mouseMode)
      assert.ok(await run(`(() => {
        const scroll = document.querySelector('.xterm-scroll').getBoundingClientRect()
        return [...document.querySelectorAll('.terminal-line')].some(line => line.textContent.startsWith('line ') && line.getBoundingClientRect().bottom > scroll.top && line.getBoundingClientRect().top < scroll.bottom)
      })()`), 'swipe actually displays historical rows')
      await run(`window.deliver({t:'data', id:'check', d:'\\r\\nnew output'})`)
      await pause()
      assert.equal(await position(), up, 'new output leaves history in place')
      const history = await run('mobileTerm.buffer.active.baseY')
      await run(`window.deliver({t:'data', id:'check', d:'\\x1b[H\\x1b[2Jredrawn screen'})`)
      await pause()
      assert.equal(await run('mobileTerm.buffer.active.baseY'), history, 'CLI clear-screen redraw retains mobile history')
      assert.equal(await position(), up, 'CLI redraw leaves reader in history')
      await swipe(-180)
      await pause()
      const returned = await run(`(() => {
        const scroll = document.querySelector('.xterm-scroll'), bounds = scroll.getBoundingClientRect()
        const last = document.querySelector('.terminal-line:last-child').getBoundingClientRect()
        return {top:scroll.scrollTop, atBottom:scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop <= 2,
          latestVisible:last.bottom <= bounds.bottom + 1 && last.bottom > bounds.top}
      })()`)
      // A cleared screen can be shorter; reaching its latest text may reduce the absolute offset.
      assert.ok(returned.top > up || (returned.atBottom && returned.latestVisible), 'finger swipe returns toward latest output: ' + JSON.stringify(returned))
      assert.equal(await run('document.querySelector(".session-body").scrollTop'), 0, 'outer body does not compete with terminal scrolling')
    }
    await run(`document.querySelector('.xterm-scroll').scrollTop = 1e9`)
    await pause()
    await run(`window.deliver({t:'data', id:'check', d:'\\x1b[40;1H\\r\\nlatest output'})`)
    await pause()
    const followed = await run(`(() => { const s=document.querySelector('.xterm-scroll'); return {height:s.scrollHeight, client:s.clientHeight, top:s.scrollTop, base:mobileTerm.buffer.active.baseY, viewport:mobileTerm.buffer.active.viewportY} })()`)
    assert.ok(followed.height - followed.client - followed.top <= 2, 'new output follows when reader is at bottom: ' + JSON.stringify(followed))
    assert.ok(await run('document.querySelector(".terminal-line:last-child").textContent.includes("latest output")'), 'bottom renders the latest rows')
    fs.writeFileSync(path.join(cacheDir, 'terminal.png'), (await win.webContents.capturePage()).toPNG())
    console.log('mobile native touch scroll / mouse mode / history during output: passed')
    assert.deepEqual(await run('Array.from(document.querySelectorAll(".segmented button")).map(b => b.textContent)'),
      await run('document.documentElement.lang === "en" ? ["Status", "Terminal", "File", "Preview"] : ["Status", "終端", "File", "預覽"]'))
    await run('document.querySelectorAll(".segmented button")[0].click()')
    await pause()
    await run(`window.deliver({t:'status', windowId:7, status:{workspace:'C:/project', agentBusy:true, agentCount:2,
      devUrl:'http://localhost:5173', changedFiles:['C:/project/README.md']},
      bgTasks:[{id:'bg', agent:'codex', desc:'Build the project', status:'running', startedAt:new Date().toISOString()}]})`)
    assert.ok(await run('document.querySelector(".remote-status").textContent.includes("Build the project")'), 'same background tasks shown in mobile Status')
    assert.ok(await run('document.querySelector(".remote-status").textContent.includes("localhost:5173")'), 'dev server shown')
    await pause()
    fs.writeFileSync(path.join(cacheDir, 'status.png'), (await win.webContents.capturePage()).toPNG())
    await run('document.querySelector(".remote-status .remote-file-row").click()')
    await pause()
    await run(`window.deliver({t:'file', windowId:7, path:'README.md', kind:'markdown', text:'# Mobile document', url:'https://localhost:1/_files/check/README.md'})`)
    assert.equal(await run('document.querySelector(".remote-files h1")?.textContent'), 'Mobile document', 'changed file opens rendered Markdown')
    await pause()
    fs.writeFileSync(path.join(cacheDir, 'file.png'), (await win.webContents.capturePage()).toPNG())
    console.log('mobile screenshots:', cacheDir)
    await run('document.querySelectorAll(".segmented button")[2].click()')
    await pause()
    await run(`window.deliver({t:'files', windowId:7, path:'', entries:[{name:'docs', path:'docs', isDir:true}]})`)
    await run('document.querySelector(".remote-file-list button").click()')
    await pause()
    await run(`window.deliver({t:'files', windowId:7, path:'docs', entries:[{name:'report.pdf', path:'docs/report.pdf', isDir:false}, {name:'view.html', path:'docs/view.html', isDir:false}]})`)
    await run('document.querySelector(".remote-file-list button").click()')
    await pause()
    await run(`window.deliver({t:'file', windowId:7, path:'docs/report.pdf', kind:'pdf', text:null, url:'https://localhost:1/_files/check/docs/report.pdf'})`)
    assert.ok(await run('document.querySelector(".remote-file-frame")?.src.endsWith("report.pdf")'), 'PDF opens in native browser viewer')
    assert.equal(await run('document.querySelector(".remote-file-frame").hasAttribute("sandbox")'), false, 'PDF plugin can load')
    await run('document.querySelector(".remote-file-head button").click()')
    await pause()
    await run(`window.deliver({t:'files', windowId:7, path:'docs', entries:[{name:'view.html', path:'docs/view.html', isDir:false}]})`)
    await run('document.querySelector(".remote-file-list button").click()')
    await pause()
    await run(`window.deliver({t:'file', windowId:7, path:'docs/view.html', kind:'html', text:null, url:'https://localhost:1/_files/check/docs/view.html'})`)
    assert.equal(await run('document.querySelector(".remote-file-frame")?.getAttribute("sandbox")'), 'allow-scripts', 'HTML preview isolated from paired app')
    assert.equal(await run('document.querySelectorAll(".remote-files textarea, .remote-files [contenteditable]").length'), 0, 'files have no editor')
    console.log('mobile Status / file navigation / Markdown / PDF / sandboxed HTML: passed')
    await run('localStorage.removeItem("aw.remote.collapsed-workspaces"); window.mountHome()')
    await pause()
    await win.webContents.capturePage()
    const home = await run(`({page:document.documentElement.scrollWidth, width:innerWidth,
      card:document.querySelector('.session-card').getBoundingClientRect().right,
      add:document.querySelector('.add-card').getBoundingClientRect().right,
      title:document.querySelector('.session-card .card-title').scrollWidth,
      visible:document.querySelector('.session-card .card-title').clientWidth})`)
    assert.ok(home.title > home.visible, 'long title needs ellipsis')
    assert.ok(home.page <= home.width && home.card <= home.width - 16 && home.add <= home.width - 16, 'home card and row fit: ' + JSON.stringify(home))
    assert.equal(await run('getComputedStyle(document.querySelector(".session-card")).minWidth'), '0px', 'button intrinsic width cannot widen the grid')
    console.log('home long session name: passed')
    for (const width of [320, 390, 768]) {
      await win.webContents.debugger.sendCommand('Emulation.setDeviceMetricsOverride', {width, height:844, deviceScaleFactor:3, mobile:true})
      await pause()
      const headers = await run(`({width:innerWidth, page:document.documentElement.scrollWidth,
        bounds:Array.from(document.querySelectorAll('.workspace-summary')).map(el => el.getBoundingClientRect().toJSON())})`)
      assert.ok(headers.bounds.every(r => r.height >= 44 && r.left >= 15 && r.right <= headers.width - 15) && headers.page <= headers.width,
        'workspace headers fit and have 44px touch targets at ' + width + ': ' + JSON.stringify(headers))
    }
    await run('document.querySelectorAll(".workspace-summary")[1].click()')
    await pause()
    assert.deepEqual(await run('Array.from(document.querySelectorAll(".workspace-group")).map(el => el.open)'), [true, false, true], 'workspace folds independently')
    assert.equal(await run('document.querySelectorAll(".workspace-group")[1].querySelector(".session-card").checkVisibility()'), false, 'folded sessions hidden')
    assert.ok(await run('document.querySelectorAll(".workspace-summary")[1].querySelector(".pill.wait")?.textContent.includes("1")'), 'waiting badge remains in folded header')
    await run('document.querySelector(".wait-card").click()')
    assert.deepEqual(await run('opened'), [{kind:'session', id:'waiting'}], 'approval remains reachable while workspace is folded')
    await run('document.querySelectorAll(".workspace-summary")[0].click(); document.querySelectorAll(".workspace-summary")[2].click()')
    await pause()
    await run('window.mountHome("https://desktop:47600/", 11, "c:/PROJECTS/BUSINESS")')
    await pause()
    assert.deepEqual(await run('Array.from(document.querySelectorAll(".workspace-group")).map(el => el.open)'), [false, false, false], 'folding survives navigation, new window ids, and path casing')
    await win.webContents.debugger.sendCommand('Emulation.setFocusEmulationEnabled', {enabled:true})
    await run('document.querySelector(".workspace-summary").focus()')
    await win.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {type:'keyDown', key:'Enter', code:'Enter', windowsVirtualKeyCode:13, text:'\r'})
    await win.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {type:'keyUp', key:'Enter', code:'Enter', windowsVirtualKeyCode:13})
    await pause()
    assert.equal(await run('document.querySelector(".workspace-group").open'), true, 'native disclosure supports keyboard Enter')
    await run('document.querySelector(".workspace-group .add-card").click()')
    assert.deepEqual(await run('newWindows'), [11], 'expanded group keeps correct workspace actions')
    await run('window.mountHome("https://other-computer:47600/")')
    await pause()
    assert.deepEqual(await run('Array.from(document.querySelectorAll(".workspace-group")).map(el => el.open)'), [true, true, true], 'other computer has independent folding')
    await run('localStorage.setItem("aw.remote.collapsed-workspaces", "{}"); window.mountHome()')
    await pause()
    assert.equal(await run('document.querySelector(".workspace-group").open'), true, 'invalid saved preference safely defaults to expanded')
    await win.webContents.debugger.sendCommand('Emulation.setDeviceMetricsOverride', {width:320, height:844, deviceScaleFactor:3, mobile:true})
    await run('document.querySelectorAll(".workspace-summary").forEach(el => el.click())')
    await pause()
    fs.writeFileSync(path.join(cacheDir, 'workspaces.png'), (await win.webContents.capturePage()).toPNG())
    console.log('workspace native folding / persistence / host isolation / approval / keyboard: passed')
    await run('window.mountSettings()')
    await pause()
    assert.ok(await run(`document.body.textContent.includes(${JSON.stringify('v' + version)})`), 'show loaded mobile interface version')
    assert.ok(await run('Array.from(document.querySelectorAll("button")).some(b => /Reload mobile interface|重新載入手機介面/.test(b.textContent))'), 'standalone PWA has a reload action')
    console.log('mobile interface version / reload control: passed')
    await run('window.mountDialog()')
    await pause()
    assert.equal(await run('document.activeElement.textContent'), 'Keep meeting', 'destructive dialog focuses the safe action')
    await win.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {type:'keyDown', key:'Enter', code:'Enter', windowsVirtualKeyCode:13, text:'\r'})
    await win.webContents.debugger.sendCommand('Input.dispatchKeyEvent', {type:'keyUp', key:'Enter', code:'Enter', windowsVirtualKeyCode:13})
    await pause()
    assert.deepEqual(await run('window.dialogActions'), ['keep'], 'Enter activates the focused safe button')
    assert.equal(await run('document.activeElement.id'), 'trigger', 'dialog restores focus')
    await run('window.openDialog()')
    await pause()
    await run('document.activeElement.dispatchEvent(new KeyboardEvent("keydown",{key:"Tab",shiftKey:true,bubbles:true,cancelable:true}))')
    assert.equal(await run('document.activeElement.textContent'), 'Cancel meeting', 'Shift+Tab stays in dialog')
    await run('document.activeElement.dispatchEvent(new KeyboardEvent("keydown",{key:"Tab",bubbles:true,cancelable:true}))')
    assert.equal(await run('document.activeElement.textContent'), 'Keep meeting', 'Tab wraps within dialog')
    await run('document.activeElement.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}))')
    await pause()
    assert.equal(await run('!!document.querySelector("[role=alertdialog]")'), false, 'Escape dismisses dialog')
    await run('window.openDialog()')
    await pause()
    await run('document.querySelector(".apple-alert-btn-destructive").click()')
    await pause()
    assert.deepEqual(await run('window.dialogActions'), ['keep', 'keep', 'cancel'], 'explicit confirm performs destructive action')
    console.log('Apple alert focus / keyboard / safe action: passed')
  } finally {
    win.destroy()
  }
  app.quit()
}).catch(err => { console.error(err); app.exit(1) })
