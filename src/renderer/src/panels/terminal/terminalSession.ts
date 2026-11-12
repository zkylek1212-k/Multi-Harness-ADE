import { Terminal } from '@xterm/xterm'
import type { ITheme } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { looksLikeApprovalPrompt, readApprovalScreen } from './approvalDetect'
import { focusTerm, isComposing, isImeKey, trackComposition } from './imeGuard'
import type { PtySpawnOptions } from '../../../../preload/index'

// 一個終端 session 的呈現：xterm、PTY 綁定、IME、審批偵測、尺寸順序。TerminalPanel 與畫布節點共用。
// 行程屬於後端：這裡的建立／掛載／休眠／銷毀都不會送 pty:kill，要結束行程由呼叫端自己送。

// 比這還窄／矮就當作窗格正在收合或動畫中，不跟著縮：PTY 一縮，CLI 就照那個寬度重畫，捲動歷史留下窄版殘片
const MIN_COLS = 20
const MIN_ROWS = 5

export interface TerminalSessionOptions {
  theme?: ITheme
  minimumContrastRatio?: number
  /** 在 CSS transform: scale 底下顯示（畫布）時校正滑鼠座標，見 compensateZoom */
  zoomCompensation?: boolean
  /** spawn／attach 完成，拿到 pty id */
  onPtyId?: (ptyId: string) => void
  /** 每段輸出（已排進 xterm） */
  onOutput?: (data: string) => void
  /** 畫面上的審批提示出現／消失 */
  onApproval?: (waiting: boolean) => void
  /** 行程結束；null = 喚醒時發現它已經不在 */
  onExit?: (code: number | null) => void
  /**
   * 休眠時請後端停止對這個視窗送畫面資料。桌面 IPC 目前沒有這個通道，不給的話休眠只卸下前端 listener：
   * 後端照送、前端丟棄，喚醒那一瞬間送達的輸出可能同時在快照裡又被套用一次。
   */
  unsubscribe?: (ptyId: string) => void | Promise<void>
}

export type TerminalSessionSource =
  /** 用掛載後量到的尺寸 spawn 新行程 */
  | { spawn: Omit<PtySpawnOptions, 'cols' | 'rows'> }
  /** 接上已經在後端跑著的 pty（手機開的終端、畫布節點） */
  | { ptyId: string }

export interface TerminalSession {
  readonly term: Terminal
  /** 還活著的 pty；start 完成前與行程結束後是 null */
  readonly ptyId: string | null
  /** 已經 start 過（不論完成與否） */
  readonly started: boolean
  /** 掛進（或搬到）一個 DOM 容器 */
  attach(container: HTMLElement): void
  /** 放開容器上的監聽；term、PTY 與訂閱都留著 */
  detach(): void
  /** 只做一次：spawn 或接上 pty，掛上輸入／輸出 */
  start(source: TerminalSessionSource): void
  /** 依容器大小調整；組字中不要呼叫（見 imeGuard） */
  fit(): void
  focus(): void
  /** 送輸入給 pty */
  write(data: string): void
  /** 停止接收畫面資料；行程與結束事件的訂閱留著 */
  sleep(): void
  /** 重新訂閱：清掉畫面，套用快照，再依序套用之後的資料 */
  wake(): void
  /** 釋放 xterm 與所有監聽 */
  dispose(): void
}

export function createTerminalSession(opts: TerminalSessionOptions = {}): TerminalSession {
  const term = new Terminal({
    theme: opts.theme,
    minimumContrastRatio: opts.minimumContrastRatio,
    fontFamily: "'SF Mono', 'JetBrains Mono', 'Cascadia Code', ui-monospace, Menlo, Consolas, monospace",
    fontSize: 12,
    cursorBlink: true,
    // 跟 main 的 headless 畫面同一套 ConPTY 行為（pty.ts），尺寸變化後三方才會一致
    windowsPty: { backend: 'conpty' }
  })
  const fitAddon = new FitAddon()
  term.loadAddon(fitAddon)

  term.attachCustomKeyEventHandler((e: KeyboardEvent) => {
    // false skips xterm's key processing without preventing the browser/IME.
    if (isImeKey(term, e)) return false

    // 2. Ctrl+C (Windows/Linux) 或 Cmd+C (macOS)：
    // 若終端有選取文字，執行複製到剪貼簿並阻止發送 SIGINT (\x03) 給 pty，避免中斷終端執行
    if ((e.ctrlKey || e.metaKey) && e.code === 'KeyC' && !e.shiftKey && !e.altKey) {
      if (term.hasSelection()) {
        // 同上：不 preventDefault 的話瀏覽器還會再複製一次（內容一樣，但沒必要）
        e.preventDefault()
        if (e.type === 'keydown') {
          const sel = term.getSelection()
          if (sel) {
            navigator.clipboard.writeText(sel).catch(() => {})
          }
        }
        return false
      }
      return true
    }

    // 3. Ctrl+V (Windows/Linux) 或 Cmd+V (macOS)：自己讀剪貼簿送出。
    // 一定要 preventDefault：只回傳 false 只是叫 xterm 不要把這個鍵當成 \x16，
    // 瀏覽器的預設貼上動作還是會發生，xterm 也監聽 textarea 的 paste 事件，
    // 於是同一次 Ctrl+V 會被插入兩次。preventDefault 之後只剩下面這一條路。
    if ((e.ctrlKey || e.metaKey) && e.code === 'KeyV' && !e.shiftKey && !e.altKey) {
      e.preventDefault()
      if (e.type === 'keydown') {
        navigator.clipboard
          .readText()
          .then((text) => {
            if (text) term.paste(text)
          })
          .catch(() => {})
      }
      return false
    }

    return true
  })

  let ptyId: string | null = null
  // 記在 session 上，不是元件的 ref：元件 remount（版面／停靠／分割變動）ref 會歸零，就會對同一個 term 再 spawn 一次、
  // 再掛一組 onData——之後每敲一個字會送進兩個 pty、兩邊都回顯，畫面上就是每個字重複兩次。
  let started = false
  let disposed = false
  let asleep = false
  /** 每次休眠／喚醒／銷毀加一：晚到的快照回覆對不上就丟掉 */
  let epoch = 0
  let approval = false
  let approvalTimer: ReturnType<typeof setTimeout> | undefined
  let release: (() => void) | null = null
  /** 畫面資料的 listener，休眠時卸下 */
  let stream: (() => void)[] = []
  /** 結束事件與鍵盤輸入，留到 dispose */
  const bound: (() => void)[] = []
  /** 快照還沒回來時先留住的畫面事件 */
  let held: (() => void)[] | null = null
  /** 訂閱與退訂一次一個、照順序到後端，退訂不會追過還在路上的訂閱 */
  let backend: Promise<unknown> = Promise.resolve()
  const unzoom = opts.zoomCompensation ? compensateZoom(term) : null

  const checkApproval = (): void => {
    if (approvalTimer || !ptyId) return
    approvalTimer = setTimeout(() => {
      approvalTimer = undefined
      const waiting = looksLikeApprovalPrompt(readApprovalScreen(term))
      if (waiting === approval) return
      approval = waiting
      opts.onApproval?.(waiting)
    }, 120)
  }

  /**
   * 接著 pty 時不直接改 xterm：送給 main，main 依輸出順序套用後回 pty:resized 才改，
   * 桌面、main 的畫面與手機三方才會一致（見 pty.ts resizePty）。
   */
  const fit = (): void => {
    const dims = fitAddon.proposeDimensions()
    if (!dims || dims.cols < MIN_COLS || dims.rows < MIN_ROWS) return
    if (dims.cols === term.cols && dims.rows === term.rows) return
    if (ptyId) window.api.pty.resize(ptyId, dims.cols, dims.rows)
    else term.resize(dims.cols, dims.rows)
  }

  const feed = (apply: () => void): void => {
    if (held) held.push(apply)
    else apply()
  }

  const listen = (id: string): void => {
    stream = [
      window.api.pty.onData(id, (data) =>
        feed(() => {
          term.write(data, checkApproval)
          opts.onOutput?.(data)
        })
      ),
      // 排在已收到的輸出後面才改尺寸，跟 main 套用的順序一樣
      window.api.pty.onResized(id, (cols, rows) =>
        feed(() =>
          term.write('', () => {
            term.resize(cols, rows)
            checkApproval()
          })
        )
      )
    ]
  }

  const mute = (): void => {
    stream.forEach((off) => off())
    stream = []
    held = null
  }

  const exited = (code: number | null): void => {
    clearTimeout(approvalTimer)
    // 結束後 main 不再處理 resize，改回直接調整 xterm
    ptyId = null
    approval = false
    opts.onExit?.(code)
  }

  /** 重新訂閱：清掉畫面，套用快照，再依序套用之後的資料 */
  const resync = (id: string): void => {
    const mine = ++epoch
    backend = backend
      .then(async () => {
        if (mine !== epoch) return
        // 後端在 mirror lock 底下加入訂閱並回快照：先掛好 listener 再問，快照之後的資料全在 held 裡，不缺不重
        held = []
        listen(id)
        const res = await window.api.pty.attach(id)
        // 等的時候又休眠或銷毀了：那邊已經卸下 listener
        if (mine !== epoch) return
        if (!res) {
          mute()
          if (ptyId) exited(null)
          return
        }
        const queued = held ?? []
        held = null
        term.reset()
        term.resize(res.cols, res.rows)
        term.write(res.snapshot)
        queued.forEach((apply) => apply())
        term.write('', checkApproval)
      })
      .catch(() => {
        if (mine === epoch) mute()
      })
  }

  const detach = (): void => {
    release?.()
    release = null
  }

  return {
    term,
    get ptyId() {
      return ptyId
    },
    get started() {
      return started
    },

    attach(container) {
      detach()
      // xterm 的 open() 只有第一次有作用，之後換容器要自己搬
      if (term.element) container.appendChild(term.element)
      else term.open(container)
      // 還沒 start：直接量，spawn 用這個尺寸
      if (started) fit()
      else fitAddon.fit()
      const untrackIme = trackComposition(term)

      // 杜絕手掌誤觸觸控板產生的中鍵（Button 1）貼上：以 capture 階段攔截，防止 xterm 接收 auxclick
      const handleAuxClick = (e: MouseEvent): void => {
        if (e.button === 1) {
          e.preventDefault()
          e.stopPropagation()
        }
      }
      container.addEventListener('auxclick', handleAuxClick, true)

      // 拖分隔線、展開收合會連續觸發：停下來才調整一次，CLI 只重畫一次
      let fitTimer: ReturnType<typeof setTimeout> | undefined
      const ro = new ResizeObserver(() => {
        clearTimeout(fitTimer)
        fitTimer = setTimeout(() => {
          // 組字中改尺寸會讓 xterm 重繪並中止組字（見 imeGuard）
          if (!isComposing(term)) fit()
        }, 100)
      })
      ro.observe(container)

      release = () => {
        clearTimeout(fitTimer)
        ro.disconnect()
        untrackIme()
        container.removeEventListener('auxclick', handleAuxClick, true)
      }
    },

    detach,

    start(source) {
      if (started || disposed) return
      started = true
      const spawn = 'spawn' in source ? { ...source.spawn, cols: term.cols, rows: term.rows } : null
      const ready: Promise<string> = spawn
        ? window.api.pty.spawn(spawn)
        : (() => {
            // 已經在跑：接上去、先補目前畫面。listener 在送出請求前就掛好，快照之後的輸出一段都不會漏
            const id = (source as { ptyId: string }).ptyId
            held = []
            listen(id)
            return window.api.pty.attach(id).then((res) => {
              if (!res) throw new Error('remote terminal is no longer running')
              // 先用 pty 現在的尺寸還原畫面，下面再調成窗格大小
              term.resize(res.cols, res.rows)
              term.write(res.snapshot)
              return id
            })
          })()

      ready
        .then((id) => {
          if (disposed) return
          ptyId = id
          opts.onPtyId?.(id)
          if (asleep) mute()
          else if (held) {
            const queued = held
            held = null
            queued.forEach((apply) => apply())
          } else if (spawn) listen(id)
          // 接上的途中休眠又醒來：留住的資料已經丟了，重新對齊一次
          else resync(id)
          bound.push(
            window.api.pty.onExit(id, exited),
            (() => {
              const input = term.onData((data) => window.api.pty.write(id, data))
              return () => input.dispose()
            })()
          )
          term.write('', checkApproval)
          // spawn 期間窗格變過的話 xterm 已直接改了尺寸，pty 還是 spawn 時的大小
          if (spawn && (term.cols !== spawn.cols || term.rows !== spawn.rows)) {
            window.api.pty.resize(id, term.cols, term.rows)
          }
          // attach 的 pty 是別人開的尺寸，調成窗格大小
          fit()
        })
        .catch((err: unknown) => {
          if (!ptyId) mute()
          const msg = err instanceof Error ? err.message : String(err)
          term.write(`\r\n\x1b[31mFailed to start: ${msg}\x1b[0m\r\n`)
        })
    },

    fit,

    focus() {
      focusTerm(term)
    },

    write(data) {
      if (ptyId) window.api.pty.write(ptyId, data)
    },

    sleep() {
      if (asleep || disposed) return
      asleep = true
      epoch++
      mute()
      clearTimeout(approvalTimer)
      approvalTimer = undefined
      const id = ptyId
      const off = opts.unsubscribe
      if (id && off) backend = backend.then(() => off(id)).catch(() => {})
    },

    wake() {
      if (!asleep || disposed) return
      asleep = false
      // 還沒 start 完（完成時會自己掛上）或已經結束（不能把它畫成活的）就不必問
      if (ptyId) resync(ptyId)
    },

    dispose() {
      if (disposed) return
      disposed = true
      epoch++
      detach()
      mute()
      bound.forEach((off) => off())
      clearTimeout(approvalTimer)
      unzoom?.()
      term.dispose()
    }
  }
}

const remapped = new WeakSet<Event>()

/**
 * xterm 用未縮放的格寬去換算 getBoundingClientRect 之後的座標，所以在 CSS transform: scale 底下滑鼠選取會選錯字，
 * 偏移隨距離放大。這裡在 capture 階段攔下滑鼠事件，把座標換回未縮放的位置再重發。回傳解除的函式。
 */
export function compensateZoom(term: Terminal): () => void {
  let dragging = false
  const remap = (e: MouseEvent): void => {
    if (remapped.has(e)) return
    const target = e.target instanceof Node ? e.target : null
    const inside = !!target && !!term.element?.contains(target)
    // xterm 拖曳時改聽 document：從這個終端開始的拖曳，滑出去之後也要照同一個終端換算
    if (e.type === 'mousedown') dragging = inside
    if (!inside && !dragging) return
    if (e.type === 'mouseup') dragging = false
    const screen = term.element?.querySelector<HTMLElement>('.xterm-screen')
    if (!screen || !target) return
    const r = screen.getBoundingClientRect()
    const s = r.width / screen.offsetWidth
    if (!Number.isFinite(s) || s <= 0 || Math.abs(s - 1) < 1e-3) return
    e.stopImmediatePropagation()
    // xterm 自己也會擋 mousedown 的預設行為；原事件被攔下後由這裡代勞
    if (e.type === 'mousedown') e.preventDefault()
    const copy = new MouseEvent(e.type, {
      bubbles: true,
      cancelable: true,
      composed: true,
      view: window,
      detail: e.detail,
      screenX: e.screenX,
      screenY: e.screenY,
      clientX: r.left + (e.clientX - r.left) / s,
      clientY: r.top + (e.clientY - r.top) / s,
      ctrlKey: e.ctrlKey,
      shiftKey: e.shiftKey,
      altKey: e.altKey,
      metaKey: e.metaKey,
      button: e.button,
      buttons: e.buttons,
      relatedTarget: e.relatedTarget
    })
    remapped.add(copy)
    target.dispatchEvent(copy)
  }
  const types = ['mousedown', 'mousemove', 'mouseup'] as const
  types.forEach((t) => window.addEventListener(t, remap, true))
  return () => types.forEach((t) => window.removeEventListener(t, remap, true))
}
