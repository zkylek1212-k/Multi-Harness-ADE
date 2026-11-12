import { useEffect, useRef, useState } from 'react'
import type { ChannelEvent, ChannelSummary, ChannelView, NodeRuntime } from '../../shared/canvas'
import type { RemoteConnection } from './conn'
import { AgentMark, IWarning } from './icons'
import { ago, t, type Key } from './i18n'
import { ConfirmSheet, NavBar } from './ui'

// 手機版 Cowork 2.0（cowork2.0-implement.md §3.9）：Channel 清單、節點狀態與審批、事件時間軸、送訊息、第一段停止。
// 只用 PHONE_CANVAS_OPS；建立／關閉節點、合併、捨棄、全部結束、解除停止只在桌面。點節點進現有的終端畫面。
// 審批與訊息都帶 requestId／sessionGeneration，桌面只接受一次；過期就顯示錯誤並重新整理，不自動重送。

const ERRORS = { cv_err_stale_request: 1, cv_err_stopped: 1, cv_err_not_ready: 1, cv_err_not_allowed: 1, cv_err_offline: 1, cv_err_timeout: 1 }

const statusTone =(rt: NodeRuntime | undefined): string =>
  !rt ? 'idle' : rt.approval ? 'wait' : rt.status === 'error' ? 'danger' : rt.status === 'running' ? (rt.busy ? 'run' : 'idle') : 'idle'

function StatusPill({ rt }: { rt: NodeRuntime | undefined }): JSX.Element {
  const key = !rt ? 'exited' : rt.approval ? 'waiting' : rt.status === 'running' ? (rt.busy ? 'running' : 'idle') : rt.status
  return (
    <span className={`pill ${statusTone(rt)}`}>
      <span className="dot" aria-hidden="true" />
      {t(`cv_${key.replace('-', '_')}` as Key)}
    </span>
  )
}

function eventText(e: ChannelEvent, name: (id: string) => string): string {
  switch (e.t) {
    case 'node-started':
      return t('cv_ev_started', { node: name(e.node) })
    case 'node-exited':
      return t('cv_ev_exited', { node: name(e.node) })
    case 'approval':
      return t(`cv_ev_approval_${e.decision || e.status}` as Key, { node: name(e.node) })
    case 'handoff':
      return t('cv_ev_handoff', { from: name(e.from), to: name(e.to) })
    case 'merged':
    case 'discarded':
      return t(`cv_ev_${e.t}` as Key, { node: name(e.node) })
    default:
      return t(`cv_ev_${e.t.replace('-', '_')}` as Key)
  }
}

export default function CoworkView({
  conn,
  windowId,
  workspace,
  hostName,
  onBack,
  onOpenSession
}: {
  conn: RemoteConnection
  windowId: number
  workspace: string
  hostName: string
  onBack: () => void
  onOpenSession: (ptyId: string) => void
}): JSX.Element {
  const [list, setList] = useState<ChannelSummary[] | null>(null)
  const [channelId, setChannelId] = useState<string | null>(null)
  const [view, setView] = useState<ChannelView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [confirmStop, setConfirmStop] = useState(false)
  const [message, setMessage] = useState<{ node: string; text: string } | null>(null)
  const idRef = useRef(channelId)
  idRef.current = channelId

  const known = (k: string): k is Key => k in ERRORS
  const fail = (code: string, msg?: string): void => {
    const k = `cv_err_${code.replace(/-/g, '_')}`
    setError(known(k) ? t(k) : msg || code)
  }

  useEffect(() => {
    const load = (): void => {
      void conn.canvas(windowId, 'channels.list').then((r) => r.ok && setList(r.data))
      const id = idRef.current
      if (id) void conn.canvas(windowId, 'channel.get', { channelId: id }).then((r) => r.ok && idRef.current === id && setView(r.data))
    }
    const off = conn.onMessage((m) => {
      if (m.t !== 'canvasUpdate') return
      if (m.update.channelId === idRef.current) setView(m.update.view ?? null)
      void conn.canvas(windowId, 'channels.list').then((r) => r.ok && setList(r.data))
    })
    const offState = conn.onState((s) => s === 'open' && load())
    load()
    return () => {
      off()
      offState()
    }
  }, [conn, windowId])

  const run = async (fn: () => Promise<{ ok: boolean; code?: string; message?: string }>): Promise<boolean> => {
    setBusy(true)
    setError(null)
    const r = await fn()
    setBusy(false)
    if (!r.ok) {
      fail(r.code || 'unknown', r.message)
      // A stale card refreshes from the desktop instead of being retried
      if (idRef.current) void conn.canvas(windowId, 'channel.get', { channelId: idRef.current }).then((x) => x.ok && setView(x.data))
    }
    return r.ok
  }

  if (channelId && view) {
    const c = view.channel
    const name = (id: string): string => {
      const n = c.nodes.find((x) => x.id === id)
      return n ? `${n.agent} · ${n.id.slice(0, 4)}` : id.slice(0, 4)
    }
    return (
      <div className="session">
        <NavBar
          title={c.name}
          subtitle={workspace}
          backLabel={t('cowork')}
          onBack={() => {
            setChannelId(null)
            setView(null)
          }}
          trailing={
            <button className="btn danger" disabled={busy || view.stop.requested} onClick={() => setConfirmStop(true)}>
              {view.stop.requested ? t('cv_stopRequested') : t('cv_stop')}
            </button>
          }
        />
        <main className="workspace-pane">
          {view.stop.requested && <p className="footnote">{t('cv_stopNote')}</p>}
          <div className="stack">
            {c.nodes.length === 0 && <p className="empty">{t('cv_noNodes')}</p>}
            {c.nodes.map((n) => {
              const rt = view.runtime[n.id]
              const a = rt?.approval
              return (
                <div key={n.id} className="bubble form-card">
                  <button className="session-card press" disabled={!rt?.ptyId} onClick={() => rt?.ptyId && onOpenSession(rt.ptyId)}>
                    <AgentMark launcherKey={n.agent} />
                    <span className="card-main">
                      <span className="card-title">{name(n.id)}</span>
                      <span className="card-sub">{n.worktree ? n.worktree.branch : t('cv_repoRoot')}</span>
                    </span>
                    <StatusPill rt={rt} />
                  </button>
                  {a && rt?.sessionGeneration && (
                    <div className="cw-card">
                      <p className="card-title">{a.title}</p>
                      {a.hook?.files.map((f) => (
                        <p key={f.path} className="card-sub">
                          {f.path} +{f.added} -{f.removed}
                        </p>
                      ))}
                      {!a.hook && a.tail && <pre className="cw-pre">{a.tail.split('\n').slice(-6).join('\n')}</pre>}
                      {a.canApprove || a.canReject ? (
                        <div className="cw-actions">
                          {a.canReject && (
                            <button className="btn" disabled={busy} onClick={() => run(() => conn.canvas(windowId, 'node.reject', { channelId: c.id, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration }))}>
                              {t('cv_reject')}
                            </button>
                          )}
                          {a.canApprove && !view.stop.requested && (
                            <button className="btn prominent" disabled={busy} onClick={() => run(() => conn.canvas(windowId, 'node.approve', { channelId: c.id, nodeId: n.id, requestId: a.requestId, sessionGeneration: a.sessionGeneration }))}>
                              {t('cv_approve')}
                            </button>
                          )}
                        </div>
                      ) : (
                        <p className="footnote">{t('cv_answerInTerminal')}</p>
                      )}
                    </div>
                  )}
                  {rt?.status === 'running' && rt.sessionGeneration && !a && (
                    message?.node === n.id ? (
                      <div className="field">
                        <textarea className="cw-input" rows={3} maxLength={16000} value={message.text} placeholder={t('cv_messagePlaceholder')} onChange={(e) => setMessage({ node: n.id, text: e.target.value })} />
                        <div className="cw-actions">
                          <button className="btn" onClick={() => setMessage(null)}>{t('cancel')}</button>
                          <button
                            className="btn prominent"
                            disabled={busy || !message.text.trim() || !rt.inputReady}
                            onClick={async () => {
                              const ok = await run(() => conn.canvas(windowId, 'node.message', { channelId: c.id, nodeId: n.id, sessionGeneration: rt.sessionGeneration!, text: message.text }))
                              if (ok) setMessage(null)
                            }}
                          >
                            {t('cv_paste')}
                          </button>
                        </div>
                        {!rt.inputReady && <p className="footnote">{t('cv_notReady')}</p>}
                      </div>
                    ) : (
                      <button className="btn" disabled={view.stop.requested} onClick={() => setMessage({ node: n.id, text: '' })}>{t('cv_message')}</button>
                    )
                  )}
                </div>
              )
            })}
          </div>
          <div className="section-head"><h2>{t('cv_feed')}</h2></div>
          <div className="stack">
            {c.events.length === 0 && <p className="empty">{t('cv_noEvents')}</p>}
            {[...c.events].reverse().slice(0, 100).map((e, i) => (
              <p key={i} className="card-sub">
                <span>{eventText(e, name)}</span> <span className="footnote">{ago(e.at)}</span>
              </p>
            ))}
          </div>
          {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
        </main>
        {confirmStop && (
          <ConfirmSheet
            title={t('cv_stopTitle')}
            message={t('cv_stopConfirm')}
            action={t('cv_stop')}
            onCancel={() => setConfirmStop(false)}
            onConfirm={() => {
              setConfirmStop(false)
              void run(() => conn.canvas(windowId, 'stop'))
            }}
          />
        )}
      </div>
    )
  }

  return (
    <div className="session">
      <NavBar title={t('cowork')} subtitle={workspace} backLabel={hostName} onBack={onBack} />
      <main className="workspace-pane">
        <p className="footnote">{t('cv_desktopOnly')}</p>
        <div className="stack">
          {list === null ? (
            <p className="empty">{t('loading')}</p>
          ) : list.length === 0 ? (
            <p className="empty">{t('cv_none')}</p>
          ) : (
            list.map((ch) => (
              <button
                key={ch.id}
                className="bubble session-card press"
                onClick={() => {
                  setChannelId(ch.id)
                  setView(null)
                  void conn.canvas(windowId, 'channel.get', { channelId: ch.id }).then((r) => (r.ok ? setView(r.data) : fail(r.code, r.message)))
                }}
              >
                <span className="card-main">
                  <span className="card-title"># {ch.name}</span>
                  <span className="card-sub">{t('cv_nodes', { n: ch.nodeCount })} · {ago(ch.updatedAt)}</span>
                </span>
                {ch.pendingApprovals > 0 && <span className="pill wait"><span className="dot" aria-hidden="true" />{ch.pendingApprovals}</span>}
              </button>
            ))
          )}
        </div>
        {channelId && !view && <p className="empty">{t('loading')}</p>}
        {error && <p className="form-error" role="alert"><IWarning size={18} />{error}</p>}
      </main>
    </div>
  )
}
