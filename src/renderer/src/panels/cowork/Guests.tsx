// 邀請同事進這個 channel（cowork-agui-implement.md §13）：同事在公司內網用瀏覽器開，可以對話、核准、停止這一輪、看改動。
// 風險寫在最上面；連結一次性、7 天到期；每位同事隨時可撤銷，撤銷後立即斷線。
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import type { GuestInfo, GuestInvite } from '../../../../shared/canvas'
import { ago, type CanvasCall } from './types'

export default function Guests({ call, channelId, onClose }: { call: CanvasCall; channelId: string; onClose: () => void }): JSX.Element {
  const { t } = useTranslation()
  const [list, setList] = useState<GuestInfo[]>([])
  const [invite, setInvite] = useState<GuestInvite | null>(null)
  const [copied, setCopied] = useState(false)
  const load = useCallback(() => void call('guest.list', { channelId }).then((l) => l && setList(l)), [call, channelId])
  useEffect(() => {
    load()
    const id = window.setInterval(load, 10_000)
    return () => window.clearInterval(id)
  }, [load])

  return (
    <section className="cw-card cw-guests" aria-label={t('cw.invite')}>
      <div className="cw-row">
        <b>{t('cw.inviteTitle')}</b>
        <div className="cw-grow" />
        <button className="cw-icon" aria-label={t('common.close')} onClick={onClose}>✕</button>
      </div>
      <ul className="cw-risk">
        <li>{t('cw.risk1')}</li>
        <li>{t('cw.risk2')}</li>
        <li>{t('cw.risk3')}</li>
        <li>{t('cw.risk4')}</li>
      </ul>
      {invite ? (
        <div className="cw-col">
          <div className="cw-row">
            <input className="cw-input cw-grow cw-mono" readOnly value={invite.url} aria-label={t('cw.inviteLink')} onFocus={(e) => e.currentTarget.select()} />
            <button className="cw-btn dark" onClick={() => void navigator.clipboard.writeText(invite.url).then(() => setCopied(true))}>{copied ? t('cw.copied') : t('cw.copy')}</button>
          </div>
          <span className="cw-muted">{t('cw.inviteOnce', { date: new Date(invite.expiresAt).toLocaleString() })}</span>
          {invite.caUrl && <span className="cw-muted">{t('cw.inviteCert', { url: invite.caUrl })}</span>}
        </div>
      ) : (
        <button className="cw-btn dark" onClick={() => void call('guest.invite', { channelId }).then((r) => r && (setInvite(r), setCopied(false)))}>{t('cw.inviteCreate')}</button>
      )}
      <div className="cw-muted">{t('cw.guests', { n: list.length })}</div>
      {list.map((g) => (
        <div key={g.id} className="cw-row">
          <span className={`cw-dot st-${g.online ? 'running' : 'exited'}`} />
          <b>{g.name}</b>
          <span className="cw-muted">{t('cw.joined', { ago: ago(g.createdAt) })} · {g.online ? t('cw.online') : t('cw.lastSeen', { ago: ago(g.lastSeenAt) })}</span>
          <div className="cw-grow" />
          <button className="cw-btn danger" onClick={() => window.confirm(t('cw.revokeConfirm', { name: g.name })) && void call('guest.revoke', { channelId, guestId: g.id }).then(load)}>{t('cw.revoke')}</button>
        </div>
      ))}
    </section>
  )
}
