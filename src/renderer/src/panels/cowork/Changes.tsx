// Workspace 面板的「改動」分頁（cowork-agui-implement.md §8.4）：這段對話的 worktree、改動、提交全部、合併、清理與捨棄。
// 每個會動資料的操作都先預覽、明確按下才做；後端再核對一次（token 綁定來源與目標），畫面這裡不自己判斷安全。
import { useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import type { ChangedFile, ChangesSummary, ChannelView, DiscardPreview, MergePreview } from '../../../../shared/canvas'
import type { CanvasCall } from './types'

function Files({ title, files }: { title: string; files: ChangedFile[] }): JSX.Element | null {
  const [open, setOpen] = useState<string | null>(null)
  if (files.length === 0) return null
  return (
    <div className="cw-files">
      <div className="cw-files-title">{title}</div>
      {files.map((f) => (
        <div key={f.path}>
          <button className="cw-file" onClick={() => setOpen(open === f.path ? null : f.path)}>
            <span className="cw-file-status">{f.status}</span>
            <span className="cw-file-path">{f.path}</span>
            {(f.binary || f.truncated) && <span className="cw-muted">{f.binary ? 'binary' : '…'}</span>}
          </button>
          {open === f.path && f.text && <pre className="cw-diff">{f.text}</pre>}
        </div>
      ))}
    </div>
  )
}

export default function Changes({ view, nodeId, call }: { view: ChannelView; nodeId: string; call: CanvasCall }): JSX.Element {
  const { t } = useTranslation()
  const n = view.channel.nodes.find((x) => x.id === nodeId)!
  const rt = view.runtime[nodeId]
  const ended = !rt || !['running', 'creating', 'closing'].includes(rt.status)
  const ids = { channelId: view.channel.id, nodeId }
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
    // 節點狀態一變（例如剛結束）、對話的一輪做完、或終端的 CLI 從忙碌變閒置，就重讀改動
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodeId, rt?.status, rt?.inRun, rt?.busy])

  const run = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    await fn()
    setBusy(false)
  }
  const uncommitted = changes ? [...new Set([...changes.staged, ...changes.unstaged, ...changes.untracked].map((f) => f.path))] : []

  return (
    <div className="cw-changes">
      <dl className="cw-dl">
        <dt>{t('canvas.status')}</dt>
        <dd>{t(`canvas.status_${(rt?.status || 'exited').replace('-', '_')}`)}{rt?.error ? ` — ${rt.error}` : ''}</dd>
        <dt>{t('canvas.branch')}</dt>
        <dd>{n.worktree ? n.worktree.branch : t('canvas.repoRoot')}</dd>
        {n.worktree && (
          <>
            <dt>{t('canvas.worktree')}</dt>
            <dd className="cw-path" title={n.worktree.path}>{n.worktree.path}</dd>
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


      {n.worktree && (
        <section className="cw-sec">
          <div className="cw-sec-title">
            {t('canvas.changes')}
            <button className="cw-icon-btn" onClick={() => void refresh()} title={t('canvas.refresh')}>↻</button>
          </div>
          {changes ? (
            <>
              {changes.commits.length > 0 && (
                <div className="cw-files">
                  <div className="cw-files-title">{t('canvas.commits')}</div>
                  {changes.commits.map((c) => (
                    <div key={c} className="cw-commit">{c.slice(0, 10)} {c.slice(41)}</div>
                  ))}
                </div>
              )}
              <Files title={t('canvas.committed')} files={changes.committed} />
              <Files title={t('canvas.staged')} files={changes.staged} />
              <Files title={t('canvas.unstaged')} files={changes.unstaged} />
              <Files title={t('canvas.untracked')} files={changes.untracked} />
              {changes.commits.length === 0 && uncommitted.length === 0 && <p className="cw-muted">{t('canvas.noChanges')}</p>}
            </>
          ) : (
            <p className="cw-muted">…</p>
          )}
        </section>
      )}

      {n.worktree && uncommitted.length > 0 && (
        <section className="cw-sec">
          <div className="cw-sec-title">{t('canvas.commitAll')}</div>
          <p className="cw-muted">{t('canvas.commitAllHint', { n: uncommitted.length })}</p>
          <input className="cw-input" value={commitMsg} placeholder={t('canvas.commitMessage')} onChange={(e) => setCommitMsg(e.target.value)} />
          <button
            className="cw-btn primary"
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
        <section className="cw-sec">
          <div className="cw-sec-title">{t('canvas.merge')}</div>
          <div className="cw-row">
            <select className="cw-input" value={target} onChange={(e) => { setTarget(e.target.value); setMerge(null) }}>
              {(branches?.all || [target]).filter((b) => b && !b.startsWith('cowork/')).map((b) => (
                <option key={b} value={b}>{b}{b === branches?.current ? ` (${t('canvas.current')})` : ''}</option>
              ))}
            </select>
            <button className="cw-btn" disabled={busy || !changes?.clean} title={changes?.clean ? '' : t('canvas.mergeNeedsClean')} onClick={() => void run(async () => setMerge((await call('node.mergePreview', { ...ids, target })) ?? null))}>
              {t('canvas.preview')}
            </button>
          </div>
          {merge && (
            <div className="cw-preview">
              <p>{t('canvas.mergeInto', { source: merge.source.branch, target: merge.target.branch })}</p>
              <ul>{merge.commits.map((c) => <li key={c}>{c.slice(0, 10)} {c.slice(41)}</li>)}</ul>
              <ul>{merge.files.map((f) => <li key={f.path}>{f.status} {f.path}</li>)}</ul>
              <button
                className="cw-btn primary"
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
        <section className="cw-sec">
          <div className="cw-sec-title">{t('canvas.finish')}</div>
          <p className="cw-muted">{n.worktree ? t('canvas.cleanupHint') : t('canvas.removeRootHint')}</p>
          <div className="cw-row">
            <button className="cw-btn" disabled={busy} onClick={() => void run(() => call('node.cleanup', ids))}>
              {n.worktree ? t('canvas.cleanup') : t('canvas.removeNode')}
            </button>
            {n.worktree && (
              <button className="cw-btn danger" disabled={busy} onClick={() => void run(async () => setDiscard((await call('node.discardPreview', ids)) ?? null))}>
                {t('canvas.discard')}
              </button>
            )}
          </div>
          {discard && (
            <div className="cw-preview danger">
              <p>{t('canvas.discardWarn')}</p>
              {discard.unmergedCommits.length > 0 && <ul>{discard.unmergedCommits.map((c) => <li key={c}>{c.slice(0, 10)} {c.slice(41)}</li>)}</ul>}
              <ul>
                {[...discard.changes.staged, ...discard.changes.unstaged, ...discard.changes.untracked].map((f) => (
                  <li key={`${f.status}${f.path}`}>{f.status} {f.path}</li>
                ))}
                {discard.ignored.map((p) => <li key={`i${p}`}>{t('canvas.ignored')} {p}</li>)}
              </ul>
              <button className="cw-btn danger" disabled={busy} onClick={() => window.confirm(t('canvas.discardConfirm')) && void run(() => call('node.discard', { ...ids, token: discard.token }))}>
                {t('canvas.discardConfirmButton')}
              </button>
            </div>
          )}
        </section>
      )}
    </div>
  )
}
