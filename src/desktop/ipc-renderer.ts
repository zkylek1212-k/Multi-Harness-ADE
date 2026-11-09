// src/preload/index.ts is written against ipcRenderer. This gives it the same surface over Tauri, so the UI is unchanged.
type Listener = (event: unknown, ...args: any[]) => void

const tauri = (): any => (window as any).__TAURI__.core
const listeners = new Map<string, Set<Listener>>()

function dispatch(batch: [string, unknown[]][]): void {
  for (const [channel, args] of batch) {
    const set = listeners.get(channel)
    if (!set) continue
    for (const l of [...set]) l({}, ...args)
  }
}

const pipe = new (tauri().Channel)()
pipe.onmessage = dispatch
const subscribed: Promise<unknown> = tauri().invoke('subscribe', { channel: pipe })

// Sends keep their order: while one batch is in flight the next ones collect behind it.
// A fetch per keystroke could arrive out of order, and a terminal cannot tolerate that.
let outbox: [string, unknown[]][] = []
let flying = false
async function flush(): Promise<void> {
  if (flying) return
  flying = true
  await subscribed.catch(() => {})
  while (outbox.length) {
    const batch = outbox
    outbox = []
    await tauri().invoke('ipc_send', { batch }).catch(() => {})
  }
  flying = false
}

export const ipcRenderer = {
  async invoke(channel: string, ...args: unknown[]): Promise<any> {
    try {
      return await tauri().invoke('ipc', { channel, args })
    } catch (e) {
      throw e instanceof Error ? e : new Error(String(e))
    }
  },
  send(channel: string, ...args: unknown[]): void {
    outbox.push([channel, args])
    void flush()
  },
  on(channel: string, listener: Listener): void {
    let set = listeners.get(channel)
    if (!set) listeners.set(channel, (set = new Set()))
    set.add(listener)
  },
  removeListener(channel: string, listener: Listener): void {
    const set = listeners.get(channel)
    if (!set) return
    set.delete(listener)
    if (!set.size) listeners.delete(channel)
  }
}

export const contextBridge = {
  exposeInMainWorld(key: string, value: unknown): void {
    ;(window as any)[key] = value
  }
}
