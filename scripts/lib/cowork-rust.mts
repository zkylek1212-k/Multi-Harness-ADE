// The API check-cowork*.mts uses, on the Rust implementation (through wb-ipc). A service runs in one wb-ipc process; resolving a CLI
// comes back here, so the scenario's own deps decide. Pure functions go through a one-shot call of the same executable.
import { execFileSync, spawn } from 'node:child_process'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { createInterface } from 'node:readline'

const target = process.env.CARGO_TARGET_DIR || path.join(process.env.LOCALAPPDATA || '', 'wb-cargo-target')
const exe = process.env.WB_IPC_PATH || path.join(target, 'debug', 'wb-ipc.exe')
const data = fs.mkdtempSync(path.join(os.tmpdir(), 'wb-cowork-'))

function call(name: string, args: unknown[]): any {
  const out = execFileSync(exe, ['--cowork-shared', name], { input: JSON.stringify(args.map((a) => (a === undefined ? null : a))), encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  const r = JSON.parse(out)
  if (r.err !== undefined) throw new Error(r.err)
  return r.undefined ? undefined : r.value
}

const plain = (name: string) => (...args: unknown[]): any => call(name, args)
const nullToUndef = (name: string) => (...args: unknown[]): any => call(name, args) ?? undefined

class RunawayMonitor {
  private history: unknown[] = []
  private now0: number
  private o: unknown
  constructor(now: number, o: unknown = {}) {
    this.now0 = now
    this.o = o
  }
  feed(ev: unknown, now: number): any {
    this.history.push({ ev, now })
    return call('runawayFeed', [this.now0, this.o, this.history])
  }
}

// Whatever a scenario asks of shared goes to the same-named Rust function
export const shared: any = new Proxy(
  {
    RunawayMonitor,
    parseExecLine: nullToUndef('parseExecLine'),
    plannerInvocation(agent: string, o: any) {
      const inv = call('plannerInvocation', [agent, o])
      return { ...inv, ...(inv.ndjson ? { stdin: (p: string) => call('agyStdin', [p]) } : {}), ...(inv.guard ? { guard: (l: string) => call('agyToolGuard', [l]) } : {}) }
    }
  } as Record<string, unknown>,
  {
    get(t, k: string) {
      if (k in t) return t[k]
      return plain(k)
    }
  }
)

const asyncOf = (name: string) => async (...args: unknown[]): Promise<any> => call(name, args)
export const gitm: any = {
  git: asyncOf('git'),
  readBaseline: asyncOf('readBaseline'),
  createSnapshot: asyncOf('createSnapshot'),
  snapshotChanges: asyncOf('snapshotChanges'),
  snapshotDirFor: plain('snapshotDirFor'),
  removeSnapshot: plain('removeSnapshot'),
  scopeEscapes: plain('scopeEscapes'),
  diffGuard: plain('diffGuard'),
  gitGuardState: asyncOf('gitGuardState'),
  exportBaselineHooks: asyncOf('exportBaselineHooks'),
  patchId: asyncOf('patchId'),
  mergeTree: asyncOf('mergeTree')
}

// The scenarios in check-cowork*.mts run on this, through wb-ipc
export const resolveCheckCommand = asyncOf('resolveCheckCommand')
export const runCheck = async (argv: string[], cwd: string, o: any = {}): Promise<any> => call('runCheck', [argv, cwd, o.timeoutMs, o.tree])

export class CoworkError extends Error {
  code: string
  data?: unknown
  constructor(code: string, message?: string, data?: unknown) {
    super(message || code)
    this.code = code
    this.data = data
  }
}

const child = spawn(exe, [], { env: { ...process.env, WB_DATA_DIR: data }, stdio: ['pipe', 'pipe', 'inherit'] })
let nextId = 1
let nextHandle = 1
const waiting = new Map<number, (m: any) => void>()
const services = new Map<number, CoworkService>()

createInterface({ input: child.stdout! }).on('line', async (line) => {
  const m = JSON.parse(line)
  if (m.cb !== undefined) {
    const svc = services.get(m.handle)!
    const fn = m.kind === 'resolveCli' ? svc.deps.resolveCli : svc.deps.resolveExecCli
    let value: any
    try {
      value = fn ? await fn(m.agent) : { error: 'execution is not available' }
      // The fake CLI reads its script from the environment at spawn, and this process's env is what a scenario changed
      if (value && !value.error) value = { ...value, env: { ...(value.env || {}), ...(process.env.FAKE_COWORK ? { FAKE_COWORK: process.env.FAKE_COWORK } : {}) } }
    } catch (e) {
      value = { error: (e as Error).message }
    }
    child.stdin!.write(JSON.stringify({ cbReply: m.cb, value }) + '\n')
  } else if (m.event === 'cowork:emit') {
    const svc = services.get(m.handle)
    if (svc) {
      svc.mirror.set(m.run.id, m.run)
      svc.deps.emit?.(m.run)
    }
  } else if (m.id !== undefined) {
    waiting.get(m.id)?.(m)
    waiting.delete(m.id)
  }
})

function rpc(channel: string, args: unknown[]): Promise<any> {
  const id = nextId++
  return new Promise((resolve) => {
    waiting.set(id, resolve)
    child.stdin!.write(JSON.stringify({ id, window: 1, channel, args: args.map((a) => (a === undefined ? null : a)) }) + '\n')
  })
}

export class CoworkService {
  deps: any
  handle = nextHandle++
  mirror = new Map<string, any>()
  private ready: Promise<any>

  constructor(deps: any) {
    this.deps = deps
    services.set(this.handle, this)
    this.ready = rpc('test:coworkNew', [this.handle, deps.dataDir])
  }

  private async call(method: string, ...args: unknown[]): Promise<any> {
    await this.ready
    const m = await rpc('test:coworkCall', [this.handle, method, args])
    const r = m.result
    if (!r.ok) throw new CoworkError(r.code, r.message, r.data ?? undefined)
    return r.value
  }

  /** Blocks nothing, so a scenario has to await it before using get() */
  async init(): Promise<void> {
    const runs = await this.call('init')
    for (const r of runs) this.mirror.set(r.id, r)
  }

  /** What the last event from the backend said: events arrive before the reply to the call that caused them */
  get(id: string): any {
    return this.mirror.get(id) ?? null
  }

  async start(o: any): Promise<any> {
    const r = await this.call('start', o)
    this.mirror.set(r.id, r)
    return r
  }

  async delete(id: string): Promise<void> {
    await this.call('delete', id)
    this.mirror.delete(id)
  }

  shutdown(): Promise<void> {
    return this.call('shutdown')
  }

  /** Replaces changing a live run object directly in the TS scenarios */
  testPatch(id: string, keys: string[], value: unknown): Promise<void> {
    return this.call('testPatch', id, keys, value)
  }
}

for (const m of ['list', 'cancel', 'discuss', 'setSummarizer', 'summarizeDiscussion', 'retry', 'dropFailedReviewers', 'addNote', 'feedback', 'editBoard', 'dismissUnresolved', 'approve', 'raiseLimits', 'logDispatch', 'execPreview', 'execStart', 'execDiff', 'execWaive', 'execMessage', 'execRetry', 'execAcceptScope', 'execRevertScope', 'execPause', 'execResume', 'execMerge', 'execCleanup']) {
  ;(CoworkService.prototype as any)[m] = function (this: any, ...args: unknown[]) {
    return this.call(m, ...args)
  }
}

export function stop(): void {
  child.stdin!.end()
  child.kill()
}
