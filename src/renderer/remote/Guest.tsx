import { useEffect, useMemo, useState } from 'react'
import type { ChangesSummary, ChannelView } from '../../shared/canvas'
import ChatBlock from './ChatBlock'
import { joinOverWs, RemoteConnection, type ConnState } from './conn'
import { AgentMark, IWarning } from './icons'
import { t } from './i18n'

// 同事的瀏覽器（cowork-agui-implement.md §13）：用邀請連結加入一個 channel，可以看對話與改動、送訊息、核准、停止這一輪。
// 只能用那一個 channel；其他畫面（終端、檔案、設定）都沒有。token 存在這個瀏覽器，撤銷後立即失效。
const KEY = 'wb-guest-token'
const read = (): string | null => {
  try {
    return localStorage.getItem(KEY)
  } catch {
    return null
  }
}
const write = (v: string | null): void => {
  try {
    if (v) localStorage.setItem(KEY, v)
    else localStorage.removeItem(KEY)
  } catch {
    /* 私密模式：這次的分頁裡還是能用 */
  }
}
const joinCode = (): string | null => /[#&]join=([A-Za-z0-9_-]+)/.exec(location.hash)?.[1] ?? null

export const isGuestPage = (): boolean => !!joinCode() || !!read()

export default function GuestApp(): JSX.Element {
  const [token, setToken] = useState<string | null>(() => (joinCode() ? null : read()))
  const [ended, setEnded] = useState(false)
  if (ended) return <main className="pair content"><div className="pair-hero"><h1>{t('gu_endedTitle')}</h1><p>{t('gu_ended')}</p></div></main>
  if (!token) {
    return (
      <Join
        onJoined={(tok) => {
          write(tok)
          history.replaceState(null, '', location.pathname)
          setToken(tok)
        }}
      />
    )
  }
  return (
    <Channel
      token={token}
      onEnded={() => {
        write(null)
        setEnded(true)
      }}
    />
  )
}

function Join({ onJoined }: { onJoined: (token: string) => void }): JSX.Element {
  const code = joinCode()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(code ? null : t('gu_noInvite'))
  const join = async (): Promise<void> => {
    if (!code || !name.trim()) return
    setBusy(true)
    setError(null)
    try {
      onJoined(await joinOverWs(location.origin, code, name.trim()))
    } catch (e) {
      setError((e as Error).message === 'invalid' ? t('gu_invalid') : t('gu_unreachable'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <main className="pair content">
      <div className="pair-hero">
        <h1>{t('gu_joinTitle')}</h1>
        <p>{t('gu_joinDesc')}</p>
      </div>
      <div className="bubble form-card">
        <div className="field">
          <label htmlFor="gu-name">{t('gu_name')}</label>
          <input id="gu-name" className="cw-input" maxLength={40} value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void join()} />
        </div>
        <p className="footnote">{t('gu_risk')}</p>
        {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
      </div>
      <div className="pair-actions">
        <button className="btn prominent" disabled={busy || !code || !name.trim()} onClick={() => void join()}>{busy ? t('gu_joining') : t('gu_join')}</button>
      </div>
    </main>
  )
}

/** A terminal Bot gets the message when its prompt is idle, like typing it (chat.send) */
function TermMessage({ conn, channelId, nodeId, run, busy }: { conn: RemoteConnection; channelId: string; nodeId: string; run: (fn: () => Promise<{ ok: boolean }>) => Promise<boolean>; busy: boolean }): JSX.Element {
  const [text, setText] = useState('')
  return (
    <div className="field">
      <textarea className="cw-input" rows={2} maxLength={16000} value={text} placeholder={t('cv_chatPlaceholder')} onChange={(e) => setText(e.target.value)} />
      <div className="cw-actions">
        <button className="btn prominent" disabled={busy || !text.trim()} onClick={async () => (await run(() => conn.canvas(0, 'chat.send', { channelId, to: nodeId, text: text.trim() }))) && setText('')}>{t('cv_chatSend')}</button>
      </div>
    </div>
  )
}

function Changes({ conn, channelId, nodeId }: { conn: RemoteConnection; channelId: string; nodeId: string }): JSX.Element {
  const [sum, setSum] = useState<ChangesSummary | null>(null)
  const [open, setOpen] = useState(false)
  const load = (): void => void conn.canvas(0, 'node.changes', { channelId, nodeId }).then((r) => r.ok && setSum(r.data))
  if (!open) return <button className="btn" onClick={() => { setOpen(true); load() }}>{t('gu_changes')}</button>
  const files = sum ? [...sum.committed, ...sum.staged, ...sum.unstaged, ...sum.untracked] : []
  return (
    <div className="stack">
      <div className="cw-actions">
        <button className="btn" onClick={load}>{t('gu_refresh')}</button>
        <button className="btn" onClick={() => setOpen(false)}>{t('gu_hide')}</button>
      </div>
      {sum && files.length === 0 && <p className="footnote">{t('gu_noChanges')}</p>}
      {sum?.commits.map((c) => <p key={c} className="footnote">{c}</p>)}
      {files.map((f, i) => (
        <details key={`${f.path}${i}`}>
          <summary className="card-sub">{f.status} {f.path}</summary>
          {f.text && <pre className="cw-pre">{f.text}</pre>}
        </details>
      ))}
    </div>
  )
}

function Channel({ token, onEnded }: { token: string; onEnded: () => void }): JSX.Element {
  const conn = useMemo(() => new RemoteConnection(location.origin, token, 'guest'), [token])
  const [me, setMe] = useState<{ name: string; channelId: string } | null>(null)
  const [view, setView] = useState<ChannelView | null>(null)
  const [state, setState] = useState<ConnState>('connecting')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let channelId = ''
    const offState = conn.onState((s) => {
      setState(s)
      if (s === 'unauthorized') onEnded()
    })
    const off = conn.onMessage((m) => {
      if (m.t === 'authed' && 'guest' in m && m.guest) {
        channelId = m.guest.channelId
        setMe({ name: m.guest.name, channelId })
        void conn.canvas(0, 'channel.get', { channelId }).then((r) => r.ok && setView(r.data))
      }
      if (m.t === 'canvasUpdate' && m.update.channelId === channelId) setView(m.update.view ?? null)
    })
    conn.start()
    return () => {
      off()
      offState()
      conn.stop()
    }
  }, [conn, onEnded])

  const run = async (fn: () => Promise<{ ok: boolean; code?: string; message?: string }>): Promise<boolean> => {
    setBusy(true)
    setError(null)
    const r = await fn()
    setBusy(false)
    if (!r.ok) setError(r.message || r.code || 'error')
    return r.ok
  }

  if (!me || !view) return <main className="pair content"><p className="empty">{state === 'connecting' ? t('connecting') : t('offline')}</p></main>
  const c = view.channel
  return (
    <div className="session guest">
      <header className="guest-head">
        <b># {c.name}</b>
        <span className="footnote">{t('gu_as', { name: me.name })}</span>
        <span className={`pill ${state === 'open' ? 'run' : 'wait'}`}><span className="dot" aria-hidden="true" />{state === 'open' ? t('connected') : t('offline')}</span>
      </header>
      <main className="workspace-pane guest-main">
        <p className="footnote">{t('gu_risk')}</p>
        {c.nodes.map((n) => {
          const rt = view.runtime[n.id]
          const a = rt?.approval
          return (
            <section key={n.id} className="bubble form-card">
              <div className="session-card">
                <AgentMark launcherKey={n.agent} />
                <span className="card-main">
                  <span className="card-title">{n.agent}{n.role ? ` · ${n.role}` : ''}</span>
                  <span className="card-sub">{n.worktree ? n.worktree.branch : ''}</span>
                </span>
                {rt?.inRun && <button className="btn" disabled={busy} onClick={() => run(() => conn.canvas(0, 'chat.interrupt', { channelId: c.id, nodeId: n.id }))}>{t('gu_stop')}</button>}
              </div>
              {n.mode === 'chat' && <ChatBlock conn={conn} windowId={0} view={view} nodeId={n.id} run={run} busy={busy} />}
              {n.mode !== 'chat' && a && rt?.sessionGeneration && (
                <div className="cw-card">
                  <p className="card-title">{a.title}</p>
                  {a.hook?.files.map((f) => <p key={f.path} className="card-sub">{f.path} +{f.added} -{f.removed}</p>)}
                  {!a.hook && a.tail && <pre className="cw-pre">{a.tail.split('\n').slice(-6).join('\n')}</pre>}
                  <div className="cw-actions">
                    {a.canReject && <button className="btn" disabled={busy} onClick={() => run(() => conn.canvas(0, 'node.reject', { channelId: c.id, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration }))}>{t('cv_reject')}</button>}
                    {a.canApprove && <button className="btn prominent" disabled={busy} onClick={() => run(() => conn.canvas(0, 'node.approve', { channelId: c.id, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration }))}>{t('cv_approve')}</button>}
                  </div>
                </div>
              )}
              {n.mode !== 'chat' && rt?.status === 'running' && <TermMessage conn={conn} channelId={c.id} nodeId={n.id} run={run} busy={busy} />}
              {n.worktree && <Changes conn={conn} channelId={c.id} nodeId={n.id} />}
            </section>
          )
        })}
        {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
      </main>
    </div>
  )
}
