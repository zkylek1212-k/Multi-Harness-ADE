// Does this Codex still speak what the chat-node harness uses (cowork-agui-implement.md §4.2)? Run after a Codex upgrade,
// or against a fork: npx tsx scripts/check-codex-protocol.mts [path-to-codex]. The app checks the same list before a
// chat starts (canvas/chat.rs codex_protocol_ok); this is the same check from the command line.
// Needs: cargo build -p wb-ipc. Starts no AI session: generate-json-schema only describes the protocol.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { coreCall } from './lib/core-call.mts'

const codex = process.argv[2] || 'codex'
const out = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-codex-schema-'))
try {
  // npm installs codex as a .cmd shim, which Windows only runs through cmd.exe
  const viaCmd = process.platform === 'win32' && !codex.toLowerCase().endsWith('.exe')
  const run = (args: string[]): string => execFileSync(viaCmd ? 'cmd.exe' : codex, viaCmd ? ['/d', '/c', codex, ...args] : args, { encoding: 'utf8', stdio: 'pipe' })
  run(['app-server', 'generate-json-schema', '--out', out])
  const version = run(['--version']).trim()
  const missing: string[] = coreCall('codexSchemaMissing', out)
  assert.deepEqual(missing, [], `${version} lacks: ${missing.join(', ')}`)
  console.log(`codex protocol: ${version} has everything the chat harness uses ok`)
} finally {
  fs.rmSync(out, { recursive: true, force: true })
}
