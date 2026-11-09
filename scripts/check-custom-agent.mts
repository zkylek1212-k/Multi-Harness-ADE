// node --experimental-strip-types scripts/check-custom-agent.mts
// A locally configured CLI: settings isolation, opt-in, how it starts, its four kinds of resources and their path boundaries.
import assert from 'node:assert/strict'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, until } from './lib/core-call.mts'
import { CUSTOM_AGENT_ID, CUSTOM_RESOURCE_KINDS, sanitizeCustomAgent } from '../src/shared/customAgent.ts'

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-custom-agent-'))
const data = path.join(tmp, 'data')
const repo = path.join(tmp, 'repo')
fs.mkdirSync(data)
fs.mkdirSync(repo)
const argsLog = path.join(tmp, 'args.log')
const printer = path.join(tmp, 'print-args.mjs')
fs.writeFileSync(printer, `import fs from 'node:fs'; fs.appendFileSync(${JSON.stringify(argsLog)}, JSON.stringify(process.argv.slice(2)) + '\\n'); setInterval(() => {}, 1000)\n`)
const b = new Backend({ WB_DATA_DIR: data, USERPROFILE: path.join(tmp, 'home') })
const settingsFile = path.join(data, 'settings.json')
const readSettings = (): any => b.ipc('settings:get')

const started = async (o: unknown): Promise<string[]> => {
  fs.rmSync(argsLog, { force: true })
  const id = await b.ipc('pty:spawn', o)
  // The first PowerShell on a cold machine can take a while. On failure, show what the terminal says.
  try {
    await until(() => fs.existsSync(argsLog), 'the program started', 60000)
  } catch (e) {
    throw new Error(`${(e as Error).message}: ${JSON.stringify(o)}, screen: ${await b.ipc('test:ptyScreen', id)}`)
  }
  await b.ipc('pty:kill', id)
  return JSON.parse(fs.readFileSync(argsLog, 'utf8').trim())
}

try {
  await b.ipc('test:openWindow', repo)
  const defaults = sanitizeCustomAgent(null)
  assert.equal(defaults.enabled, false)
  assert.deepEqual(await b.ipc('pty:launchers'), [])
  const config = sanitizeCustomAgent({
    enabled: true, name: 'My Harness', command: process.execPath, args: [printer, 'path with spaces', ''],
    resources: { skill: path.join(tmp, 'skills'), plugin: path.join(tmp, 'plugins'), mcp: path.join(tmp, 'config', 'mcp.json'), hook: path.join(tmp, 'config', 'hooks.json') }
  })
  assert.deepEqual(config.args, [printer, 'path with spaces'])
  await assert.rejects(b.ipc('settings:set', { customAgent: { ...config, command: '' } }), /executable/)
  await assert.rejects(b.ipc('settings:set', { customAgent: { ...config, resources: { ...config.resources, skill: '../outside' } } }), /absolute/)
  await b.ipc('settings:set', { customAgent: config, cliBypassPermissions: true })
  assert.deepEqual((await readSettings()).customAgent, config)
  const wsFile = path.join(repo, '.workbench', 'settings.json')
  const wsSettings = JSON.parse(fs.readFileSync(wsFile, 'utf8'))
  assert.equal(wsSettings.customAgent, undefined, 'custom commands are not synced into repositories')
  fs.writeFileSync(wsFile, JSON.stringify({ ...wsSettings, customAgent: { ...config, command: 'malicious', enabled: false } }))
  assert.deepEqual((await readSettings()).customAgent, config, 'a repository cannot override command, paths or opt-in')
  assert.ok(fs.existsSync(settingsFile))

  // A repository launcher cannot impersonate the locally configured harness
  fs.mkdirSync(path.join(repo, 'agents'))
  fs.writeFileSync(path.join(repo, 'agents', 'spoof.yaml'), 'launcher:\n  id: custom-agent\n  cli: codex\n')
  const launchers = await b.ipc('pty:launchers')
  assert.equal(launchers.length, 1)
  assert.equal(launchers[0].id, CUSTOM_AGENT_ID)
  assert.equal(launchers[0].command, process.execPath)

  // Started as configured, spaces intact, no provider flags even with bypass on
  assert.deepEqual(await started({ command: CUSTOM_AGENT_ID, args: ['--extra'] }), ['path with spaces', '--extra'])
  assert.deepEqual(await started({ launcherId: CUSTOM_AGENT_ID }), ['path with spaces'])
  for (const [ext, body] of [['.cmd', `@"${process.execPath}" "${printer}" %*\r\n`], ['.ps1', `& "${process.execPath}" "${printer}" @args\r\n`]] as const) {
    const command = path.join(tmp, `codex custom harness${ext}`)
    fs.writeFileSync(command, body)
    await b.ipc('settings:set', { customAgent: { ...config, command, args: ['path with spaces'] } })
    assert.deepEqual(await started({ launcherId: CUSTOM_AGENT_ID }), ['path with spaces'], ext)
  }
  await b.ipc('settings:set', { customAgent: config, cliBypassPermissions: false })

  const read = (kind: string, file: string): Promise<string | null> => b.ipc('ext:customRead', kind, file)
  const write = (kind: string, file: string, content: string, before: string | null): Promise<void> => b.ipc('ext:customWrite', kind, file, content, before)
  for (const kind of CUSTOM_RESOURCE_KINDS) {
    const file = kind === 'skill' ? 'demo/SKILL.md' : kind === 'plugin' ? 'demo/plugin.json' : path.basename(config.resources[kind])
    const first = kind === 'skill' ? '# demo\n' : '{"existing":true}\n'
    assert.equal(await read(kind, file), null)
    await write(kind, file, first, null)
    assert.equal(await read(kind, file), first)
    assert.ok((await b.ipc('ext:customList', kind)).includes(file))
    await assert.rejects(write(kind, file, first, null), /changed on disk/)
    const second = kind === 'skill' ? '# changed\n' : '{"existing":true,"new":1}\n'
    await write(kind, file, second, first)
    const target = kind === 'skill' || kind === 'plugin' ? path.join(config.resources[kind], file) : config.resources[kind]
    assert.equal(fs.readFileSync(`${target}.workbench-backup`, 'utf8'), first)
    assert.equal((await b.ipc('ext:customList', kind)).some((f: string) => f.endsWith('.workbench-backup')), false)
    if (kind !== 'skill') {
      await assert.rejects(write(kind, file, '{bad', second))
      assert.equal(fs.readFileSync(target, 'utf8'), second)
    }
    await assert.rejects(read(kind, '../escape'), /relative/)
    await assert.rejects(read(kind, 'demo:escape'), /relative/)
  }
  await assert.rejects(read('mcp', 'other.json'), /configured file/)
  fs.writeFileSync(path.join(config.resources.skill, 'binary'), Buffer.from([0, 1, 2]))
  await assert.rejects(read('skill', 'binary'), /UTF-8/)
  fs.writeFileSync(path.join(config.resources.skill, 'huge'), 'x'.repeat(1024 * 1024 + 1))
  await assert.rejects(read('skill', 'huge'), /1 MB/)
  const outside = path.join(tmp, 'outside')
  fs.mkdirSync(outside)
  fs.symlinkSync(outside, path.join(config.resources.skill, 'linked'), 'junction')
  await assert.rejects(write('skill', 'linked/escape.md', 'escape', null), /Symbolic/)
  assert.equal(fs.existsSync(path.join(outside, 'escape.md')), false)
  fs.writeFileSync(path.join(outside, 'keep.txt'), 'keep')
  await assert.rejects(read('skill', 'demo/SKILL.md.WORKBENCH-BACKUP'), /read-only/)
  const backup = path.join(config.resources.skill, 'demo', 'SKILL.md.workbench-backup')
  fs.unlinkSync(backup)
  fs.linkSync(path.join(outside, 'keep.txt'), backup)
  await assert.rejects(write('skill', 'demo/SKILL.md', '# new', '# changed\n'), /hard link/)
  assert.equal(fs.readFileSync(path.join(outside, 'keep.txt'), 'utf8'), 'keep')

  await b.ipc('settings:set', { customAgent: { ...config, enabled: false } })
  assert.equal((await b.ipc('pty:launchers')).length, 0)
  await assert.rejects(b.ipc('pty:spawn', { launcherId: CUSTOM_AGENT_ID }), /disabled/)
  await assert.rejects(b.ipc('pty:spawn', { command: CUSTOM_AGENT_ID }), /disabled/)
  await assert.rejects(read('skill', 'demo/SKILL.md'), /disabled/)
  assert.equal((await readSettings()).customAgent.resources.hook, config.resources.hook, 'disabling keeps the configuration')
  console.log('custom agent: settings isolation, opt-in, how it starts, four resource kinds, backups, conflicts and path boundaries')
} finally {
  b.stop()
  await new Promise((r) => setTimeout(r, 300))
  try {
    fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3 })
  } catch {
    /* Windows sometimes still holds a handle */
  }
}
