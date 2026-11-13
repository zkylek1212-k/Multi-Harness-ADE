// 右欄 Inspector（§2.3、§3.3、§3.5）：選取節點的 worktree 與改動、提交全部、合併／清理／捨棄、交接草稿與訊息。
// 每個會動資料的操作都先預覽、明確按下才做；後端再核對一次（token 綁定來源與目標），畫面這裡不自己判斷安全。
import { useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import type { ChangedFile, ChangesSummary, ChannelView, DiscardPreview, LegacyWorktree, MergePreview } from '../../../../shared/canvas'
import type { CanvasCall } from './CanvasView'
import { NodeTeamSection, RelayCards, TeamInspector } from './TeamPanels'

export type Selection = { kind: 'node' | 'edge' | 'team'; id: string } | null

export default function Inspector({
  view,
  selection,
  call,
  agentName,
  nodeName,
  onPickTeam
}: {
  view: ChannelView | null
  selection: Selection
  call: CanvasCall
  agentName: (id: string) => string
  nodeName: (id: string) => string
  onPickTeam: (teamId: string) => void
}): JSX.Element {
  const { t } = useTranslation()
  if (!view) return <aside className="cv-right" />
  if (selection?.kind === 'team' && view.teams.some((x) => x.id === selection.id)) {
    return <TeamInspector key={selection.id} view={view} teamId={selection.id} call={call} nodeName={nodeName} />
  }
  if (selection?.kind === 'edge') {
    const e = view.channel.edges.find((x) => x.id === selection.id)
    if (e) return <EdgeInspector key={e.id} view={view} edgeId={e.id} call={call} nodeName={nodeName} />
  }
  if (selection?.kind === 'node') {
    const n = view.channel.nodes.find((x) => x.id === selection.id)
    if (n) return <NodeInspector key={n.id} view={view} nodeId={n.id} call={call} agentName={agentName} onPickTeam={onPickTeam} />
  }
  return (
    <aside className="cv-right">
      <p className="cv-muted">{t('canvas.selectHint')}</p>
      <RelayCards view={view} call={call} nodeName={nodeName} />
      <LegacyInventory call={call} />
    </aside>
  )
}

function Paste({ view, nodeId, text, call }: { view: ChannelView; nodeId: string; text: string; call: CanvasCall }): JSX.Element {
  const { t } = useTranslation()
  const rt = view.runtime[nodeId]
  const [done, setDone] = useState<string | null>(null)
  const ready = !!rt?.inputReady && !!rt.sessionGeneration && !view.stop.requested
  return (
    <div className="cv-row">
      <button className="cv-btn" disabled={!text.trim()} onClick={() => navigator.clipboard.writeText(text).then(() => setDone(t('canvas.copied')))}>
        {t('canvas.copy')}
      </button>
      <button
        className="cv-btn primary"
        disabled={!text.trim() || !ready}
        title={ready ? t('canvas.pasteHint') : t('canvas.notReady')}
        onClick={async () => {
          const r = await call('node.message', { channelId: view.channel.id, nodeId, sessionGeneration: rt!.sessionGeneration!, text })
          if (r !== undefined) setDone(t('canvas.pasted'))
        }}
      >
        {t('canvas.paste')}
      </button>
      {done && <span className="cv-muted">{done}</span>}
    </div>
  )
}

function EdgeInspector({ view, edgeId, call, nodeName }: { view: ChannelView; edgeId: string; call: CanvasCall; nodeName: (id: string) => string }): JSX.Element {
  const { t } = useTranslation()
  const e = view.channel.edges.find((x) => x.id === edgeId)!
  const [text, setText] = useState(e.text)
  return (
    <aside className="cv-right">
      <h3 className="cv-h">{t('canvas.handoff')}</h3>
      <p className="cv-muted">
        {nodeName(e.from)} → {nodeName(e.to)} · {new Date(e.at).toLocaleString()}
      </p>
      <p className="cv-muted">{t('canvas.handoffNote')}</p>
      <textarea className="cv-textarea" rows={14} value={text} onChange={(x) => setText(x.target.value)} />
      <Paste view={view} nodeId={e.to} text={text} call={call} />
      <button className="cv-btn" onClick={() => void call('edge.remove', { channelId: view.channel.id, edgeId })}>
        {t('canvas.removeEdge')}
      </button>
    </aside>
  )
}

function Files({ title, files }: { title: string; files: ChangedFile[] }): JSX.Element | null {
  const [open, setOpen] = useState<string | null>(null)
  if (files.length === 0) return null
  return (
    <div className="cv-files">
      <div className="cv-files-title">{title}</div>
      {files.map((f) => (
        <div key={f.path}>
          <button className="cv-file" onClick={() => setOpen(open === f.path ? null : f.path)}>
            <span className="cv-file-status">{f.status}</span>
            <span className="cv-file-path">{f.path}</span>
            {(f.binary || f.truncated) && <span className="cv-muted">{f.binary ? 'binary' : '…'}</span>}
          </button>
          {open === f.path && f.text && <pre className="cv-diff">{f.text}</pre>}
        </div>
      ))}
    </div>
  )
}

function NodeInspector({ view, nodeId, call, agentName, onPickTeam }: { view: ChannelView; nodeId: string; call: CanvasCall; agentName: (id: string) => string; onPickTeam: (teamId: string) => void }): JSX.Element {
  const { t } = useTranslation()
  const n = view.channel.nodes.find((x) => x.id === nodeId)!
  const rt = view.runtime[nodeId]
  const ended = !rt || !['running', 'creating', 'closing'].includes(rt.status)
  const ids = { channelId: view.channel.id, nodeId }
  const [message, setMessage] = useState('')
  const [changes, setChanges] = useState<ChangesSummary | null>(null)
  const [commitMsg, setCommitMsg] = useState('')
  const [branches, setBranches] = useState<{ current: string; all: string[] } | null>(null)
  const [target, setTarget] = useState('')
  const [merge, setMerge] = useState<MergePreview | null>(null)
  const [discard, setDiscard] = useState<DiscardPreview | null>(null)
  const [busy, setBusy] = useState(false)

  const refresh = async (): Promise<void> => {
    setMerge(null)
    setDiscard(null)
    if (!n.worktree) return
    const c = await call('node.changes', ids)
    if (c) setChanges(c)
  }
  useEffect(() => {
    void refresh()
    window.api.git
      .branches()
      .then((b) => {
        setBranches(b)
        setTarget(b.current)
      })
      .catch(() => {})
    // 節點狀態一變（例如剛結束）就重讀改動
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodeId, rt?.status])

  const run = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    await fn()
    setBusy(false)
  }
  const uncommitted = changes ? [...new Set([...changes.staged, ...changes.unstaged, ...changes.untracked].map((f) => f.path))] : []

  return (
    <aside className="cv-right">
      <h3 className="cv-h">{agentName(n.agent)} · {n.id.slice(0, 4)}</h3>
      <dl className="cv-dl">
        <dt>{t('canvas.status')}</dt>
        <dd>{t(`canvas.status_${(rt?.status || 'exited').replace('-', '_')}`)}{rt?.error ? ` — ${rt.error}` : ''}</dd>
        <dt>{t('canvas.branch')}</dt>
        <dd>{n.worktree ? n.worktree.branch : t('canvas.repoRoot')}</dd>
        {n.worktree && (
          <>
            <dt>{t('canvas.worktree')}</dt>
            <dd className="cv-path" title={n.worktree.path}>{n.worktree.path}</dd>
            <dt>{t('canvas.base')}</dt>
            <dd>{n.worktree.baseCommit.slice(0, 10)}</dd>
          </>
        )}
        {rt?.proxyMode && (
          <>
            <dt>{t('canvas.proxy')}</dt>
            <dd>{rt.proxyMode}</dd>
          </>
        )}
      </dl>
      <NodeTeamSection view={view} nodeId={nodeId} call={call} onPickTeam={onPickTeam} />

      {rt?.status === 'running' && (
        <section className="cv-sec">
          <div className="cv-sec-title">{t('canvas.message')}</div>
          <textarea className="cv-textarea" rows={4} value={message} placeholder={t('canvas.messagePlaceholder')} onChange={(e) => setMessage(e.target.value)} />
          <Paste view={view} nodeId={nodeId} text={message} call={call} />
        </section>
      )}

      {n.worktree && (
        <section className="cv-sec">
          <div className="cv-sec-title">
            {t('canvas.changes')}
            <button className="cv-icon-btn" onClick={() => void refresh()} title={t('canvas.refresh')}>↻</button>
          </div>
          {changes ? (
            <>
              {changes.commits.length > 0 && (
                <div className="cv-files">
                  <div className="cv-files-title">{t('canvas.commits')}</div>
                  {changes.commits.map((c) => (
                    <div key={c} className="cv-commit">{c.slice(0, 10)} {c.slice(41)}</div>
                  ))}
                </div>
              )}
              <Files title={t('canvas.committed')} files={changes.committed} />
              <Files title={t('canvas.staged')} files={changes.staged} />
              <Files title={t('canvas.unstaged')} files={changes.unstaged} />
              <Files title={t('canvas.untracked')} files={changes.untracked} />
              {changes.commits.length === 0 && uncommitted.length === 0 && <p className="cv-muted">{t('canvas.noChanges')}</p>}
            </>
          ) : (
            <p className="cv-muted">…</p>
          )}
        </section>
      )}

      {n.worktree && uncommitted.length > 0 && (
        <section className="cv-sec">
          <div className="cv-sec-title">{t('canvas.commitAll')}</div>
          <p className="cv-muted">{t('canvas.commitAllHint', { n: uncommitted.length })}</p>
          <input className="cv-input" value={commitMsg} placeholder={t('canvas.commitMessage')} onChange={(e) => setCommitMsg(e.target.value)} />
          <button
            className="cv-btn primary"
            disabled={busy || !ended || !commitMsg.trim()}
            title={ended ? '' : t('canvas.endFirst')}
            onClick={() =>
              void run(async () => {
                if (await call('node.commitAll', { ...ids, files: uncommitted, message: commitMsg })) {
                  setCommitMsg('')
                  await refresh()
                }
              })
            }
          >
            {t('canvas.commit')}
          </button>
        </section>
      )}

      {n.worktree && ended && (
        <section className="cv-sec">
          <div className="cv-sec-title">{t('canvas.merge')}</div>
          <div className="cv-row">
            <select className="cv-input" value={target} onChange={(e) => { setTarget(e.target.value); setMerge(null) }}>
              {(branches?.all || [target]).filter((b) => b && !b.startsWith('cowork/')).map((b) => (
                <option key={b} value={b}>{b}{b === branches?.current ? ` (${t('canvas.current')})` : ''}</option>
              ))}
            </select>
            <button className="cv-btn" disabled={busy || !changes?.clean} title={changes?.clean ? '' : t('canvas.mergeNeedsClean')} onClick={() => void run(async () => setMerge((await call('node.mergePreview', { ...ids, target })) ?? null))}>
              {t('canvas.preview')}
            </button>
          </div>
          {merge && (
            <div className="cv-preview">
              <p>{t('canvas.mergeInto', { source: merge.source.branch, target: merge.target.branch })}</p>
              <ul>{merge.commits.map((c) => <li key={c}>{c.slice(0, 10)} {c.slice(41)}</li>)}</ul>
              <ul>{merge.files.map((f) => <li key={f.path}>{f.status} {f.path}</li>)}</ul>
              <button
                className="cv-btn primary"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await call('node.merge', { ...ids, token: merge.token })
                    await refresh()
                  })
                }
              >
                {t('canvas.mergeConfirm')}
              </button>
            </div>
          )}
        </section>
      )}

      {ended && (
        <section className="cv-sec">
          <div className="cv-sec-title">{t('canvas.finish')}</div>
          <p className="cv-muted">{n.worktree ? t('canvas.cleanupHint') : t('canvas.removeRootHint')}</p>
          <div className="cv-row">
            <button className="cv-btn" disabled={busy} onClick={() => void run(() => call('node.cleanup', ids))}>
              {n.worktree ? t('canvas.cleanup') : t('canvas.removeNode')}
            </button>
            {n.worktree && (
              <button className="cv-btn danger" disabled={busy} onClick={() => void run(async () => setDiscard((await call('node.discardPreview', ids)) ?? null))}>
                {t('canvas.discard')}
              </button>
            )}
          </div>
          {discard && (
            <div className="cv-preview danger">
              <p>{t('canvas.discardWarn')}</p>
              {discard.unmergedCommits.length > 0 && <ul>{discard.unmergedCommits.map((c) => <li key={c}>{c.slice(0, 10)} {c.slice(41)}</li>)}</ul>}
              <ul>
                {[...discard.changes.staged, ...discard.changes.unstaged, ...discard.changes.untracked].map((f) => (
                  <li key={`${f.status}${f.path}`}>{f.status} {f.path}</li>
                ))}
                {discard.ignored.map((p) => <li key={`i${p}`}>{t('canvas.ignored')} {p}</li>)}
              </ul>
              <button className="cv-btn danger" disabled={busy} onClick={() => window.confirm(t('canvas.discardConfirm')) && void run(() => call('node.discard', { ...ids, token: discard.token }))}>
                {t('canvas.discardConfirmButton')}
              </button>
            </div>
          )}
        </section>
      )}
    </aside>
  )
}

/** 舊 Cowork 留下的 worktree：只列出，不提供自動刪除（§4.2） */
function LegacyInventory({ call }: { call: CanvasCall }): JSX.Element {
  const { t } = useTranslation()
  const [list, setList] = useState<LegacyWorktree[] | null>(null)
  return (
    <section className="cv-sec">
      <div className="cv-sec-title">{t('canvas.legacy')}</div>
      <p className="cv-muted">{t('canvas.legacyHint')}</p>
      <button className="cv-btn" onClick={async () => setList((await call('legacy.inventory')) ?? null)}>{t('canvas.legacyCheck')}</button>
      {list && list.length === 0 && <p className="cv-muted">{t('canvas.legacyNone')}</p>}
      {list?.map((w) => (
        <div key={w.path} className="cv-preview">
          <div className="cv-path" title={w.path}>{w.path}</div>
          <div className="cv-muted">
            {w.branch} · {t('canvas.legacyState', { dirty: w.dirty.length, untracked: w.untracked.length, ignored: w.ignored.length })}
            {!w.ownershipProvable && ` · ${t('canvas.legacyUnproven')}`}
          </div>
          {w.error && <div className="cv-error">{w.error}</div>}
        </div>
      ))}
    </section>
  )
}
