// Bots 頁（cowork-agui-implement.md §8.1、§8.2，OpenBot 的 Bots＋Needs you）：最上面是要你處理的對話，下面是可用的
// Bot，「＋ New Bot」接自己的 Agent（本機 CLI＋協議，或 AG-UI 端點）。
import { useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import AgentMark from '@/components/AgentMark'
import CustomAgentSettings from '@/components/CustomAgentSettings'
import type { CliLauncher } from '../../../../preload/index'
import type { ChannelSummary } from '../../../../shared/canvas'
import { sanitizeCustomAgent, type CustomAgentConfig } from '../../../../shared/customAgent'
import { ago, markOf } from './types'

export default function BotsView({
  launchers,
  canChat,
  channels,
  agentName,
  onOpen,
  onStart,
  onSaved,
  extra
}: {
  launchers: CliLauncher[]
  canChat: (agent: string) => boolean
  channels: ChannelSummary[]
  agentName: (id: string) => string
  onOpen: (channelId: string) => void
  /** Opens the home page with this Bot picked */
  onStart: (agent: string) => void
  onSaved: () => void
  /** More "Needs you" rows (routines' findings) */
  extra?: JSX.Element | null
}): JSX.Element {
  const { t } = useTranslation()
  const [draft, setDraft] = useState<CustomAgentConfig | null>(null)
  const [saved, setSaved] = useState(false)
  const waiting = channels.filter((c) => c.pendingApprovals > 0)

  useEffect(() => {
    if (!draft) setSaved(false)
  }, [draft])

  const openNew = async (): Promise<void> => {
    const s = (await window.api.settings.get()) as { customAgent?: unknown }
    setDraft(sanitizeCustomAgent(s.customAgent))
  }
  const save = async (): Promise<void> => {
    if (!draft) return
    await window.api.settings.set({ customAgent: draft } as never)
    setSaved(true)
    onSaved()
  }

  return (
    <main className="cw-main cw-bots-page">
      <div className="cw-home-inner wide">
        <h1>{t('cw.botsTitle')}</h1>
        <section className="cw-sec" aria-label={t('cw.needsYou')}>
          <div className="cw-sec-title">{t('cw.needsYou')}</div>
          {waiting.length === 0 && !extra && <p className="cw-muted">{t('cw.nothingWaiting')}</p>}
          {waiting.map((c) => (
            <button key={c.id} className="cw-conv" onClick={() => onOpen(c.id)}>
              {c.lead ? <AgentMark agent={markOf(c.lead.agent)} size={20} /> : <span className="cw-orb" />}
              <span className="cw-conv-text">
                <span className="cw-conv-meta">{c.lead ? agentName(c.lead.agent) : ''} · {t('cw.waitingCount', { n: c.pendingApprovals })}</span>
                <span className="cw-conv-name">{c.name}</span>
              </span>
              <span className="cw-conv-right">
                <span className="cw-muted">{c.updatedAt ? ago(c.updatedAt) : ''}</span>
                <span className="cw-dot st-waiting" />
              </span>
            </button>
          ))}
          {extra}
        </section>
        <div className="cw-row">
          <h2 className="cw-grow">{t('cw.exploreBots')}</h2>
          <button className="cw-btn dark" onClick={() => void openNew()}>＋ {t('cw.newBot')}</button>
        </div>
        <div className="cw-bots">
          {launchers.map((l) => (
            <div key={l.id} className="cw-bot">
              <AgentMark agent={markOf(l.cli)} size={26} />
              <span className="cw-grow">
                <b>{l.name}</b>
                <span className="cw-muted">{canChat(l.id) ? t('cw.botBoth') : t('cw.botTerminal')}</span>
              </span>
              <button className="cw-btn" onClick={() => onStart(l.id)}>{t('cw.newConversation')}</button>
            </div>
          ))}
        </div>
        {draft && (
          <section className="cw-sec cw-newbot">
            <div className="cw-sec-title">{t('cw.newBotTitle')}</div>
            <p className="cw-muted">{t('cw.newBotHint')}</p>
            <CustomAgentSettings value={draft} onChange={(v) => { setSaved(false); setDraft(v) }} />
            <div className="cw-row">
              <button className="cw-btn dark" onClick={() => void save()}>{t('common.save')}</button>
              <button className="cw-btn" onClick={() => setDraft(null)}>{t('common.close')}</button>
              {saved && <span className="cw-muted" role="status">{t('cw.newBotSaved')}</span>}
            </div>
          </section>
        )}
      </div>
    </main>
  )
}
