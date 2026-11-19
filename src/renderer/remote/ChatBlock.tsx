import { useEffect, useState } from 'react'
import type { ChannelView } from '../../shared/canvas'
import type { TranscriptEntry } from '../../shared/agui'
import type { RemoteConnection } from './conn'
import { t } from './i18n'

// 手機上的一個對話 Bot（cowork-agui-implement.md §7、P4）：最近的對話、待審批的工具、ask_person 的提問、送訊息。
// 後端只給最近 40 則，工具的參數與輸出不會到手機，長文字會截短。Agent 的字一律當純文字。
type Run = (fn: () => Promise<{ ok: boolean; code?: string; message?: string }>) => Promise<boolean>

export default function ChatBlock({ conn, windowId, view, nodeId, run, busy }: { conn: RemoteConnection; windowId: number; view: ChannelView; nodeId: string; run: Run; busy: boolean }): JSX.Element {
  const [entries, setEntries] = useState<TranscriptEntry[]>([])
  const [text, setText] = useState('')
  const [answer, setAnswer] = useState('')
  const channelId = view.channel.id
  const rt = view.runtime[nodeId]
  const asks = (view.asks ?? []).filter((a) => a.node === nodeId)
  const stopped = view.stop.requested

  // The view changes on every turn's start and end and on each approval: read the conversation again then
  useEffect(() => {
    void conn.canvas(windowId, 'chat.transcript', { channelId, nodeId }).then((r) => r.ok && setEntries(r.data.entries))
  }, [conn, windowId, channelId, nodeId, view])

  const shown = entries.filter((e) => !(e.kind === 'text' && e.text.trim() === 'PASS')).slice(-20)
  return (
    <div className="stack">
      {shown.map((e) => {
        switch (e.kind) {
          case 'user':
            return <p key={`u${e.id}`} className="chat-user">{e.from !== 'user' && <span className="footnote">{e.from} · </span>}{e.text}</p>
          case 'text':
            return <p key={`t${e.id}`} className="chat-text">{e.text}</p>
          case 'tool':
            return <p key={`x${e.id}`} className="footnote">▸ {e.name}</p>
          case 'runError':
            return <p key={`e${e.id}`} className="footnote">{e.interrupted ? t('cv_chatStopped') : e.message}</p>
          case 'approval':
            return e.status === 'pending' ? (
              <div key={`a${e.id}`} className="cw-card">
                <p className="card-title">{t('cv_chatApproval', { tool: e.tool })}</p>
                <pre className="cw-pre">{e.summary}</pre>
                <div className="cw-actions">
                  <button className="btn" disabled={busy} onClick={() => run(() => conn.canvas(windowId, 'chat.answer', { channelId, nodeId, approvalId: e.id, allow: false }))}>{t('cv_reject')}</button>
                  <button className="btn prominent" disabled={busy || stopped} onClick={() => run(() => conn.canvas(windowId, 'chat.answer', { channelId, nodeId, approvalId: e.id, allow: true }))}>{t('cv_approve')}</button>
                </div>
              </div>
            ) : (
              <p key={`a${e.id}`} className="footnote">{e.tool}: {e.status}</p>
            )
          default:
            return null
        }
      })}
      {rt?.inRun && <p className="footnote">{t('cv_chatWorking')}</p>}
      {asks.map((a) => (
        <div key={a.id} className="cw-card">
          <p className="card-title">{t('cv_chatAsks')}</p>
          <p className="chat-text">{a.question}</p>
          <div className="cw-actions">
            {a.choices.map((c) => (
              <button key={c} className="btn" disabled={busy} onClick={() => run(() => conn.canvas(windowId, 'ask.answer', { channelId, askId: a.id, text: c }))}>{c}</button>
            ))}
          </div>
          <div className="field">
            <input className="cw-input" value={answer} maxLength={16000} placeholder={t('cv_chatAnswer')} onChange={(e) => setAnswer(e.target.value)} />
            <button className="btn prominent" disabled={busy || !answer.trim()} onClick={async () => (await run(() => conn.canvas(windowId, 'ask.answer', { channelId, askId: a.id, text: answer.trim() }))) && setAnswer('')}>{t('cv_chatSend')}</button>
          </div>
        </div>
      ))}
      {rt?.status === 'running' && (
        <div className="field">
          <textarea className="cw-input" rows={2} maxLength={16000} value={text} placeholder={t('cv_chatPlaceholder')} onChange={(e) => setText(e.target.value)} />
          <div className="cw-actions">
            <button className="btn prominent" disabled={busy || stopped || !text.trim()} onClick={async () => (await run(() => conn.canvas(windowId, 'chat.send', { channelId, to: nodeId, text: text.trim() }))) && setText('')}>{t('cv_chatSend')}</button>
          </div>
          {rt.inRun && <p className="footnote">{t('cv_chatQueued')}</p>}
        </div>
      )}
    </div>
  )
}
