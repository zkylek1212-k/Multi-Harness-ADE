import { useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import type { CliLauncher, ProxyConfig, ProxyMode, ProxyStatus } from '../../../preload/index'

const modes: ProxyMode[] = ['inherit', 'custom', 'direct']
const empty: ProxyConfig = { mode: 'inherit', url: '', username: '', bypass: '', agents: {} }

export function ProxyStatusPill({ onClick }: { onClick: () => void }): JSX.Element {
  const { t } = useTranslation()
  const [proxy, setProxy] = useState<ProxyConfig>()
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    const refresh = (): void => { window.api.settings.get().then(s => { setProxy(s.proxy); setFailed(false) }).catch(() => setFailed(true)) }
    refresh()
    return window.api.settings.onChanged(refresh)
  }, [])
  let label = t('common.loading')
  if (failed) label = t('proxy.loadFailed')
  else if (proxy) {
    label = t(`proxy.${proxy.mode}`)
    if (proxy.mode === 'custom') {
      try { label = new URL(proxy.url).host } catch { label = t('proxy.custom') }
    }
  }
  return <button type="button" className="proxy-status-pill" onClick={onClick} title={t('proxy.scope')}>{label}</button>
}

export default function ProxySettings({ value, onChange }: {
  value?: ProxyConfig
  onChange: (value: ProxyConfig) => void
}): JSX.Element {
  const { t } = useTranslation()
  const c = value || empty
  const [launchers, setLaunchers] = useState<CliLauncher[]>([])
  const [status, setStatus] = useState<ProxyStatus>()
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [testing, setTesting] = useState(false)
  const [result, setResult] = useState('')
  const [error, setError] = useState('')
  const refresh = async (): Promise<void> => {
    try {
      const [next, rows] = await Promise.all([window.api.settings.proxyStatus(), window.api.pty.launchers()])
      setStatus(next)
      setLaunchers(rows)
      setError('')
    } catch { setError(t('proxy.loadFailed')) }
  }
  useEffect(() => {
    void refresh()
    return window.api.settings.onChanged(() => { void refresh() })
  }, [])
  const update = (patch: Partial<ProxyConfig>): void => { setResult(''); onChange({ ...c, ...patch }) }
  const address = /^(http|https|socks5):\/\/(\[[^\]]*\]|[^:/]*)(?::([^/]*))?\/?$/.exec(c.url)
  const scheme = address?.[1] || 'http'
  const host = address?.[2] || ''
  const port = address?.[3] ?? (scheme === 'https' ? '443' : scheme === 'http' ? '80' : '')
  const setAddress = (protocol: string, hostname: string, number: string): void => {
    const h = hostname.includes(':') && !hostname.startsWith('[') ? `[${hostname}]` : hostname
    update({ url: `${protocol}://${h}:${number}` })
  }
  const rows = new Map<string, { id: string; name: string; cli: string }>([
    ['claude', { id: 'claude', name: 'Claude Code', cli: 'claude' }],
    ['codex', { id: 'codex', name: 'Codex', cli: 'codex' }],
    ['antigravity', { id: 'antigravity', name: 'Antigravity', cli: 'antigravity' }],
    ['custom-agent', { id: 'custom-agent', name: t('customAgent.title'), cli: 'custom-agent' }]
  ])
  launchers.forEach(l => rows.set(l.id, l))
  Object.keys(c.agents).forEach(id => { if (!rows.has(id)) rows.set(id, { id, name: id, cli: id }) })
  const writePassword = async (next: string): Promise<void> => {
    setBusy(true)
    setError('')
    try {
      await window.api.settings.proxyPassword(next)
      setPassword('')
      await refresh()
    } catch { setError(t('proxy.secretFailed')) } finally { setBusy(false) }
  }
  return <div className="macos-settings-body proxy-settings">
    <p className="macos-row-sub">{t('proxy.scope')}</p>
    <fieldset className="proxy-modes">
      <legend>{t('proxy.mode')}</legend>
      {modes.map(mode => <label key={mode}><input type="radio" name="proxy-mode" checked={c.mode === mode} onChange={() => update({ mode })} />{t(`proxy.${mode}`)}</label>)}
    </fieldset>
    <p className="macos-row-sub">{t('proxy.directHint')}</p>
    <div className="proxy-address">
      <label className="custom-agent-field">{t('proxy.scheme')}<select className="macos-input" value={scheme} onChange={e => setAddress(e.target.value, host, port)}>
        {['http', 'https', 'socks5'].map(s => <option key={s}>{s}</option>)}
      </select></label>
      <label className="custom-agent-field">{t('proxy.host')}<input className="macos-input" value={host} onChange={e => setAddress(scheme, e.target.value, port)} /></label>
      <label className="custom-agent-field">{t('proxy.port')}<input className="macos-input" type="number" min="1" max="65535" value={port} onChange={e => setAddress(scheme, host, e.target.value)} /></label>
    </div>
    <label className="custom-agent-field">{t('proxy.username')}<input className="macos-input" autoComplete="off" value={c.username} onChange={e => update({ username: e.target.value })} /></label>
    <label className="custom-agent-field">{t('proxy.password')}<input className="macos-input" type="password" autoComplete="new-password" value={password} onChange={e => setPassword(e.target.value)} /></label>
    <div className="proxy-actions">
      <span role="status">{status ? t(status.passwordSaved ? 'proxy.secretSaved' : 'proxy.secretEmpty') : t('common.loading')}</span>
      <button type="button" className="macos-btn-secondary" disabled={busy || !password} onClick={() => { void writePassword(password) }}>{t('proxy.savePassword')}</button>
      <button type="button" className="macos-btn-secondary" disabled={busy || !status?.passwordSaved} onClick={() => { void writePassword('') }}>{t('proxy.clearPassword')}</button>
    </div>
    <p className="macos-row-sub">{t('proxy.secretHint')}</p>
    <label className="custom-agent-field">{t('proxy.bypass')}<input className="macos-input" value={c.bypass} onChange={e => update({ bypass: e.target.value })} /></label>
    <p className="macos-row-sub">{t('proxy.loopback')}</p>
    <button type="button" className="macos-btn-secondary" disabled={testing || !host || !port} onClick={async () => {
      setTesting(true)
      setResult('')
      try { const r = await window.api.settings.testProxy(c.url); setResult(t(r.ok ? 'proxy.reachable' : 'proxy.unreachable')) }
      catch { setResult(t('proxy.unreachable')) } finally { setTesting(false) }
    }}>{testing ? t('common.testing') : t('proxy.test')}</button>
    <p className="macos-row-sub">{t('proxy.testHint')}</p>
    {result && <p role="status">{result}</p>}
    <h3 className="macos-section-header">{t('proxy.agents')}</h3>
    {[...rows.values()].map(l => <div key={l.id} className="proxy-agent">
      <label className="custom-agent-field">{l.name}<select className="macos-input" value={c.agents[l.id] || ''} onChange={e => {
        const agents = { ...c.agents }
        if (e.target.value) agents[l.id] = e.target.value as ProxyMode
        else delete agents[l.id]
        update({ agents })
      }}><option value="">{t('proxy.global')}</option>{modes.map(mode => <option key={mode} value={mode}>{t(`proxy.${mode}`)}</option>)}</select></label>
      {l.cli === 'claude' && scheme === 'socks5' && (c.agents[l.id] || c.mode) === 'custom' && <p className="proxy-warning">{t('proxy.socksWarning')}</p>}
    </div>)}
    <div className="proxy-actions"><h3 className="macos-section-header">{t('proxy.sessions')}</h3><button type="button" className="macos-btn-secondary" onClick={() => { void refresh() }}>{t('common.refresh')}</button></div>
    {status?.sessions.map(s => <p key={s.id} className="macos-row-sub">{s.agent} · {s.id.slice(0, 8)} · {s.started?.mode === 'custom' ? s.started.url : t(`proxy.${s.started?.mode || 'inherit'}`)}{s.restartNeeded && <span className="proxy-warning"> · {t('proxy.restart')}</span>}</p>)}
    {error && <p role="alert" className="proxy-warning">{error}</p>}
  </div>
}
