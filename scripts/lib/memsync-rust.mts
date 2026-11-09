// The memory sync API of check-memsync.mts on the Rust implementation. Calls that can reach outside go through a running
// wb-ipc, with each git, gh or HTTP request coming back here so the test's own fake GitHub answers it.
import { execFileSync, spawn } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { createInterface } from 'node:readline'

const target = process.env.CARGO_TARGET_DIR || path.join(process.env.LOCALAPPDATA || '', 'wb-cargo-target')
const exe = process.env.WB_IPC_PATH || path.join(target, 'debug', 'wb-ipc.exe')
const data = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-memsync-'))

export class MemSyncError extends Error {
  code: string
  constructor(code: string, message?: string) {
    super(message || code)
    this.code = code
  }
}

const unwrap = (r: any): any => {
  if (!r.ok) throw new MemSyncError(r.code, r.message)
  return r.value
}

const child = spawn(exe, [], { env: { ...process.env, WB_DATA_DIR: data }, stdio: ['pipe', 'pipe', 'inherit'] })
let nextId = 1
const waiting = new Map<number, { resolve: (v: any) => void; tools: any; exec?: any }>()

createInterface({ input: child.stdout! }).on('line', async (line) => {
  const m = JSON.parse(line)
  if (m.cb !== undefined) {
    const w = waiting.get(m.call)
    let value: any
    try {
      if (m.kind === 'run') value = await w!.tools.run(m.cmd, m.args, { cwd: m.opts.cwd ?? undefined, timeoutMs: m.opts.timeoutMs ?? undefined, env: m.opts.env, input: m.opts.input ?? undefined })
      else if (m.kind === 'cli') value = await w!.exec(m.args)
      else if (m.kind === 'http') value = { status: await w!.tools.httpStatus(m.url) }
      else if (m.kind === 'api') value = await w!.tools.githubApi(m.method, m.path, m.token, m.body ?? undefined)
    } catch (e) {
      value = { error: (e as Error).message }
    }
    child.stdin!.write(JSON.stringify({ cbReply: m.cb, value }) + '\n')
  } else if (m.id !== undefined) {
    waiting.get(m.id)?.resolve(m.result)
    waiting.delete(m.id)
  }
})

function remote(name: string, tools: any, args: Record<string, unknown>, exec?: any): Promise<any> {
  const id = nextId++
  return new Promise((resolve, reject) => {
    waiting.set(id, {
      tools,
      exec,
      resolve: (r) => {
        try {
          resolve(unwrap(r))
        } catch (e) {
          reject(e)
        }
      }
    })
    child.stdin!.write(JSON.stringify({ id, window: 1, channel: 'test:memsync', args: [name, { git: tools?.git, gh: tools?.gh, ...args }] }) + '\n')
  })
}

function local(name: string, args: Record<string, unknown>): any {
  return unwrap(JSON.parse(execFileSync(exe, ['--memsync', name], { input: JSON.stringify(args), encoding: 'utf8' })))
}

export const ensureRepo = (t: any, o: any) => remote('ensureRepo', t, o)
export const connectRepoByUrl = (t: any, o: any) => remote('connectRepoByUrl', t, o)
export const createRepoViaGitLogin = (t: any, o: any) => remote('createRepoViaGitLogin', t, o)
export const ghStatus = (t: any) => remote('ghStatus', t, {})
export const projectState = (t: any, o: any) => remote('projectState', t, o)
export const linkProject = (t: any, o: any) => remote('linkProject', t, o)
export const syncNow = (t: any, o: any) => remote('syncNow', t, o)
export const cloneState = (t: any, clonePath: string) => remote('cloneState', t, { clonePath })
export const applyBundledPlugin = (agent: string, exec: any, o: any) => remote('applyBundledPlugin', { git: '', gh: null }, { agent, marketplaceDir: o.marketplaceDir, homes: o.homes }, exec)
export const ensureFolder = (o: any) => local('ensureFolder', o)
export const installedPlugins = (agent: string, homes: any) => local('installedPlugins', { agent, homes })
export const pluginVersion = (dir: string) => local('pluginVersion', { dir })
export const prepareMarketplace = (bundled: string, dir: string) => local('prepareMarketplace', { bundled, dir })
export const parseRepoInput = (input: string) => local('parseRepoInput', { input })
export const pluginAction = (installed: unknown[], version: string) => local('pluginAction', { installed, version })
export const autoLinkBlock = (state: unknown) => local('autoLinkBlock', { state })
export function stop(): void {
  child.stdin!.end()
  child.kill()
}
