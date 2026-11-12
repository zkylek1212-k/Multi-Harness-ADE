import type { ClientMessage, ServerMessage } from '../../shared/remoteProtocol'
import type { CanvasOp, CanvasOps, CanvasResult } from '../../shared/canvas'

// 與桌面 Remote Bridge 的 WebSocket 連線。iOS 把 App 切到背景時連線一定會斷，
// 所以這裡的重點是「回到前景立刻重連、重新 attach 目前的終端」。

export type ConnState = 'connecting' | 'open' | 'closed' | 'unauthorized'

/** `https://host:port/` → `wss://host:port/ws` */
function wsUrl(base: string): string {
  try {
    const u = new URL('/ws', base)
    u.protocol = 'wss:'
    return u.href
  } catch {
    return `wss://${location.host}/ws`
  }
}

/**
 * 配對：開一條一次性的 WebSocket，換到 token 就關掉。
 * 為什麼不用 fetch('/api/pair')：要配對的可能是「另一台」電腦，那是跨 origin，
 * fetch 會被 CORS 擋掉；WebSocket 沒有這個限制。
 */
export function pairOverWs(base: string, code: string, name: string, timeoutMs = 12_000): Promise<string> {
  return new Promise((resolve, reject) => {
    let ws: WebSocket
    try {
      ws = new WebSocket(wsUrl(base))
    } catch (e) {
      reject(e instanceof Error ? e : new Error('cannot open'))
      return
    }
    const timer = window.setTimeout(() => {
      ws.close()
      reject(new Error('timeout'))
    }, timeoutMs)
    const done = (err: Error | null, token?: string): void => {
      window.clearTimeout(timer)
      ws.onmessage = null
      ws.onerror = null
      ws.onclose = null
      ws.close()
      if (err) reject(err)
      else resolve(token!)
    }
    ws.onopen = () => ws.send(JSON.stringify({ t: 'pairRequest', code, name }))
    ws.onmessage = (ev) => {
      try {
        const m = JSON.parse(ev.data as string) as ServerMessage
        if (m.t === 'paired') return done(null, m.token)
        if (m.t === 'error') return done(new Error('invalid'))
      } catch {
        // 不是我們認得的訊息就繼續等
      }
    }
    ws.onerror = () => done(new Error('unreachable'))
    ws.onclose = () => done(new Error('unreachable'))
  })
}

export class RemoteConnection {
  private ws: WebSocket | null = null
  private retry = 0
  private timer: number | null = null
  private pingTimer: number | null = null
  private stopped = false
  private listeners = new Set<(m: ServerMessage) => void>()
  private stateListeners = new Set<(s: ConnState) => void>()
  state: ConnState = 'closed'

  /** base：要連的那台電腦，`https://host:port/`（可以不是這個頁面的 origin） */
  constructor(
    private base: string,
    private token: string
  ) {}

  start(): void {
    this.stopped = false
    this.connect()
    document.addEventListener('visibilitychange', this.onVisibility)
  }

  stop(): void {
    this.stopped = true
    document.removeEventListener('visibilitychange', this.onVisibility)
    this.clearTimers()
    this.ws?.close()
    this.ws = null
  }

  onMessage(cb: (m: ServerMessage) => void): () => void {
    this.listeners.add(cb)
    return () => this.listeners.delete(cb)
  }

  onState(cb: (s: ConnState) => void): () => void {
    this.stateListeners.add(cb)
    return () => this.stateListeners.delete(cb)
  }

  send(msg: ClientMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg))
  }

  private reqId = 0

  /** 畫布操作（與桌面 window.api.canvas.call 同名同參數）；斷線或逾時回 ok:false，不丟例外，也不自動重送 */
  canvas<K extends CanvasOp>(windowId: number, op: K, req?: CanvasOps[K]['req']): Promise<CanvasResult<CanvasOps[K]['res']>> {
    if (this.ws?.readyState !== WebSocket.OPEN) return Promise.resolve({ ok: false, code: 'offline' })
    const reqId = ++this.reqId
    return new Promise((resolve) => {
      const done = (r: CanvasResult<CanvasOps[K]['res']>): void => {
        window.clearTimeout(timer)
        off()
        resolve(r)
      }
      const timer = window.setTimeout(() => done({ ok: false, code: 'timeout' }), 30_000)
      const off = this.onMessage((m) => {
        if (m.t === 'canvas' && m.reqId === reqId) done(m.result as CanvasResult<CanvasOps[K]['res']>)
      })
      this.send({ t: 'canvas', reqId, windowId, op, req: req ?? null })
    })
  }

  private setState(s: ConnState): void {
    this.state = s
    for (const cb of this.stateListeners) cb(s)
  }

  private clearTimers(): void {
    if (this.timer) window.clearTimeout(this.timer)
    if (this.pingTimer) window.clearInterval(this.pingTimer)
    this.timer = null
    this.pingTimer = null
  }

  private connect(): void {
    if (this.stopped) return
    this.clearTimers()
    this.setState('connecting')
    const ws = new WebSocket(wsUrl(this.base))
    this.ws = ws
    ws.onopen = () => {
      this.retry = 0
      ws.send(JSON.stringify({ t: 'auth', token: this.token }))
      // 25 秒一次心跳，讓 NAT／Wi-Fi 省電模式不要把閒置連線收掉
      this.pingTimer = window.setInterval(() => this.send({ t: 'ping' }), 25_000)
    }
    ws.onmessage = (ev) => {
      let msg: ServerMessage
      try {
        msg = JSON.parse(ev.data as string)
      } catch {
        return
      }
      if (msg.t === 'authed') {
        this.setState('open')
        this.send({ t: 'visibility', visible: document.visibilityState === 'visible' })
      }
      for (const cb of this.listeners) cb(msg)
    }
    ws.onclose = (ev) => {
      if (this.ws !== ws) return
      this.ws = null
      this.clearTimers()
      if (ev.code === 4001) {
        this.setState('unauthorized')
        return
      }
      this.setState('closed')
      this.schedule()
    }
  }

  private schedule(): void {
    if (this.stopped || document.visibilityState !== 'visible') return
    const delay = Math.min(5000, 400 * 2 ** this.retry++)
    this.timer = window.setTimeout(() => this.connect(), delay)
  }

  private onVisibility = (): void => {
    const visible = document.visibilityState === 'visible'
    if (visible && !this.ws) {
      this.retry = 0
      this.connect()
    } else {
      this.send({ t: 'visibility', visible })
    }
  }
}
