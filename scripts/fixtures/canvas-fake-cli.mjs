// A stand-in CLI for check-canvas.mts, started through cliPaths as "claude". Reads keys raw; each command is one line:
//   ask             draw a Claude-style approval menu; Enter prints ANSWER:yes, Esc prints ANSWER:no
//   hook <file>     run the PermissionRequest hook from --settings like Claude does, with a Write of <file>; print HOOK:<stdout>
//   bracketed       turn on bracketed paste (DECSET 2004)
// Everything typed is echoed, so a check can see whether a paste arrived and whether Enter came with it.
import { spawn } from 'node:child_process'
import fs from 'node:fs'

const settingsAt = process.argv.indexOf('--settings')
const settings = settingsAt >= 0 ? JSON.parse(fs.readFileSync(process.argv[settingsAt + 1], 'utf8')) : null
const hookCmd = settings?.hooks?.PermissionRequest?.[0]?.hooks?.[0]?.command
const out = (s) => process.stdout.write(s)
out(`FAKE-CLI ready hook=${hookCmd ? 'yes' : 'no'} pipe=${process.env.WB_HOOK_PIPE ? 'yes' : 'no'}\r\n> `)

process.stdin.setRawMode?.(true)
process.stdin.resume()
let line = ''
let asking = false
process.stdin.on('data', (d) => {
  for (const ch of d.toString()) {
    if (asking) {
      if (ch === '\r') {
        asking = false
        out('\r\nANSWER:yes\r\n> ')
      } else if (ch === '\x1b') {
        asking = false
        out('\r\nANSWER:no\r\n> ')
      }
      continue
    }
    if (ch === '\r') {
      run(line.trim())
      line = ''
    } else if (ch >= ' ' || ch === '\n') {
      // Printable only: echoing a lone ESC (the stop key, paste brackets) would swallow the next output as a sequence
      line += ch
      out(ch === '\n' ? '\r\n' : ch)
    }
  }
})

function run(cmd) {
  out('\r\n')
  if (cmd === 'ask') {
    asking = true
    out(' Do you want to create a.txt?\r\n ❯ 1. Yes\r\n   2. No\r\n\r\n Esc to cancel')
    return
  }
  if (cmd === 'bracketed') {
    out('\x1b[?2004h> ')
    return
  }
  if (cmd.startsWith('hook ') && hookCmd) {
    const file = cmd.slice(5)
    const child = spawn(hookCmd, { shell: true, stdio: ['pipe', 'pipe', 'inherit'] })
    let got = ''
    child.stdout.on('data', (b) => (got += b))
    child.on('close', () => out(`HOOK:${got || 'PASS'}\r\n> `))
    child.stdin.end(JSON.stringify({ session_id: 's', hook_event_name: 'PermissionRequest', tool_name: 'Write', tool_input: { file_path: file, content: 'hi\nthere' } }))
    return
  }
  out(`echo:${cmd}\r\n> `)
}
