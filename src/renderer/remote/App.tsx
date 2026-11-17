import { useEffect, useMemo, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { version } from '../../../package.json'
import type {
  RemoteGitFile,
  RemoteLauncher,
  RemoteSession,
  RemoteWindow,
  RemoteWorkspaceOption,
  ServerMessage
} from '../../shared/remoteProtocol'
import { sessionStatus } from '../../shared/remoteProtocol'
import { pairOverWs, RemoteConnection, type ConnState } from './conn'
import { disablePush, enablePush, isStandalone, pushState, pushSupported, registerServiceWorker } from './api'
import TerminalView from './TerminalView'
import { FilePane, StatusPane } from './WorkspacePanes'
import CoworkView from './CoworkView'
import { ago, getLangPref, onLangChange, setLangPref, t, type LangPref } from './i18n'
import { getThemePref, setThemePref, type ThemePref } from './theme'
import { questionPreview } from './prompt'
import {
  AgentMark,
  IBranch,
  ICheck,
  IChevronRight,
  IconMark,
  IDesktop,
  IDoc,
  IGear,
  IHand,
  IPhone,
  IPlus,
  IShare,
  IStop,
  IWarning
} from './icons'
import { ConfirmSheet, ConnCapsule, Menu, NavBar, Sheet, Switch, useScrolled, type MenuItem } from './ui'
import {
  activeUrl,
  addHost,
  currentUrl,
  getToken,
  hostOf,
  hostsFull,
  loadHosts,
  MAX_HOSTS,
  parseHostInput,
  removeHost,
  renameHost,
  setActiveUrl,
  setToken,
  setupUrlFor,
  type SavedHost
} from './hosts'

type View =
  | { kind: 'home' }
  | { kind: 'session'; id: string }
  | { kind: 'handoff'; windowId: number }
  | { kind: 'git'; windowId: number }
  | { kind: 'status'; windowId: number }
  | { kind: 'file'; windowId: number; path?: string }
  | { kind: 'cowork'; windowId: number }

function sessionFromHash(): string | null {
  return /[#&]s=([\w-]+)/.exec(location.hash)?.[1] ?? null
}

/** Windows 路徑：分隔符與大小寫都不該影響比對 */
function samePath(a: string, b: string): boolean {
  return a.replace(/\\/g, '/').toLowerCase() === b.replace(/\\/g, '/').toLowerCase()
}

function formatCode(raw: string): string {
  const s = raw.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8)
  return s.length > 4 ? `${s.slice(0, 4)}-${s.slice(4)}` : s
}

// 背景層：莫蘭迪色暈，讓上面的磨砂玻璃有顏色可以透
function Backdrop(): JSX.Element {
  return (
    <div className="backdrop" aria-hidden="true">
      <div className="blob b1" />
      <div className="blob b2" />
      <div className="blob b3" />
      <div className="blob b4" />
      <div className="blob b5" />
    </div>
  )
}

export default function App(): JSX.Element {
  // 切換語言時整棵樹重畫（不重新掛載，連線與目前畫面都保留）
  const [, setLangTick] = useState(0)
  useEffect(() => onLangChange(() => setLangTick((n) => n + 1)), [])
  return (
    <>
      <Backdrop />
      <Screens />
    </>
  )
}

function Screens(): JSX.Element {
  // 目前連哪一台電腦。切換不換頁：WebSocket 可以跨 origin，所以只是換連線目標。
  const [active, setActive] = useState<string>(() => activeUrl())
  const [token, setTokenState] = useState<string | null>(() => getToken(activeUrl()))
  const [hosts, setHosts] = useState<SavedHost[]>(() => loadHosts())
  const [unpaired, setUnpaired] = useState(false)

  useEffect(() => {
    // Service Worker 只為「送來這個 App 的那台電腦」註冊：推播綁在 origin 上
    void registerServiceWorker()
  }, [])

  const switchHost = (url: string): void => {
    const u = setActiveUrl(url)
    setUnpaired(false)
    setActive(u)
    setTokenState(getToken(u))
  }

  const changeHosts = (list: SavedHost[]): void => {
    setHosts(list)
    // 刪掉的可能正是目前連著的那一台
    if (!list.some((h) => h.url === active)) switchHost(activeUrl())
  }

  if (!token) {
    return (
      <PairScreen
        base={active}
        hosts={hosts}
        notice={unpaired ? t('unauthorized') : null}
        onSwitch={switchHost}
        onHostsChange={changeHosts}
        onPaired={(tok) => {
          setToken(active, tok)
          addHost(active)
          setHosts(loadHosts())
          setUnpaired(false)
          setTokenState(tok)
        }}
      />
    )
  }
  return (
    <Main
      key={`${active}|${token}`}
      base={active}
      token={token}
      hosts={hosts}
      onSwitch={switchHost}
      onHostsChange={changeHosts}
      onUnauthorized={() => {
        setToken(active, null)
        setUnpaired(true)
        setTokenState(null)
      }}
    />
  )
}

// —— 配對 ——

function PairScreen({
  base,
  hosts,
  notice,
  onSwitch,
  onHostsChange,
  onPaired
}: {
  base: string
  hosts: SavedHost[]
  notice: string | null
  onSwitch: (url: string) => void
  onHostsChange: (list: SavedHost[]) => void
  onPaired: (token: string) => void
}): JSX.Element {
  const [code, setCode] = useState(() => formatCode(/[#&]pair=([A-Za-z0-9-]+)/.exec(location.hash)?.[1] ?? ''))
  const [name, setName] = useState(/iPad/.test(navigator.userAgent) ? 'iPad' : 'iPhone')
  const [error, setError] = useState<string | null>(notice)
  const [unreachable, setUnreachable] = useState(false)
  const [busy, setBusy] = useState(false)
  const [manage, setManage] = useState(false)
  const ready = code.replace(/-/g, '').length === 8 && name.trim().length > 0
  const remote = base !== currentUrl()

  const submit = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    setUnreachable(false)
    try {
      // 配對走 WebSocket：要配對的可能是另一台電腦（跨 origin，fetch 會被 CORS 擋）
      const token = await pairOverWs(base, code, name.trim())
      history.replaceState(null, '', '/')
      onPaired(token)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      if (msg === 'invalid') setError(t('pairInvalid'))
      else {
        setUnreachable(true)
        setError(t('pairFailed', { e: msg }))
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <form
      className="pair content"
      onSubmit={(e) => {
        e.preventDefault()
        if (ready && !busy) void submit()
      }}
    >
      <div className="pair-hero">
        <img src="/remote-icon-180.png" alt="" width={76} height={76} />
        <h1>{t('pairTitle')}</h1>
        <p>{remote ? t('pairDescRemote', { host: hostOf(base) }) : t('pairDesc')}</p>
      </div>

      <div className="section">
        <div className="bubble form-card">
          <div className="field">
            <label htmlFor="code">{t('pairCode')}</label>
            <input
              id="code"
              className="code-input"
              value={code}
              onChange={(e) => setCode(formatCode(e.target.value))}
              placeholder="XXXX-XXXX"
              autoCapitalize="characters"
              autoCorrect="off"
              autoComplete="one-time-code"
              spellCheck={false}
              aria-invalid={!!error}
              aria-describedby={error ? 'pair-error' : undefined}
            />
          </div>
          <div className="field">
            <label htmlFor="name">{t('deviceName')}</label>
            <input id="name" value={name} onChange={(e) => setName(e.target.value)} maxLength={60} />
          </div>
        </div>
        {error && (
          <p className="form-error" id="pair-error" role="alert">
            <IWarning size={18} />
            {error}
          </p>
        )}
      </div>

      {!isStandalone() && (
        <div className="notice neutral bubble">
          <IShare size={22} />
          <div>
            <b>{t('addToHomeTitle')}</b>
            <p className="t-sub">{t('addToHome')}</p>
          </div>
        </div>
      )}

      <div className="pair-actions">
        <button className="btn prominent press" disabled={!ready || busy}>
          {busy ? t('pairing') : t('pair')}
        </button>
      </div>

      {/* 連不上通常不是配對碼的問題，而是這支手機還沒信任那台電腦的憑證 */}
      {unreachable && (
        <div className="notice neutral bubble">
          <IWarning size={22} />
          <div>
            <b>{t('trustFirstTitle')}</b>
            <p className="t-sub">{t('trustFirst', { url: setupUrlFor(base) })}</p>
            <a className="btn press" href={setupUrlFor(base)} target="_blank" rel="noreferrer">
              {t('openSetupPage')}
            </a>
          </div>
        </div>
      )}

      {/* 選另一台已記住的電腦，或加一台新的——都不離開這個畫面 */}
      <div className="section">
        <div className="section-head">
          <h2>{t('computers')}</h2>
        </div>
        <div className="stack">
          {hosts
            .filter((h) => h.url !== base)
            .map((h) => (
              <button type="button" key={h.url} className="add-card press" onClick={() => onSwitch(h.url)}>
                <IDesktop size={18} />
                {h.label}
              </button>
            ))}
          <button type="button" className="add-card press" onClick={() => setManage(true)} disabled={hostsFull()}>
            <IPlus size={20} />
            {t('addComputer')}
          </button>
        </div>
      </div>

      {manage && (
        <AddComputerSheet
          onClose={() => setManage(false)}
          onAdd={(url) => {
            const list = addHost(url)
            if (list) {
              onHostsChange(list)
              onSwitch(url)
            }
            setManage(false)
          }}
        />
      )}
    </form>
  )
}

/** 加一台電腦：輸入它的區網位址（電腦的「設定 → 遠端控制」上就有） */
function AddComputerSheet({ onClose, onAdd }: { onClose: () => void; onAdd: (url: string) => void }): JSX.Element {
  const [value, setValue] = useState('')
  const parsed = parseHostInput(value)
  return (
    <Sheet
      title={t('addComputer')}
      onClose={onClose}
      leading={
        // 這個 sheet 會畫在配對畫面的 <form> 裡：不標 type 的按鈕會變成送出表單
        <button type="button" className="text-btn" onClick={onClose}>
          {t('cancel')}
        </button>
      }
      trailing={
        <button type="button" className="text-btn strong" disabled={!parsed} onClick={() => parsed && onAdd(parsed)}>
          {t('add')}
        </button>
      }
    >
      <div className="bubble form-card">
        <div className="field">
          <label htmlFor="host-input">{t('computerAddress')}</label>
          <input
            id="host-input"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="192.168.1.20"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            inputMode="url"
          />
        </div>
      </div>
      <p className="footnote">{parsed ? t('addComputerResolved', { url: hostOf(parsed) }) : t('addComputerHint')}</p>
    </Sheet>
  )
}

// —— 主畫面 ——

function Main({
  base,
  token,
  hosts,
  onSwitch,
  onHostsChange,
  onUnauthorized
}: {
  base: string
  token: string
  hosts: SavedHost[]
  onSwitch: (url: string) => void
  onHostsChange: (list: SavedHost[]) => void
  onUnauthorized: () => void
}): JSX.Element {
  const conn = useMemo(() => new RemoteConnection(base, token), [base, token])
  const [connState, setConnState] = useState<ConnState>('connecting')
  const [host, setHost] = useState('')
  const [sessions, setSessions] = useState<RemoteSession[]>([])
  const [windows, setWindows] = useState<RemoteWindow[]>([])
  const [workspaces, setWorkspaces] = useState<RemoteWorkspaceOption[]>([])
  const [busy, setBusy] = useState<Record<string, boolean>>({})
  const [bypass, setBypass] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [view, setView] = useState<View>(() => {
    const id = sessionFromHash()
    return id ? { kind: 'session', id } : { kind: 'home' }
  })
  const [exits, setExits] = useState<Record<string, number>>({})
  const [tails, setTails] = useState<Record<string, string>>({})
  const [sheet, setSheet] = useState<null | { kind: 'new'; windowId: number } | { kind: 'settings' }>(null)
  const [lastSession, setLastSession] = useState<RemoteSession | null>(null)
  // 手機請桌面開工作區後，等它出現在 state 就直接跳出「新增終端」，不用再多按一次
  const pendingWorkspace = useRef<string | null>(null)

  useEffect(() => {
    const offState = conn.onState((s) => {
      setConnState(s)
      if (s === 'unauthorized') onUnauthorized()
    })
    const off = conn.onMessage((m: ServerMessage) => {
      switch (m.t) {
        case 'authed':
          setHost(m.hostName)
          // 連上也驗過了，把名字換成電腦自己報的主機名（剛加進來時只有 IP）
          onHostsChange(addHost(base, m.hostName) || loadHosts())
          break
        case 'state': {
          setSessions(m.sessions)
          setWindows(m.windows)
          setWorkspaces(m.workspaces || [])
          setBusy(Object.fromEntries(m.sessions.map((s) => [s.id, s.busy])))
          setBypass(m.bypass)
          setLoaded(true)
          const want = pendingWorkspace.current
          if (want) {
            const opened = m.windows.find((w) => samePath(w.workspace, want))
            if (opened) {
              pendingWorkspace.current = null
              setSheet({ kind: 'new', windowId: opened.id })
            }
          }
          break
        }
        case 'activity':
          setBusy((prev) => ({ ...prev, [m.id]: m.busy }))
          break
        case 'exit':
          setExits((prev) => ({ ...prev, [m.id]: m.code }))
          setBusy((prev) => ({ ...prev, [m.id]: false }))
          break
        case 'approval':
          setTails((prev) => ({ ...prev, [m.id]: m.tail }))
          break
        case 'spawned':
          setSheet(null)
          setView({ kind: 'session', id: m.id })
          break
      }
    })
    conn.start()
    // 點通知（Service Worker 轉來）時直接開到那個終端
    const onSwMessage = (ev: MessageEvent): void => {
      const id = /[#&]s=([\w-]+)/.exec(String(ev.data?.url ?? ''))?.[1]
      if (ev.data?.type === 'open' && id) setView({ kind: 'session', id })
    }
    navigator.serviceWorker?.addEventListener('message', onSwMessage)
    return () => {
      off()
      offState()
      conn.stop()
      navigator.serviceWorker?.removeEventListener('message', onSwMessage)
    }
  }, [conn])

  useEffect(() => {
    window.scrollTo(0, 0)
  }, [view.kind])

  const current = view.kind === 'session' ? sessions.find((s) => s.id === view.id) || null : null
  useEffect(() => {
    if (current) setLastSession(current)
  }, [current])

  const goHome = (): void => {
    history.replaceState(null, '', '/')
    setView({ kind: 'home' })
  }
  const hostName = host || t('computer')

  let screen: JSX.Element
  if (view.kind === 'session' && (current || lastSession?.id === view.id)) {
    const s = (current || lastSession)!
    screen = <TerminalView conn={conn} session={s} hostName={hostName} exitCode={exits[s.id]} onBack={goHome} />
  } else if (view.kind === 'handoff' || view.kind === 'git') {
    const w = windows.find((x) => x.id === view.windowId)
    screen = <DetailView conn={conn} view={view} workspace={w?.workspaceName || ''} hostName={hostName} onBack={goHome} />
  } else if (view.kind === 'cowork') {
    const w = windows.find((x) => x.id === view.windowId)
    screen = <CoworkView key={view.windowId} conn={conn} windowId={view.windowId} workspace={w?.workspaceName || ''} hostName={hostName} onBack={goHome} onOpenSession={(id) => setView({ kind: 'session', id })} />
  } else if (view.kind === 'status' || view.kind === 'file') {
    const w = windows.find(x => x.id === view.windowId)
    const windowId = view.windowId
    screen = <div className="session">
      <NavBar title={t(view.kind)} subtitle={w?.workspaceName || ''} backLabel={hostName} onBack={goHome} />
      <main className="workspace-pane">
        {view.kind === 'status' ? <StatusPane conn={conn} windowId={windowId} onFile={path => setView({ kind: 'file', windowId, path })} /> :
          <FilePane conn={conn} windowId={windowId} initialPath={view.path} />}
      </main>
    </div>
  } else {
    screen = (
      <Home
        host={hostName}
        connState={connState}
        loaded={loaded}
        sessions={sessions}
        windows={windows}
        workspaces={workspaces}
        busy={busy}
        bypass={bypass}
        tails={tails}
        hosts={hosts}
        activeHost={base}
        onOpen={(v) => setView(v)}
        onNew={(windowId) => setSheet({ kind: 'new', windowId })}
        onSwitchHost={onSwitch}
        onOpenWorkspace={(p) => {
          pendingWorkspace.current = p
          conn.send({ t: 'openWorkspace', path: p })
        }}
        onSettings={() => setSheet({ kind: 'settings' })}
      />
    )
  }

  return (
    <>
      {screen}
      <ConnCapsule state={connState} />
      {sheet?.kind === 'new' && (
        <NewTerminalSheet
          launchers={windows.find((w) => w.id === sheet.windowId)?.launchers || []}
          bypass={bypass}
          onClose={() => setSheet(null)}
          onPick={(key) => conn.send({ t: 'spawn', windowId: sheet.windowId, launcherKey: key })}
          onCowork={() => {
            setSheet(null)
            setView({ kind: 'cowork', windowId: sheet.windowId })
          }}
        />
      )}
      {sheet?.kind === 'settings' && (
        <SettingsSheet
          token={token}
          host={hostName}
          connState={connState}
          hosts={hosts}
          activeHost={base}
          onHostsChange={onHostsChange}
          onSwitchHost={onSwitch}
          bypass={bypass}
          onSetBypass={(enabled) => conn.send({ t: 'setBypass', enabled })}
          onClose={() => setSheet(null)}
          onUnpair={() => {
            conn.stop()
            onUnauthorized()
          }}
        />
      )}
    </>
  )
}

function useNow(ms = 2000): number {
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(id)
  }, [ms])
  return now
}

/** 狀態膠囊：文字 + 點，等你回覆的點會呼吸 */
function StatusPill({ status }: { status: 'waiting' | 'running' | 'idle' }): JSX.Element {
  const cls = status === 'waiting' ? 'wait' : status === 'running' ? 'run' : 'idle'
  return (
    <span className={`pill ${cls}`}>
      <span className="dot" aria-hidden="true" />
      {t(status)}
    </span>
  )
}

function Home({
  host,
  connState,
  loaded,
  sessions,
  windows,
  workspaces,
  busy,
  bypass,
  tails,
  hosts,
  activeHost,
  onOpen,
  onNew,
  onOpenWorkspace,
  onSwitchHost,
  onSettings
}: {
  host: string
  connState: ConnState
  loaded: boolean
  sessions: RemoteSession[]
  windows: RemoteWindow[]
  workspaces: RemoteWorkspaceOption[]
  busy: Record<string, boolean>
  bypass: boolean
  tails: Record<string, string>
  hosts: SavedHost[]
  activeHost: string
  onOpen: (v: View) => void
  onNew: (windowId: number) => void
  onOpenWorkspace: (path: string) => void
  onSwitchHost: (url: string) => void
  onSettings: () => void
}): JSX.Element {
  const scrolled = useScrolled(44)
  const [picker, setPicker] = useState(false)
  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    try {
      const saved: unknown = JSON.parse(localStorage.getItem('aw.remote.collapsed-workspaces') || '[]')
      return new Set(Array.isArray(saved) ? saved.filter((key): key is string => typeof key === 'string') : [])
    } catch {
      return new Set()
    }
  })
  useEffect(() => {
    try { localStorage.setItem('aw.remote.collapsed-workspaces', JSON.stringify([...collapsed])) } catch { /* This visit still works without storage. */ }
  }, [collapsed])
  // 下拉：記住的每一台（打勾的是現在連著的）＋管理。切換只是換連線目標，不換頁。
  const hostMenu: MenuItem[] = [
    ...hosts.map((h) => ({
      label: h.url === activeHost ? host : h.label,
      note: h.url === activeHost ? t('thisComputer') : hostOf(h.url),
      checked: h.url === activeHost,
      onSelect: () => onSwitchHost(h.url)
    })),
    { label: t('manageComputers'), icon: <IGear size={18} />, onSelect: onSettings }
  ]
  // 只為了讓「N 分鐘前開始」跟著走，狀態膠囊不看時鐘
  useNow()
  const waiting = sessions.filter((s) => s.needsApproval)
  const byWindow = new Map<number, RemoteSession[]>()
  for (const s of sessions) byWindow.set(s.windowId, [...(byWindow.get(s.windowId) || []), s])

  return (
    <div className="home">
      <NavBar
        title={host}
        clear
        scrolled={scrolled}
        trailing={
          <button className="circle-btn press" aria-label={t('settings')} onClick={onSettings}>
            <IGear size={22} />
          </button>
        }
      />
      <main className="content">
        <header className="large-header">
          {/* 記了多台電腦（或這台還沒記）時，標題就是切換電腦的下拉 */}
          {hostMenu.length > 1 ? (
            <button className="host-switch press" onClick={() => setPicker(true)} aria-haspopup="menu">
              <h1 className="t-large">{host}</h1>
              <span className="chev-down" aria-hidden="true">
                <IChevronRight size={20} />
              </span>
            </button>
          ) : (
            <h1 className="t-large">{host}</h1>
          )}
          <div className="header-pills">
            <span className={`pill ${connState === 'open' ? 'run' : 'idle'}`}>
              <span className="dot" aria-hidden="true" />
              {connState === 'open' ? t('connected') : t('connecting')}
            </span>
            {connState === 'open' && loaded && (
              <span className={`pill ${waiting.length ? 'wait' : 'idle'}`}>
                {waiting.length > 0 && <span className="dot" aria-hidden="true" />}
                {waiting.length > 0 ? t('waitingCount', { n: waiting.length }) : t('allQuiet')}
              </span>
            )}
          </div>
        </header>

        {bypass && (
          <div className="notice" role="note">
            <IWarning size={20} />
            <div className="t-sub">
              <b>{t('bypassTitle')}</b>
              {t('bypass')}
            </div>
          </div>
        )}

        {waiting.length > 0 && (
          <section className="section" aria-labelledby="waiting-h">
            <div className="section-head">
              <h2 id="waiting-h">{t('needsYou')}</h2>
            </div>
            <div className="stack">
              {waiting.map((s) => {
                const q = questionPreview(tails[s.id] || s.approvalTail || '')
                return (
                  <button key={s.id} className="bubble wait-card press" onClick={() => onOpen({ kind: 'session', id: s.id })}>
                    <div className="wait-head">
                      <AgentMark launcherKey={s.launcherKey} />
                      <div className="card-main">
                        <span className="card-title">{s.title}</span>
                        <span className="card-sub">{s.workspaceName}</span>
                      </div>
                      <StatusPill status="waiting" />
                    </div>
                    {q && <p className="wait-q">{q}</p>}
                    <div className="wait-cta" style={q ? undefined : { marginTop: 14 }}>
                      {t('reply')}
                      <IChevronRight size={18} />
                    </div>
                  </button>
                )
              })}
            </div>
          </section>
        )}

        {loaded && windows.length === 0 && (
          <div className="empty">
            <IDesktop size={44} />
            <p>{t('noWindows')}</p>
          </div>
        )}

        {windows.map((w) => {
          const list = (byWindow.get(w.id) || []).sort((a, b) => a.startTime - b.startTime)
          const key = JSON.stringify([activeHost, w.workspace.replace(/\\/g, '/').toLowerCase()])
          const waitingCount = list.filter((s) => s.needsApproval).length
          return (
            <details key={w.id} className="section workspace-group" open={!collapsed.has(key)} onToggle={(e) => {
              const isOpen = e.currentTarget.open
              setCollapsed((prev) => {
                if (prev.has(key) === !isOpen) return prev
                const next = new Set(prev)
                if (isOpen) next.delete(key)
                else next.add(key)
                return next
              })
            }}>
              <summary className="section-head workspace-summary press">
                <IChevronRight size={18} className="workspace-chevron" />
                <h2 title={w.workspaceName}>{w.workspaceName}</h2>
                {list.length > 0 && <span className="count">{list.length}</span>}
                {waitingCount > 0 && <span className="pill wait">{t('waitingCount', { n: waitingCount })}</span>}
              </summary>
              <div className="stack">
                {list.map((s) => (
                  <button key={s.id} className="bubble session-card press" onClick={() => onOpen({ kind: 'session', id: s.id })}>
                    <AgentMark launcherKey={s.launcherKey} />
                    <span className="card-main">
                      <span className="card-title">{s.title}</span>
                      <span className="card-sub">
                        {ago(s.startTime) === t('justNow') ? t('justNow') : t('startedAgo', { t: ago(s.startTime) })}
                      </span>
                    </span>
                    <StatusPill status={sessionStatus(s, busy)} />
                    <IChevronRight size={18} className="chev" />
                  </button>
                ))}
                <button className="add-card press" onClick={() => onNew(w.id)}>
                  <IPlus size={20} />
                  {t('newTerminal')}
                </button>
              </div>
              <div className="tiles">
                <button className="bubble tile press" onClick={() => onOpen({ kind: 'status', windowId: w.id })}>
                  <IconMark tone="accent"><ICheck size={20} /></IconMark>
                  {t('status')}
                </button>
                <button className="bubble tile press" onClick={() => onOpen({ kind: 'file', windowId: w.id })}>
                  <IconMark tone="blue"><IDoc size={20} /></IconMark>
                  {t('file')}
                </button>
                <button className="bubble tile press" onClick={() => onOpen({ kind: 'handoff', windowId: w.id })}>
                  <IconMark tone="accent">
                    <IDoc size={20} />
                  </IconMark>
                  {t('handoff')}
                </button>
                <button className="bubble tile press" onClick={() => onOpen({ kind: 'git', windowId: w.id })}>
                  <IconMark tone="blue">
                    <IBranch size={20} />
                  </IconMark>
                  {t('git')}
                </button>
              </div>
            </details>
          )
        })}

        {/* 最近用過但目前沒開的工作區：點一下請電腦開起來，開好就直接跳新增終端 */}
        {workspaces.length > 0 && (
          <section className="section" aria-labelledby="ws-h">
            <div className="section-head">
              <h2 id="ws-h">{t('otherWorkspaces')}</h2>
            </div>
            <div className="stack">
              {workspaces.map((w) => (
                <button key={w.path} className="add-card press" onClick={() => onOpenWorkspace(w.path)}>
                  <IPlus size={20} />
                  {w.name}
                </button>
              ))}
            </div>
            <p className="footnote">{t('otherWorkspacesHint')}</p>
          </section>
        )}
      </main>
      {picker && <Menu items={hostMenu} onClose={() => setPicker(false)} />}
    </div>
  )
}

function NewTerminalSheet({
  launchers,
  bypass,
  onClose,
  onPick,
  onCowork
}: {
  launchers: RemoteLauncher[]
  bypass: boolean
  onClose: () => void
  onPick: (key: string) => void
  onCowork: () => void
}): JSX.Element {
  const agents = launchers.filter((l) => l.kind !== 'shell')
  const shells = launchers.filter((l) => l.kind === 'shell')
  const grid = (list: RemoteLauncher[]): JSX.Element => (
    <div className="launcher-grid">
      {list.map((l) => (
        <button key={l.key} className="bubble launcher press" onClick={() => onPick(l.key)}>
          <AgentMark launcherKey={l.key} />
          {l.title}
        </button>
      ))}
    </div>
  )
  return (
    <Sheet
      title={t('newTerminal')}
      onClose={onClose}
      leading={
        <button className="text-btn" onClick={onClose}>
          {t('cancel')}
        </button>
      }
    >
      {bypass && (
        <div className="notice" role="note">
          <IWarning size={20} />
          <div className="t-sub">
            <b>{t('bypassTitle')}</b>
            {t('bypass')}
          </div>
        </div>
      )}
      {agents.length > 0 && (
        <>
          <h3 className="sheet-label">{t('agents')}</h3>
          {grid(agents)}
        </>
      )}
      {shells.length > 0 && (
        <>
          <h3 className="sheet-label">{t('shells')}</h3>
          {grid(shells)}
        </>
      )}
      {/* 跟桌面終端的「＋」一樣：Cowork 和終端放在同一個新增選單 */}
      <h3 className="sheet-label">{t('cowork')}</h3>
      <div className="launcher-grid">
        <button className="bubble launcher press" onClick={onCowork}>
          <IconMark tone="accent" size={44}>
            <IHand size={22} />
          </IconMark>
          <span>
            {t('cowork')}
            <span className="card-sub launcher-sub">{t('cv_openHint')}</span>
          </span>
        </button>
      </div>
    </Sheet>
  )
}

/** 三選一的設定卡（語言、外觀）：選項名稱各自清楚，選中的用主色染色 */
function OptionCard<T extends string>({
  title,
  footer,
  value,
  options,
  onChange
}: {
  title: string
  footer: string
  value: T
  options: Array<{ v: T; label: string }>
  onChange: (v: T) => void
}): JSX.Element {
  const [current, setCurrent] = useState<T>(value)
  return (
    <>
      <div className="bubble lang-card">
        <span className="row-main">{title}</span>
        <div className="lang-options" role="radiogroup" aria-label={title}>
          {options.map((o) => (
            <button
              key={o.v}
              role="radio"
              aria-checked={current === o.v}
              className={`lang-option press ${current === o.v ? 'on' : ''}`}
              onClick={() => {
                setCurrent(o.v)
                onChange(o.v)
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      </div>
      <p className="footnote stack-gap">{footer}</p>
    </>
  )
}

/** 設定裡的「電腦」區：切換、改名、移除、新增。最多 MAX_HOSTS 台，一次連一台。 */
function ComputerList({
  hosts,
  activeHost,
  onChange,
  onSwitch
}: {
  hosts: SavedHost[]
  activeHost: string
  onChange: (list: SavedHost[]) => void
  onSwitch: (url: string) => void
}): JSX.Element {
  const [adding, setAdding] = useState(false)
  return (
    <>
      <div className="bubble">
        <ul className="list">
          {hosts.map((h) => (
            <li className="row host-row" key={h.url}>
              <button
                className="host-pick press"
                aria-label={t('connectTo', { name: h.label })}
                aria-pressed={h.url === activeHost}
                onClick={() => h.url !== activeHost && onSwitch(h.url)}
              >
                <IconMark tone={h.url === activeHost ? 'accent' : 'blue'} size={32}>
                  {h.url === activeHost ? <ICheck size={18} /> : <IDesktop size={18} />}
                </IconMark>
              </button>
              <span className="row-main host-main">
                <input
                  id={`host-${h.url}`}
                  className="host-name"
                  value={h.label}
                  aria-label={t('computerName')}
                  maxLength={40}
                  onChange={(e) => onChange(renameHost(h.url, e.target.value))}
                />
                <span className="host-url">
                  {hostOf(h.url)}
                  {h.url === currentUrl() ? ` · ${t('servedFromHere')}` : ''}
                </span>
              </span>
              <button
                className="circle-btn press"
                aria-label={t('forgetComputer')}
                disabled={h.url === currentUrl()}
                onClick={() => onChange(removeHost(h.url))}
              >
                <IStop size={16} />
              </button>
            </li>
          ))}
          <li className="row">
            <span className="row-main t-sub">{t('computersCount', { n: hosts.length, max: MAX_HOSTS })}</span>
            <button className="text-btn strong" disabled={hostsFull()} onClick={() => setAdding(true)}>
              {t('addComputer')}
            </button>
          </li>
        </ul>
      </div>
      <p className="footnote stack-gap">{t('computersFooter', { n: MAX_HOSTS })}</p>
      {adding && (
        <AddComputerSheet
          onClose={() => setAdding(false)}
          onAdd={(url) => {
            const list = addHost(url)
            if (list) {
              onChange(list)
              onSwitch(url)
            }
            setAdding(false)
          }}
        />
      )}
    </>
  )
}

function SettingsSheet({
  token,
  host,
  connState,
  hosts,
  activeHost,
  onHostsChange,
  onSwitchHost,
  bypass,
  onSetBypass,
  onClose,
  onUnpair
}: {
  token: string
  host: string
  connState: ConnState
  hosts: SavedHost[]
  activeHost: string
  onHostsChange: (list: SavedHost[]) => void
  onSwitchHost: (url: string) => void
  bypass: boolean
  onSetBypass: (enabled: boolean) => void
  onClose: () => void
  onUnpair: () => void
}): JSX.Element {
  const [confirmBypass, setConfirmBypass] = useState(false)
  const [push, setPush] = useState<'unsupported' | 'denied' | 'on' | 'off'>('off')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [confirm, setConfirm] = useState(false)

  useEffect(() => {
    pushState()
      .then(setPush)
      .catch(() => setPush('unsupported'))
  }, [])

  const toggle = async (on: boolean): Promise<void> => {
    setError(null)
    setBusy(true)
    try {
      if (on) await enablePush(token)
      else await disablePush(token)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setPush(await pushState().catch(() => 'off' as const))
      setBusy(false)
    }
  }

  const canPush = pushSupported() && isStandalone()
  const footer = !canPush ? t('notifNeedsHome') : push === 'denied' ? t('notifDenied') : t('notifFooter')

  return (
    <>
      <Sheet
        title={t('settings')}
        onClose={onClose}
        trailing={
          <button className="text-btn strong" onClick={onClose}>
            {t('done')}
          </button>
        }
      >
        <div className="bubble stack-gap">
          <ul className="list">
            <li className="row">
              <IconMark tone="accent" size={32}>
                <IDesktop size={18} />
              </IconMark>
              <span className="row-main">{t('computer')}</span>
              <span className="row-value">{host}</span>
            </li>
            <li className="row">
              <IconMark tone="blue" size={32}>
                <IPhone size={18} />
              </IconMark>
              <span className="row-main">{t('thisPhone')}</span>
              <span className={`pill ${connState === 'open' ? 'run' : 'idle'}`}>
                <span className="dot" aria-hidden="true" />
                {connState === 'open' ? t('connected') : t('connecting')}
              </span>
            </li>
          </ul>
        </div>
        <h3 className="sheet-label">{t('computers')}</h3>
        <ComputerList hosts={hosts} activeHost={activeHost} onChange={onHostsChange} onSwitch={onSwitchHost} />
        <div className="bubble">
          <ul className="list">
            <li className="row">
              <span className="row-main">{t('notifications')}</span>
              <Switch
                label={t('notifications')}
                checked={push === 'on'}
                disabled={!canPush || push === 'denied' || busy}
                onChange={(v) => void toggle(v)}
              />
            </li>
          </ul>
        </div>
        <p className="footnote stack-gap">{footer}</p>
        <OptionCard<ThemePref>
          title={t('appearance')}
          footer={t('themeFooter')}
          value={getThemePref()}
          options={[
            { v: 'auto', label: t('langAuto') },
            { v: 'light', label: t('themeLight') },
            { v: 'dark', label: t('themeDark') }
          ]}
          onChange={setThemePref}
        />
        {/* 語言名稱用各自的語言寫，切錯了也認得回來 */}
        <OptionCard<LangPref>
          title={t('language')}
          footer={t('langFooter')}
          value={getLangPref()}
          options={[
            { v: 'auto', label: t('langAuto') },
            { v: 'zh-TW', label: '繁體中文' },
            { v: 'en', label: 'English' }
          ]}
          onChange={setLangPref}
        />
        <div className="bubble">
          <ul className="list">
            <li className="row">
              <IconMark tone="danger-mark" size={32}>
                <IWarning size={18} />
              </IconMark>
              <span className="row-main">{t('bypassMode')}</span>
              <Switch
                label={t('bypassMode')}
                checked={bypass}
                disabled={connState !== 'open'}
                onChange={(v) => (v ? setConfirmBypass(true) : onSetBypass(false))}
              />
            </li>
          </ul>
        </div>
        <p className="footnote stack-gap">{t('bypassFooter')}</p>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <button className="btn danger press" onClick={() => setConfirm(true)}>
          {t('unpair')}
        </button>
        <p className="footnote stack-gap">{t('interfaceVersion', { version })}</p>
        <button className="btn press" onClick={() => window.location.reload()}>
          {t('reloadInterface')}
        </button>
      </Sheet>
      {confirmBypass && (
        <ConfirmSheet
          title={t('bypassConfirmTitle')}
          message={t('bypassConfirmBody')}
          action={t('bypassEnable')}
          onCancel={() => setConfirmBypass(false)}
          onConfirm={() => {
            setConfirmBypass(false)
            onSetBypass(true)
          }}
        />
      )}
      {confirm && (
        <ConfirmSheet
          title={t('unpairTitle')}
          message={t('unpairBody')}
          action={t('unpair')}
          onCancel={() => setConfirm(false)}
          onConfirm={onUnpair}
        />
      )}
    </>
  )
}

// —— 交接筆記 / Git ——

function DetailView({
  conn,
  view,
  workspace,
  hostName,
  onBack
}: {
  conn: RemoteConnection
  view: { kind: 'handoff' | 'git'; windowId: number }
  workspace: string
  hostName: string
  onBack: () => void
}): JSX.Element {
  const [handoff, setHandoff] = useState<string | null | undefined>(undefined)
  const [git, setGit] = useState<{ branch: string | null; ahead: number; behind: number; files: RemoteGitFile[]; error?: string } | null>(null)
  const scrolled = useScrolled(4)

  useEffect(() => {
    const off = conn.onMessage((m) => {
      if (m.t === 'handoff' && m.windowId === view.windowId) setHandoff(m.text)
      if (m.t === 'git' && m.windowId === view.windowId) setGit(m)
    })
    const request = (): void => conn.send({ t: view.kind, windowId: view.windowId })
    const offState = conn.onState((s) => s === 'open' && request())
    request()
    return () => {
      off()
      offState()
    }
  }, [view.kind, view.windowId])

  return (
    <div className="detail">
      <NavBar
        title={view.kind === 'git' ? t('git') : t('handoff')}
        subtitle={workspace}
        backLabel={hostName}
        onBack={onBack}
        scrolled={scrolled}
        clear
      />
      <main className="content detail-body">
        {view.kind === 'handoff' &&
          (handoff === undefined ? (
            <p className="empty">{t('loading')}</p>
          ) : handoff === null ? (
            <div className="empty">
              <IDoc size={44} />
              <p>{t('noHandoff')}</p>
            </div>
          ) : (
            <article className="bubble markdown">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{handoff}</ReactMarkdown>
            </article>
          ))}
        {view.kind === 'git' &&
          (!git ? (
            <p className="empty">{t('loading')}</p>
          ) : git.error ? (
            <p className="form-error" role="alert">
              <IWarning size={18} />
              {git.error}
            </p>
          ) : (
            <>
              <div className="bubble branch-card">
                <IconMark tone="blue">
                  <IBranch size={20} />
                </IconMark>
                <span className="card-main card-title">{git.branch}</span>
                {git.ahead > 0 && <span className="pill run">↑ {git.ahead}</span>}
                {git.behind > 0 && <span className="pill wait">↓ {git.behind}</span>}
              </div>
              <div className="section-head">
                <h2>{git.files.length ? t('changes', { n: git.files.length }) : t('clean')}</h2>
              </div>
              <div className="files">
                {git.files.map((f) => {
                  const untracked = f.index === '?' || f.workingDir === '?'
                  const staged = !untracked && !!f.index.trim()
                  const flag = untracked ? '?' : (staged ? f.index : f.workingDir).trim() || '•'
                  return (
                    <div key={f.path} className="bubble file">
                      <span className={`git-flag ${staged ? 'staged' : untracked ? 'untracked' : ''}`}>{flag}</span>
                      <span className="git-path">{f.path}</span>
                    </div>
                  )
                })}
              </div>
            </>
          ))}
      </main>
    </div>
  )
}
