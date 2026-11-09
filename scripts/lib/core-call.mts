// Pure functions and a running backend, for the check scripts. Everything goes through wb-ipc, the same core the app uses.
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import * as path from 'node:path'
import { createInterface } from 'node:readline'

export const exe = process.env.WB_IPC_PATH || path.join(process.env.CARGO_TARGET_DIR || path.join(process.env.LOCALAPPDATA || '', 'wb-cargo-target'), 'debug', 'wb-ipc.exe')

export function coreCall(name: string, ...args: unknown[]): any {
  const r = JSON.parse(execFileSync(exe, ['--core', name], { input: JSON.stringify(args.map((a) => (a === undefined ? null : a))), encoding: 'utf8', maxBuffer: 1 << 26 }))
  if (r.err !== undefined) throw new Error(r.err)
  return r.value
}

export class Backend {
  child: ChildProcess
  events: any[] = []
  private next = 1
  private waiting = new Map<number, (m: any) => void>()

  constructor(env: Record<string, string>) {
    this.child = spawn(exe, [], { env: { ...process.env, ...env }, stdio: ['pipe', 'pipe', 'inherit'] })
    createInterface({ input: this.child.stdout! }).on('line', (line) => {
      const m = JSON.parse(line)
      if (m.event) this.events.push(m)
      else if (m.id !== undefined) {
        this.waiting.get(m.id)?.(m)
        this.waiting.delete(m.id)
      }
    })
  }

  ipc(channel: string, ...args: unknown[]): Promise<any> {
    return new Promise((resolve, reject) => {
      const id = this.next++
      this.waiting.set(id, (m) => (m.error !== undefined ? reject(new Error(m.error)) : resolve(m.result)))
      this.child.stdin!.write(JSON.stringify({ id, window: 2, channel, args }) + '\n')
    })
  }

  stop(): void {
    this.child.stdin!.end()
    this.child.kill()
  }
}

export async function until(cond: () => boolean | Promise<boolean>, what: string, ms = 15000): Promise<void> {
  const t0 = Date.now()
  while (!(await cond())) {
    if (Date.now() - t0 > ms) throw new Error(`timeout: ${what}`)
    await new Promise((r) => setTimeout(r, 50))
  }
}
