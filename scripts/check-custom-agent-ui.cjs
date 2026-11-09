// node scripts/check-custom-agent-ui.cjs
const { app, BrowserWindow } = require('./lib/browser-window.cjs')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'workbench-custom-ui-'))
app.setPath('userData', path.join(dir, 'userData'))
app.on('window-all-closed', () => {})

const fixture = `
import React from 'react'
import { createRoot } from 'react-dom/client'
import SettingsModal from './src/renderer/src/components/SettingsModal'
import TerminalPanel from './src/renderer/src/panels/terminal/TerminalPanel'
import CustomAgentExtensions from './src/renderer/src/panels/customized/CustomAgentExtensions'
import { bumpSettings, setLanguage, setTheme } from './src/renderer/src/store'
import './src/renderer/src/styles.css'
import './src/renderer/src/panels/customized/customized.css'
let root
const files = new Map()
window.config = { cliPaths:{}, cliEnabled:{}, customAgent:{enabled:false,name:'My Harness',command:'my-harness',args:['--profile','my profile'],
  resources:{skill:'C:/test/skills',plugin:'C:/test/plugins',mcp:'C:/test/mcp.json',hook:'C:/test/hooks.json'}} }
window.spawns = []; window.writes = []; window.closed = false
window.api = {
  settings: {get:async()=>structuredClone(window.config), set:async patch=>{window.config={...window.config,...patch};return window.config}, testCliPath:async()=>({ok:true,version:'Harness 1.0'})},
  files:{detectDocTools:async()=>({}),pickExecutable:async()=> 'C:/my harness/agent.exe'},
  updater:{getStatus:async()=>({status:'idle'}),onStatusChange:()=>()=>{}},
  ext:{agents:async()=>[],inventory:async()=>[],
    customList:async kind=>(kind==='mcp'||kind==='hook')?[kind==='mcp'?'mcp.json':'hooks.json']:[...files.keys()].filter(k=>k.startsWith(kind+':')).map(k=>k.slice(kind.length+1)),
    customRead:async(kind,file)=>files.get(kind+':'+file)??null,
    customWrite:async(kind,file,content,before)=>{window.writes.push({kind,file,content,before});files.set(kind+':'+file,content)}},
  conn:{list:async()=>[]}, notify:{show(){}}, window:{setTitleBarTheme(){}}, memsync:{pluginState:async()=>({ok:false})},
  pty:{launchers:async()=>[],onRemoteSpawned:()=>()=>{},spawn:async opts=>{window.spawns.push(opts);return 'test-pty'},
    resize(){},write(){},kill(){},onData:()=>()=>{},onResized:()=>()=>{},onExit:()=>()=>{}}
}
window.mount = mode => {
  root?.unmount()
  document.body.innerHTML='<div id="root" style="height:100vh"></div>'
  root=createRoot(document.getElementById('root'))
  root.render(mode==='settings'?<SettingsModal isOpen initialTab="cli" onClose={()=>window.closed=true}/>:
    mode==='terminal'?<TerminalPanel/>:<div className="cz-root"><CustomAgentExtensions/></div>)
}
window.toggleConfig = enabled => { window.config.customAgent.enabled=enabled; bumpSettings() }
window.language = setLanguage; window.theme = setTheme
window.input = (selector,value) => {
  const el=document.querySelector(selector)
  Object.getOwnPropertyDescriptor(el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(el,value)
  el.dispatchEvent(new Event('input',{bubbles:true}))
}
window.clickText = (selector,text) => { const el=Array.from(document.querySelectorAll(selector)).find(el=>el.textContent.trim()===text); if(!el)throw Error('Missing '+text);el.click() }
window.ready=true
`

app.whenReady().then(async () => {
  await require('esbuild').build({stdin:{contents:fixture,resolveDir:process.cwd(),loader:'tsx'},
    bundle:true,platform:'browser',format:'iife',jsx:'automatic',define:{'process.env.NODE_ENV':'"development"'},outfile:path.join(dir,'page.js')})
  fs.writeFileSync(path.join(dir,'index.html'), '<meta charset="UTF-8"><link rel="stylesheet" href="page.css"><script src="page.js"></script>')
  const win = new BrowserWindow({ show:false, width:1100, height:900, webPreferences:{backgroundThrottling:false,offscreen:true} })
  const run = async code => {
    try { return await win.webContents.executeJavaScript(code) }
    catch (e) { console.error('UI action failed:', code, errors); throw e }
  }
  const pause = () => win.webContents.settled(250)
  const errors = []
  win.webContents.on('console-message', (_e, level, message) => { if (level >= 3) errors.push(message) })
  try {
    await win.loadFile(path.join(dir,'index.html')); await pause()
    assert.equal(await run('window.ready'), true)
    await run('language("en");mount("settings")'); await pause()
    assert.equal(await run('!!document.querySelector(".custom-agent-settings input[type=checkbox]")'), true)
    assert.equal(await run('!!document.querySelector(".custom-agent-settings .macos-row-drawer")'), false)
    await run('document.querySelector(".custom-agent-settings input[type=checkbox]").click()'); await pause()
    assert.equal(await run('document.querySelectorAll(".custom-agent-field").length'), 7)
    await run('input(".custom-agent-field input", "My Saved Harness")'); await pause()
    await run('document.querySelector(".macos-settings-footer .macos-btn-primary").click()')
    await pause()
    assert.equal(await run('config.customAgent.enabled'), true)
    assert.equal(await run('config.customAgent.name'), 'My Saved Harness')

    await run('mount("terminal")'); await pause()
    assert.equal(await run('Array.from(document.querySelectorAll(".launchpad-card")).filter(e=>e.textContent.includes("My Saved Harness")).length'), 1)
    await run('Array.from(document.querySelectorAll(".launchpad-card")).find(e=>e.textContent.includes("My Saved Harness")).click()'); await pause()
    assert.equal(await run('spawns.at(-1).command'), 'custom-agent')
    await run('mount("extensions")'); await pause()
    assert.equal(await run('document.querySelectorAll(".custom-agent-extensions [aria-pressed]").length'), 4)
    for (const [kind, file, content] of [['Skills','demo/SKILL.md','# Demo'],['Plugins','demo/plugin.json','{"name":"demo"}'],['MCP','mcp.json','{"mcpServers":{}}'],['Hooks','hooks.json','{"hooks":{}}']]) {
      await run(`clickText('.custom-agent-extensions button', ${JSON.stringify(kind)})`); await pause()
      if (kind==='Skills'||kind==='Plugins') await run(`input('.custom-agent-file-label input',${JSON.stringify(file)})`)
      await pause(); await run('clickText(".custom-agent-extensions button", "Open / Create")'); await pause()
      await run(`input('.custom-agent-editor',${JSON.stringify(content)})`); await pause()
      assert.equal(await run('Array.from(document.querySelectorAll("[aria-pressed]")).every(e=>e.disabled)'), true, 'draft cannot be lost by switching kind')
      await run('clickText(".custom-agent-extensions button", "Save")'); await pause()
      assert.equal(await run('writes.at(-1).file'), file)
      assert.equal(await run('writes.at(-1).content'), content)
    }
    assert.deepEqual(await run('writes.map(w=>w.kind)'), ['skill','plugin','mcp','hook'])
    for (const lang of ['en','zh-TW']) for (const theme of ['light','dark']) {
      await run(`language(${JSON.stringify(lang)});theme(${JSON.stringify(theme)})`); await pause()
      fs.writeFileSync(path.join(dir,`extensions-${lang}-${theme}.png`),(await win.webContents.capturePage()).toPNG())
    }
    await run('language("en");toggleConfig(false)'); await pause()
    assert.equal(await run('!!document.querySelector(".custom-agent-extensions")'), false)
    await run('mount("terminal")'); await pause()
    assert.equal(await run('document.body.textContent.includes("My Saved Harness")'), false)
    await run('mount("settings")'); await pause()
    assert.equal(await run('!!document.querySelector(".custom-agent-settings .macos-row-drawer")'), false)
    await run('toggleConfig(true);mount("settings")'); await pause()
    await run('document.querySelector(".custom-agent-settings").scrollIntoView({block:"end"})'); await pause()
    fs.writeFileSync(path.join(dir,'settings.png'),(await win.webContents.capturePage()).toPNG())
    assert.deepEqual(errors, [])
    console.log('custom agent UI: settings save, opt-in visibility, terminal launch, four extension editors, languages/themes passed')
    console.log('Screenshots:',dir)
  } finally { win.destroy() }
  app.quit()
}).catch(e=>{console.error(e);app.exit(1)})
