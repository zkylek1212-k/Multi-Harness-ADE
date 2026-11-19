import { useEffect, useState } from 'react'
import { useTranslation } from '@/i18n'
import { CUSTOM_PROTOCOLS, CUSTOM_RESOURCE_KINDS, sanitizeCustomAgent, type CustomAgentConfig, type CustomProtocol } from '../../../shared/customAgent'
import { IconFolderOpen } from './Icons'

export default function CustomAgentSettings({ value, onChange }: {
  value?: CustomAgentConfig
  onChange: (value: CustomAgentConfig) => void
}): JSX.Element {
  const { t } = useTranslation()
  const c = value || sanitizeCustomAgent(undefined)
  const [test, setTest] = useState('')
  const [testing, setTesting] = useState(false)
  const update = (patch: Partial<CustomAgentConfig>) => { setTest(''); onChange({ ...c, ...patch }) }
  const http = c.protocol === 'agui-http'
  const [key, setKey] = useState('')
  const [keySet, setKeySet] = useState(false)
  useEffect(() => { if (http) void window.api.settings.customAgentKeySet().then(setKeySet).catch(() => {}) }, [http])
  const saveKey = async (k: string): Promise<void> => {
    await window.api.settings.customAgentKey(k)
    setKey('')
    setKeySet(!!k)
  }
  return (
    <div className="macos-section custom-agent-settings">
      <div className="macos-section-header">{t('customAgent.title')}</div>
      <div className="macos-inset-group">
        <div className="macos-row">
          <div className="macos-row-main">
            <div className="macos-row-info">
              <span className="macos-row-title" id="custom-agent-label">{t('customAgent.title')}</span>
              <span className="macos-row-sub">{t('customAgent.description')}</span>
            </div>
            <label className="apple-toggle">
              <input type="checkbox" aria-labelledby="custom-agent-label" checked={c.enabled} onChange={(e) => update({ enabled: e.target.checked })} />
              <span className="apple-toggle-slider" />
            </label>
          </div>
          {c.enabled && <div className="macos-row-drawer">
            <label className="custom-agent-field">{t('customAgent.name')}
              <input className="macos-input" value={c.name} onChange={(e) => update({ name: e.target.value })} />
            </label>
            <div className="custom-agent-field"><label htmlFor="custom-agent-command">{http ? t('customAgent.endpoint') : t('customAgent.command')}</label>
              {http ? <input id="custom-agent-command" className="macos-input" required value={c.command} placeholder="https://agent.example.com/agui" onChange={(e) => update({ command: e.target.value })} /> :
              <div className="macos-input-bar">
                <input id="custom-agent-command" className="macos-input" required value={c.command} placeholder="my-agent" onChange={(e) => update({ command: e.target.value })} />
                <button type="button" className="macos-btn-secondary" onClick={async () => {
                  const path = await window.api.files.pickExecutable(t('customAgent.command'))
                  if (path) update({ command: path })
                }}><IconFolderOpen size={13} />{t('settings.browse')}</button>
                <button type="button" className="macos-btn-secondary" disabled={!c.command.trim() || testing} onClick={async () => {
                  setTesting(true)
                  try {
                    const r = await window.api.settings.testCliPath(c.command)
                    setTest(r.ok ? r.version || 'OK' : r.error || t('customAgent.testFailed'))
                  } catch (e) { setTest(String(e)) } finally { setTesting(false) }
                }}>{testing ? t('common.testing') : t('settings.test')}</button>
              </div>}
            </div>
            {test && <div role="status" className="macos-row-sub">{test}</div>}
            <label className="custom-agent-field">{t('customAgent.args')}
              <textarea className="macos-input" rows={3} value={c.args.join('\n')} onChange={(e) => update({ args: e.target.value.split('\n') })} />
            </label>
            <label className="custom-agent-field">{t('customAgent.protocol')}
              <select className="macos-input" value={c.protocol} onChange={(e) => update({ protocol: e.target.value as CustomProtocol })}>
                {CUSTOM_PROTOCOLS.map((p) => <option key={p} value={p}>{t(`customAgent.protocol_${p.replace(/-/g, '_')}`)}</option>)}
              </select>
            </label>
            <p className="macos-row-sub">{t('customAgent.protocolHint')}</p>
            {http && <>
              <div className="custom-agent-field"><label htmlFor="custom-agent-key">{t('customAgent.key')}</label>
                <div className="macos-input-bar">
                  <input id="custom-agent-key" className="macos-input" type="password" autoComplete="off" value={key} placeholder={keySet ? t('customAgent.keySaved') : t('customAgent.keyNone')} onChange={(e) => setKey(e.target.value)} />
                  <button type="button" className="macos-btn-secondary" disabled={!key.trim()} onClick={() => void saveKey(key.trim())}>{t('common.save')}</button>
                  <button type="button" className="macos-btn-secondary" disabled={!keySet} onClick={() => void saveKey('')}>{t('customAgent.keyClear')}</button>
                </div>
              </div>
              <label className="custom-agent-field custom-agent-check">
                <input type="checkbox" checked={c.allowHttp} onChange={(e) => update({ allowHttp: e.target.checked })} /> {t('customAgent.allowHttp')}
              </label>
              <p className="macos-row-sub">{t('customAgent.httpHint')}</p>
            </>}
            <p className="macos-row-sub">{t('customAgent.pathsHint')}</p>
            {CUSTOM_RESOURCE_KINDS.map((kind) => <label key={kind} className="custom-agent-field">
              {t(`customAgent.${kind}Path`)}
              <input className="macos-input" value={c.resources[kind]} placeholder={t('customAgent.optionalPath')}
                onChange={(e) => update({ resources: { ...c.resources, [kind]: e.target.value } })} />
            </label>)}
          </div>}
        </div>
      </div>
    </div>
  )
}
