// node --experimental-strip-types scripts/check-approval.mts
// Approval detection through a real terminal: what matters is what the screen shows now, not what once went by.
import assert from 'node:assert/strict'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { Backend, until } from './lib/core-call.mts'

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-approval-'))
const data = path.join(tmp, 'data')
fs.mkdirSync(data)
const argsLog = path.join(tmp, 'args.log')
const script = path.resolve('scripts/fixtures/pty-script-cli.mjs')
const cmd = path.join(tmp, 'fake.cmd')
fs.writeFileSync(cmd, `@"${process.execPath}" "${script}" %*\r\n`)
fs.writeFileSync(path.join(data, 'settings.json'), JSON.stringify({ cliPaths: { claude: cmd, codex: cmd }, cliEnabled: {} }))
const b = new Backend({ WB_DATA_DIR: data, USERPROFILE: path.join(tmp, 'home'), PTY_SCRIPT_ARGS: argsLog })
const ws = path.join(tmp, 'ws')
fs.mkdirSync(ws)

const session = async (id: string): Promise<any> => (await b.ipc('test:ptyInfo', id))
const CLEAR = '\x1b[H\x1b[2J'
const pause = (): Promise<void> => new Promise((r) => setTimeout(r, 500))

try {
  await b.ipc('test:openWindow', ws)
  for (const agent of ['claude', 'codex']) {
    const id = await b.ipc('pty:spawn', { command: agent, cols: 120, rows: 40 })
    const out = (s: string): Promise<unknown> => b.ipc('pty:write', id, `@${Buffer.from(s).toString('base64')}\r`)
    const alerts = (): number => b.events.filter((e) => e.event === 'test:ptyEvent' && e.kind === 'approval' && e.id === id).length
    const approval = (want: boolean, why: string): Promise<void> => until(async () => (await session(id)).needsApproval === want, `${agent}: ${why}`)
    await pause()
    await pause()

    await out('I will approve the changes after the tests pass.\r\nWorking...')
    await pause()
    assert.equal((await session(id)).needsApproval, false, `${agent}: ordinary output does not latch`)

    const marker = agent === 'claude' ? '❯' : '›'
    // Separate chunks, split escape sequences, and a cursor parked above the actual menu
    await out(CLEAR + '\x1b[3;1HDo you want to pro')
    await out(`ceed?\x1b[5;2H${marker} 1. Yes\x1b[6;4H2. No\x1b[8;2HEsc to cancel\x1b[1;`)
    await out('1H')
    await approval(true, 'a rendered prompt across chunks is recognized')
    await until(() => alerts() === 1, `${agent}: one alert, have ${alerts()}`)

    await b.ipc('pty:write', id, '\x1b[B')
    await pause()
    assert.equal((await session(id)).needsApproval, true, `${agent}: arrow navigation is not an answer`)
    await out(CLEAR + `\x1b[3;1HDo you want to proceed?\x1b[5;4H1. Yes\x1b[6;2H${marker} 2. No\x1b[8;2HEsc to cancel`)
    await pause()
    assert.equal(alerts(), 1, `${agent}: navigation and redraw do not alert again`)

    await out(CLEAR + 'Continuing automatically...\r\nTests passed.\r\n' + marker + ' ')
    await approval(false, 'a prompt that disappears clears without any input')
    assert.equal(alerts(), 1)
    assert.ok(!(await b.ipc('test:ptyScreen', id)).includes('Do you want to proceed?'), 'the text for a notification excludes what was erased')

    await out(CLEAR + '1. First result\r\n2. Second result\r\n\r\nOverwrite file? [y/N]')
    await approval(true, 'a real y/n prompt')
    await out(CLEAR + 'Done.\r\n' + marker + ' ')
    await approval(false, 'answered')

    // Old menus in scrollback or inside quoted code never re-arm it
    await out(CLEAR + 'Do you want to proceed?\r\n' + marker + ' 1. Yes\r\n  2. No\r\n' + '\r\n'.repeat(45) + 'Done.\r\n' + marker + ' ')
    await pause()
    assert.equal((await session(id)).needsApproval, false, `${agent}: scrollback`)
    await out(CLEAR + 'Example:\r\n```text\r\nAllow command?\r\n' + marker + ' 1. Yes\r\n  2. No\r\n```')
    await pause()
    assert.equal((await session(id)).needsApproval, false, `${agent}: quoted code`)
    assert.equal(alerts(), 2, `${agent}: only the two real prompts alert`)
    await b.ipc('pty:kill', id)
  }
  console.log('approval: ordinary output, live menus, split chunks, navigation, automatic clearing, history')

  // Codex stays inline, whatever it was asked to do; others are left alone
  const started = async (o: unknown): Promise<string[]> => {
    fs.rmSync(argsLog, { force: true })
    const id = await b.ipc('pty:spawn', o)
    await until(() => fs.existsSync(argsLog), 'the CLI started')
    await b.ipc('pty:kill', id)
    return JSON.parse(fs.readFileSync(argsLog, 'utf8').trim())
  }
  assert.deepEqual(await started({ command: 'codex' }), ['--no-alt-screen'])
  assert.deepEqual(await started({ command: 'codex', args: ['resume', '123'] }), ['resume', '123', '--no-alt-screen'])
  assert.deepEqual(await started({ command: 'codex', args: ['--no-alt-screen'] }), ['--no-alt-screen'])
  assert.deepEqual(await started({ command: 'claude', args: ['--resume', '123'] }), ['--resume', '123'])
  console.log('codex --no-alt-screen default')
} finally {
  b.stop()
  await new Promise((r) => setTimeout(r, 300))
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 3 })
}
