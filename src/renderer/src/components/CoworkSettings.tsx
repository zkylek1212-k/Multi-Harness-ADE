// 設定視窗的 Cowork 分頁（Cowork 2.0）：節點 worktree 的根目錄、各家 CLI 的結構化審批開關。
// 只改 SettingsModal 的 settings 狀態，由底部「儲存」一起寫回。
import { useTranslation } from '@/i18n'
import type { CoworkSettings as Settings } from '../../../preload/index'

const HOOK_AGENTS = [
  { id: 'claude', name: 'Claude Code', sub: 'settings.coworkHookClaudeSub' },
  { id: 'codex', name: 'Codex', sub: 'settings.coworkHookCodexSub' },
  { id: 'antigravity', name: 'Antigravity', sub: 'settings.coworkHookAgySub' }
] as const

export function sanitizeCoworkSettings(v: unknown): Required<Settings> {
  const o = (v && typeof v === 'object' ? v : {}) as Settings
  const sa = o.structuredApproval || {}
  return {
    worktreeRoot: typeof o.worktreeRoot === 'string' ? o.worktreeRoot : '',
    structuredApproval: { claude: sa.claude ?? true, codex: sa.codex ?? false, antigravity: sa.antigravity ?? false }
  }
}

// Permission mode is the one shared CLI setting; under Bypass these switches have nothing to hook
export default function CoworkSettings({ value, onChange, bypass }: { value: unknown; onChange: (v: Settings) => void; bypass: boolean }): JSX.Element {
  const { t } = useTranslation()
  const cw = sanitizeCoworkSettings(value)
  const cloud = /onedrive|dropbox|google ?drive|我的雲端硬碟|icloud/i.test(cw.worktreeRoot)
  return (
    <div className="macos-cowork-settings">
      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkWorktreeSection')}</span>
        <div className="macos-inset-group">
          <div className="macos-row">
            <div className="macos-row-left">
              <span className="macos-row-title">{t('settings.coworkWorktreeRoot')}</span>
              <span className="macos-row-sub">{t('settings.coworkWorktreeRootSub')}</span>
              {cloud && <span className="macos-row-sub" style={{ color: 'var(--warn, var(--fg))' }}>{t('settings.coworkWorktreeCloud')}</span>}
            </div>
            <div className="macos-row-right">
              <input type="text" value={cw.worktreeRoot} placeholder=".cowork" onChange={(e) => onChange({ ...cw, worktreeRoot: e.target.value })} />
            </div>
          </div>
        </div>
      </div>

      <div className="macos-section">
        <span className="macos-section-header">{t('settings.coworkHookSection')}</span>
        {bypass && <p style={{ margin: '0 4px 8px', color: 'var(--warn, var(--fg))', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>{t('settings.coworkHookBypass')}</p>}
        <div className="macos-inset-group">
          {HOOK_AGENTS.map((a) => (
            <div className="macos-row" key={a.id}>
              <div className="macos-row-left">
                <span className="macos-row-title">{a.name}</span>
                <span className="macos-row-sub">{t(a.sub)}</span>
              </div>
              <div className="macos-row-right">
                <label className="apple-toggle">
                  <input
                    type="checkbox"
                    checked={cw.structuredApproval[a.id]}
                    disabled={bypass}
                    onChange={(e) => onChange({ ...cw, structuredApproval: { ...cw.structuredApproval, [a.id]: e.target.checked } })}
                  />
                  <span className="apple-toggle-slider" />
                </label>
              </div>
            </div>
          ))}
        </div>
        <p style={{ margin: '8px 4px 0', color: 'var(--fg-dim)', fontSize: 'var(--text-size-caption)', lineHeight: 1.5 }}>{t('settings.coworkHookNote')}</p>
      </div>
    </div>
  )
}
