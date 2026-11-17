import { useState, useEffect } from 'react'
import { useWorkbench, setTheme, bumpSettings, setUiMode, pasteInNewShell } from '@/store'
import { useTranslation } from '@/i18n'
import CustomizedPanel from '@/panels/customized/CustomizedPanel'
import RemoteSettings from './RemoteSettings'
import CoworkSettings from './CoworkSettings'
import MemorySyncSettings from './MemorySyncSettings'
import MemoryPluginPrompt from './MemoryPluginPrompt'
import CustomAgentSettings from './CustomAgentSettings'
import ProxySettings from './ProxySettings'
import {
  IconSun,
  IconMoon,
  IconPalette,
  IconTerminalBox,
  IconPuzzle,
  IconChevronRight,
  IconClose,
  IconFileText,
  IconFolderOpen,
  IconExternalLink,
  IconCheck,
  IconShield,
  IconInfo,
  IconDownload,
  IconSpark,
  IconRefresh,
  IconAppLogo,
  IconPhone,
  IconCowork
} from './Icons'
import './settingsModal.css'
import type { WorkbenchSettings, DocToolPaths, UpdaterStatus, AgentInstallInfo, CliTestRecord } from '../../../preload/index'
import type { SettingsTab } from '@/store'

interface SettingsModalProps {
  isOpen: boolean
  onClose: () => void
  initialTab?: SettingsTab | 'proxy'
}

interface TestResult {
  testing: boolean
  ok?: boolean
  version?: string
  error?: string
}

function formatReleaseNotes(notes: unknown): string {
  if (!notes) return ''
  let text = ''
  if (Array.isArray(notes)) {
    text = notes.map((n) => (typeof n === 'string' ? n : (n as { note?: string })?.note || '')).join('\n')
  } else if (typeof notes === 'string') {
    text = notes
  } else {
    return ''
  }
  return text
    .replace(/<br\s*[\/]?>/gi, '\n')
    .replace(/<\/p>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .trim()
}

interface CliConfigItem {
  id: string
  name: string
  category: 'agent' | 'shell'
  desc: string
  defaultCmd: string
  badgeColor: string
  shortBadge: string
}

const cliConfigs: CliConfigItem[] = [
  {
    id: 'claude',
    name: 'Claude Code',
    category: 'agent',
    desc: 'Anthropic Claude Code interactive coding CLI',
    defaultCmd: 'claude (or claude.ps1 / claude.cmd)',
    badgeColor: '#E05D26',
    shortBadge: '@c'
  },
  {
    id: 'antigravity',
    name: 'Antigravity (AGY)',
    category: 'agent',
    desc: 'Google DeepMind Antigravity autonomous CLI (agy.exe)',
    defaultCmd: 'agy',
    badgeColor: '#6366F1',
    shortBadge: '@a'
  },
  {
    id: 'codex',
    name: 'Codex CLI',
    category: 'agent',
    desc: 'OpenAI Codex CLI official agent runner (codex.exe)',
    defaultCmd: 'codex',
    badgeColor: '#10B981',
    shortBadge: '@o'
  },
  {
    id: 'powershell',
    name: 'PowerShell',
    category: 'shell',
    desc: 'PowerShell 7 (pwsh) or Windows PowerShell environment',
    defaultCmd: 'powershell.exe / pwsh',
    badgeColor: '#0284C7',
    shortBadge: 'PS'
  },
  {
    id: 'cmd',
    name: 'Command Prompt',
    category: 'shell',
    desc: 'Windows Command Prompt interpreter (cmd.exe)',
    defaultCmd: 'cmd.exe',
    badgeColor: '#475569',
    shortBadge: 'CMD'
  }
]

interface DocToolConfigItem {
  id: 'word' | 'excel' | 'powerpoint' | 'pdf'
  name: string
  exts: string
  desc: string
  badgeColor: string
  iconLetter: string
  defaultAppLabel: string
}

const docToolConfigs: DocToolConfigItem[] = [
  {
    id: 'word',
    name: 'Microsoft Word',
    exts: '.docx, .doc',
    desc: 'Word processor for documents and reports',
    badgeColor: '#185ABD',
    iconLetter: 'W',
    defaultAppLabel: 'System Default (Microsoft Word)'
  },
  {
    id: 'excel',
    name: 'Microsoft Excel',
    exts: '.xlsx, .xls, .csv',
    desc: 'Spreadsheet viewer and data analysis editor',
    badgeColor: '#107C41',
    iconLetter: 'X',
    defaultAppLabel: 'System Default (Microsoft Excel)'
  },
  {
    id: 'powerpoint',
    name: 'Microsoft PowerPoint',
    exts: '.pptx, .ppt',
    desc: 'Presentation deck and visual slides viewer',
    badgeColor: '#D83B01',
    iconLetter: 'P',
    defaultAppLabel: 'System Default (Microsoft PowerPoint)'
  },
  {
    id: 'pdf',
    name: 'PDF Viewer',
    exts: '.pdf',
    desc: 'Portable document reader and viewer',
    badgeColor: '#E5252A',
    iconLetter: 'PDF',
    defaultAppLabel: 'System Default (Edge / Acrobat / Preview)'
  }
]

export default function SettingsModal({
  isOpen,
  onClose,
  initialTab
}: SettingsModalProps): JSX.Element | null {
  const { theme, uiMode } = useWorkbench()
  const { t, language, setLanguage } = useTranslation()
  const [tab, setTab] = useState<SettingsTab | 'proxy'>(initialTab || 'appearance')
  const [settings, setSettings] = useState<WorkbenchSettings>({
    cliPaths: { claude: '', antigravity: '', codex: '', powershell: '', cmd: '' },
    cliEnabled: { claude: true, antigravity: true, codex: true, powershell: true, cmd: true },
    cliBypassPermissions: false,
    docToolPaths: { word: '', excel: '', powerpoint: '', pdf: '' },
    autoCheckUpdates: true
  })
  const [detectedPaths, setDetectedPaths] = useState<Record<string, string>>({
    claude: '',
    antigravity: '',
    codex: '',
    powershell: '',
    cmd: ''
  })
  const [testResults, setTestResults] = useState<Record<string, TestResult>>({
    claude: { testing: false },
    antigravity: { testing: false },
    codex: { testing: false },
    powershell: { testing: false },
    cmd: { testing: false }
  })
  const [detectedDocTools, setDetectedDocTools] = useState<DocToolPaths>({
    word: '',
    excel: '',
    powerpoint: '',
    pdf: ''
  })
  const [docToolTestResults, setDocToolTestResults] = useState<Record<string, TestResult>>({
    word: { testing: false },
    excel: { testing: false },
    powerpoint: { testing: false },
    pdf: { testing: false }
  })
  const [expandedCli, setExpandedCli] = useState<Record<string, boolean>>({})
  const [detectingCli, setDetectingCli] = useState<Record<string, boolean>>({})
  const [detectFeedback, setDetectFeedback] = useState<Record<string, { type: 'ok' | 'fail'; msg: string }>>({})
  const [installConfirmAgent, setInstallConfirmAgent] = useState<AgentInstallInfo | null>(null)
  const [installingAgent, setInstallingAgent] = useState<Record<string, boolean>>({})
  const [saving, setSaving] = useState(false)
  const [saveSuccess, setSaveSuccess] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [updaterStatus, setUpdaterStatus] = useState<UpdaterStatus | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)

  useEffect(() => {
    if (initialTab) {
      setTab(initialTab)
    }
  }, [initialTab])

  useEffect(() => {
    if (!isOpen) return

    // 載入當前設定
    window.api.settings.get().then((s) => {
      setSettings(s)
      if (s.cliTestResults) {
        const restored: Record<string, TestResult> = {}
        for (const [id, r] of Object.entries(s.cliTestResults)) {
          if (r && r.ok) {
            restored[id] = {
              testing: false,
              ok: r.ok,
              version: r.version,
              error: r.error
            }
          }
        }
        setTestResults((prev) => ({ ...prev, ...restored }))
      }
      if (s.docToolTestResults) {
        const restoredDoc: Record<string, TestResult> = {}
        for (const [id, r] of Object.entries(s.docToolTestResults)) {
          if (r && r.ok) {
            restoredDoc[id] = {
              testing: false,
              ok: r.ok,
              version: r.version,
              error: r.error
            }
          }
        }
        setDocToolTestResults((prev) => ({ ...prev, ...restoredDoc }))
      }
    })

    // 取得當前更新狀態並訂閱即時廣播
    window.api.updater.getStatus().then((st) => {
      setUpdaterStatus(st)
    })
    const unsubUpdater = window.api.updater.onStatusChange((st) => {
      setUpdaterStatus(st)
    })

    // 偵測已安裝之 Office / PDF 工具
    window.api.files.detectDocTools().then((detected) => {
      setDetectedDocTools(detected)
    })

    // 取得當前系統自動偵測到的路徑作為參考
    window.api.ext.agents().then((agents) => {
      const map: Record<string, string> = {
        claude: '',
        antigravity: '',
        codex: '',
        powershell: '',
        cmd: ''
      }
      for (const a of agents) {
        if (a.cliPath) map[a.agent] = a.cliPath
      }
      if (typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')) {
        map.powershell = 'powershell.exe'
        map.cmd = 'cmd.exe'
      }
      setDetectedPaths(map)
    })

    return () => {
      unsubUpdater()
    }
  }, [isOpen])

  // 按 Esc 鍵關閉視窗
  useEffect(() => {
    if (!isOpen) return
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, onClose])

  if (!isOpen) return null

  const handlePathChange = (id: string, val: string): void => {
    setSettings((prev: WorkbenchSettings) => {
      const nextCliTestResults = { ...(prev.cliTestResults || {}) }
      delete nextCliTestResults[id]
      return {
        ...prev,
        cliPaths: {
          ...prev.cliPaths,
          [id]: val
        },
        cliTestResults: nextCliTestResults
      }
    })
    setTestResults((prev: Record<string, TestResult>) => ({
      ...prev,
      [id]: { testing: false }
    }))
    setDetectFeedback((prev) => {
      const next = { ...prev }
      delete next[id]
      return next
    })
  }

  const handleToggleCli = (id: string, enabled: boolean): void => {
    setSettings((prev: WorkbenchSettings) => ({
      ...prev,
      cliEnabled: {
        ...prev.cliEnabled,
        [id]: enabled
      }
    }))
  }

  const toggleExpand = (id: string): void => {
    setExpandedCli((prev) => ({ ...prev, [id]: !prev[id] }))
  }

  const handleBrowseCli = async (id: string, name: string): Promise<void> => {
    const picked = await window.api.files.pickExecutable(`Select ${name} Executable`)
    if (picked) {
      handlePathChange(id, picked)
    }
  }

  const handleDetectCli = async (id: string): Promise<void> => {
    setDetectingCli((prev) => ({ ...prev, [id]: true }))
    setDetectFeedback((prev) => {
      const next = { ...prev }
      delete next[id]
      return next
    })

    try {
      const agents = await window.api.ext.agents()
      const map: Record<string, string> = {
        claude: '',
        antigravity: '',
        codex: '',
        powershell: '',
        cmd: ''
      }
      for (const a of agents) {
        if (a.cliPath) map[a.agent] = a.cliPath
      }
      if (typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')) {
        map.powershell = 'powershell.exe'
        map.cmd = 'cmd.exe'
      }
      setDetectedPaths(map)

      const found = map[id]
      if (found) {
        handlePathChange(id, found)
        setDetectFeedback((prev) => ({
          ...prev,
          [id]: { type: 'ok', msg: `${t('settings.detected')}: ${found}` }
        }))

        // 自動執行快速驗證並持久化記憶 pass 狀態，使用者下次開啟直接顯示 pass
        window.api.settings.testCliPath(found).then((testRes) => {
          if (testRes.ok) {
            setTestResults((prev) => ({
              ...prev,
              [id]: {
                testing: false,
                ok: true,
                version: testRes.version
              }
            }))
            setSettings((prev) => {
              const nextSettings: WorkbenchSettings = {
                ...prev,
                cliTestResults: {
                  ...(prev.cliTestResults || {}),
                  [id]: {
                    ok: true,
                    version: testRes.version,
                    testedPath: testRes.resolvedPath || found,
                    testedAt: Date.now()
                  }
                }
              }
              window.api.settings.set(nextSettings).catch(() => {})
              return nextSettings
            })
          }
        })
      } else {
        setDetectFeedback((prev) => ({
          ...prev,
          [id]: { type: 'fail', msg: t('settings.cliNotDetectedNotice') }
        }))
      }
    } catch {
      setDetectFeedback((prev) => ({
        ...prev,
        [id]: { type: 'fail', msg: t('settings.cliNotDetectedNotice') }
      }))
    } finally {
      setDetectingCli((prev) => ({ ...prev, [id]: false }))
    }
  }

  const handleRequestInstall = async (id: string): Promise<void> => {
    if (id !== 'claude' && id !== 'antigravity' && id !== 'codex') return
    try {
      const info = await window.api.ext.getAgentInstallInfo(id)
      setInstallConfirmAgent(info)
    } catch (e) {
      console.error('Failed to get install info:', e)
    }
  }

  // 安裝指令貼進一個新的、看得見的終端，由使用者自己按 Enter 執行。
  // 不再由 App 在背景啟動隱藏的 PowerShell：那種「下載後直接執行」的行為會被防毒軟體判成惡意程式，使用者也看不到實際在跑什麼。
  const handleConfirmInstall = (): void => {
    if (!installConfirmAgent) return
    const { id, command, shell } = installConfirmAgent
    setInstallConfirmAgent(null)
    pasteInNewShell(command, shell)
    setDetectFeedback((prev) => ({ ...prev, [id]: { type: 'ok', msg: t('settings.installPasted') } }))
    onClose()
  }

  const handleDetectAll = async (): Promise<void> => {
    try {
      const agents = await window.api.ext.agents()
      const map: Record<string, string> = {
        claude: '',
        antigravity: '',
        codex: '',
        powershell: '',
        cmd: ''
      }
      for (const a of agents) {
        if (a.cliPath) map[a.agent] = a.cliPath
      }
      if (typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')) {
        map.powershell = 'powershell.exe'
        map.cmd = 'cmd.exe'
      }
      setDetectedPaths(map)

      setSettings((prev) => {
        const nextPaths = { ...prev.cliPaths }
        for (const cfg of cliConfigs) {
          if (map[cfg.id]) {
            nextPaths[cfg.id] = map[cfg.id]
          }
        }
        return { ...prev, cliPaths: nextPaths }
      })
    } catch (e) {
      console.error('Failed to auto-detect all CLIs:', e)
    }
  }

  const handleResetToAuto = (): void => {
    setSettings((prev) => {
      const nextPaths = { ...prev.cliPaths }
      for (const cfg of cliConfigs) {
        nextPaths[cfg.id] = ''
      }
      return { ...prev, cliPaths: nextPaths }
    })
  }

  const handleTestPath = async (id: string): Promise<void> => {
    const custom = settings.cliPaths?.[id]?.trim()
    const detected = detectedPaths[id]
    let target = custom || detected
    if (!target) {
      if (id === 'antigravity') target = 'agy'
      else if (id === 'powershell') target = 'powershell.exe'
      else if (id === 'cmd') target = 'cmd.exe'
      else target = id
    }

    setTestResults((prev) => ({
      ...prev,
      [id]: { testing: true }
    }))

    const res = await window.api.settings.testCliPath(target)
    const actualPath = res.resolvedPath || target

    if (res.ok && res.resolvedPath) {
      setDetectedPaths((prev) => ({ ...prev, [id]: res.resolvedPath! }))
    }

    setTestResults((prev) => ({
      ...prev,
      [id]: {
        testing: false,
        ok: res.ok,
        version: res.version,
        error: res.error
      }
    }))

    if (res.ok) {
      setSettings((prev) => {
        const nextSettings: WorkbenchSettings = {
          ...prev,
          cliTestResults: {
            ...(prev.cliTestResults || {}),
            [id]: {
              ok: true,
              version: res.version,
              testedPath: actualPath,
              testedAt: Date.now()
            }
          }
        }
        window.api.settings.set(nextSettings).catch((err) => {
          console.warn('[Settings] Failed to persist test result:', err)
        })
        return nextSettings
      })
    }
  }

  // ── Document Tools 處理函式 ───────────────────────────────────────
  const handleDocToolPathChange = (
    id: 'word' | 'excel' | 'powerpoint' | 'pdf',
    val: string
  ): void => {
    setSettings((prev) => {
      const nextDocResults = { ...(prev.docToolTestResults || {}) }
      delete nextDocResults[id]
      return {
        ...prev,
        docToolPaths: {
          ...(prev.docToolPaths || {}),
          [id]: val
        },
        docToolTestResults: nextDocResults
      }
    })
    setDocToolTestResults((prev) => ({
      ...prev,
      [id]: { testing: false }
    }))
  }

  const handleBrowseDocTool = async (
    id: 'word' | 'excel' | 'powerpoint' | 'pdf',
    name: string
  ): Promise<void> => {
    const picked = await window.api.files.pickExecutable(`Select ${name} Executable`)
    if (picked) {
      handleDocToolPathChange(id, picked)
    }
  }

  const handleDetectDocTool = (id: 'word' | 'excel' | 'powerpoint' | 'pdf'): void => {
    const detected = detectedDocTools[id]
    if (detected) {
      handleDocToolPathChange(id, detected)
    }
  }

  const handleClearDocTool = (id: 'word' | 'excel' | 'powerpoint' | 'pdf'): void => {
    handleDocToolPathChange(id, '')
  }

  const handleTestDocTool = async (
    id: 'word' | 'excel' | 'powerpoint' | 'pdf'
  ): Promise<void> => {
    const custom = settings.docToolPaths?.[id]?.trim()
    const detected = detectedDocTools[id]
    const target = custom || detected
    if (!target) {
      const defaultMsg = 'Will use System Default Application'
      setDocToolTestResults((prev) => ({
        ...prev,
        [id]: { testing: false, ok: true, version: defaultMsg }
      }))
      setSettings((prev) => {
        const nextSettings: WorkbenchSettings = {
          ...prev,
          docToolTestResults: {
            ...(prev.docToolTestResults || {}),
            [id]: {
              ok: true,
              version: defaultMsg,
              testedPath: 'System Default',
              testedAt: Date.now()
            }
          }
        }
        window.api.settings.set(nextSettings).catch(() => {})
        return nextSettings
      })
      return
    }

    setDocToolTestResults((prev) => ({
      ...prev,
      [id]: { testing: true }
    }))

    const res = await window.api.settings.testDocToolPath(target)
    const successVersion = res.version || t('settings.validExecutable') || 'Ready and valid'
    setDocToolTestResults((prev) => ({
      ...prev,
      [id]: {
        testing: false,
        ok: res.ok,
        version: res.ok ? successVersion : undefined,
        error: res.error
      }
    }))

    if (res.ok) {
      setSettings((prev) => {
        const nextSettings: WorkbenchSettings = {
          ...prev,
          docToolTestResults: {
            ...(prev.docToolTestResults || {}),
            [id]: {
              ok: true,
              version: successVersion,
              testedPath: target,
              testedAt: Date.now()
            }
          }
        }
        window.api.settings.set(nextSettings).catch((err) => {
          console.warn('[Settings] Failed to persist doc tool test result:', err)
        })
        return nextSettings
      })
    }
  }

  const handleDetectAllDocTools = (): void => {
    setSettings((prev) => ({
      ...prev,
      docToolPaths: {
        word: detectedDocTools.word || prev.docToolPaths?.word || '',
        excel: detectedDocTools.excel || prev.docToolPaths?.excel || '',
        powerpoint: detectedDocTools.powerpoint || prev.docToolPaths?.powerpoint || '',
        pdf: detectedDocTools.pdf || prev.docToolPaths?.pdf || ''
      }
    }))
  }

  const handleResetAllDocTools = (): void => {
    setSettings((prev) => ({
      ...prev,
      docToolPaths: {
        word: '',
        excel: '',
        powerpoint: '',
        pdf: ''
      }
    }))
  }

  const handleSave = async (): Promise<void> => {
    setSaving(true)
    setSaveError(null)
    try {
      await window.api.settings.set({
        ...settings,
        language
      })
      bumpSettings()
      setSaveSuccess(true)
      setTimeout(() => {
        setSaveSuccess(false)
        onClose()
      }, 600)
    } catch (err: unknown) {
      console.error('Failed to save settings:', err)
      const msg = err instanceof Error ? err.message : String(err)
      setSaveError(msg)
    } finally {
      setSaving(false)
    }
  }

  const handleCheckUpdate = async (): Promise<void> => {
    setCheckingUpdate(true)
    try {
      const st = await window.api.updater.check()
      setUpdaterStatus(st)
    } finally {
      setCheckingUpdate(false)
    }
  }

  const handleDownloadUpdate = async (): Promise<void> => {
    await window.api.updater.download()
  }

  const handleInstallUpdate = (): void => {
    window.api.updater.install()
  }

  const handleOpenReleasePage = (): void => {
    window.api.updater.openRelease()
  }

  const handleToggleAutoCheck = (checked: boolean): void => {
    setSettings((prev) => {
      const nextSettings: WorkbenchSettings = {
        ...prev,
        autoCheckUpdates: checked
      }
      window.api.settings.set(nextSettings).catch((err) => {
        console.error('Failed to save autoCheckUpdates setting:', err)
      })
      bumpSettings()
      return nextSettings
    })
  }

  return (
    <div className="apple-modal-backdrop" onClick={onClose}>
      <div className="macos-settings-window" onClick={(e) => e.stopPropagation()}>
        {/* 自帶的記憶 plugin：已啟用的 agent 沒裝或版本不對時，徵求同意後安裝 */}
        <MemoryPluginPrompt auto />
        {/* Left Sidebar */}
        <div className="macos-settings-sidebar">
          <div className="macos-sidebar-header">
            <h3 className="macos-sidebar-title">{t('settings.title')}</h3>
          </div>

          <nav className="macos-sidebar-nav">
            <button type="button" className={`macos-sidebar-item ${tab === 'proxy' ? 'active' : ''}`} onClick={() => setTab('proxy')}>
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-green)' }}><IconShield size={14} /></div>
              <span className="macos-sidebar-item-text">{t('proxy.title')}</span>
            </button>
            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'appearance' ? 'active' : ''}`}
              onClick={() => setTab('appearance')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-blue)' }}>
                <IconPalette size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.appearance')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'cli' ? 'active' : ''}`}
              onClick={() => setTab('cli')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-green)' }}>
                <IconTerminalBox size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.cliAgents')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'doctools' ? 'active' : ''}`}
              onClick={() => setTab('doctools')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-orange)' }}>
                <IconFileText size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.documentTools')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'extensions' ? 'active' : ''}`}
              onClick={() => setTab('extensions')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-purple)' }}>
                <IconPuzzle size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.extensions')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'remote' ? 'active' : ''}`}
              onClick={() => setTab('remote')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--morandi-green)' }}>
                <IconPhone size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('remote.tab')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'cowork' ? 'active' : ''}`}
              onClick={() => setTab('cowork')}
            >
              <div className="macos-icon-squircle" style={{ background: 'var(--accent)' }}>
                <IconCowork size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.coworkTab')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'memsync' ? 'active' : ''}`}
              onClick={() => setTab('memsync')}
            >
              <div className="macos-icon-squircle" style={{ background: '#475569' }}>
                <IconShield size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('memsync.tab')}</span>
            </button>

            <button
              type="button"
              className={`macos-sidebar-item ${tab === 'about' ? 'active' : ''}`}
              onClick={() => setTab('about')}
            >
              <div className="macos-icon-squircle" style={{ background: '#0284c7' }}>
                <IconInfo size={14} />
              </div>
              <span className="macos-sidebar-item-text">{t('settings.aboutTab')}</span>
              {updaterStatus?.updateAvailable && (
                <span
                  style={{
                    marginLeft: 'auto',
                    width: 7,
                    height: 7,
                    borderRadius: '50%',
                    background: '#ef4444',
                    boxShadow: '0 0 6px #ef4444'
                  }}
                  title={t('settings.updateAvailable', { version: updaterStatus.updateInfo?.version || '' })}
                />
              )}
            </button>
          </nav>
        </div>

        {/* Right Content Pane */}
        <div className="macos-settings-content">
          {tab === 'appearance' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('settings.appearanceTitle')}</h2>
                    <p className="macos-settings-desc">
                      {t('settings.appearanceDesc')}
                    </p>
                  </div>
                  <button
                    type="button"
                    className="macos-close-btn"
                    onClick={onClose}
                    title={t('settings.closeEsc')}
                  >
                    <IconClose size={12} />
                  </button>
                </div>
              </div>

              <div className="macos-settings-body">
                {/* Work Mode：Developer / Vibe Coding */}
                <div className="macos-section">
                  <span className="macos-section-header">{t('vibe.modeSection')}</span>
                  <div className="macos-inset-group" style={{ padding: '14px 16px' }}>
                    <div className="macos-lang-cards">
                      {(['developer', 'vibe', 'cowork'] as const).map((m) => (
                        <button
                          key={m}
                          type="button"
                          className={`macos-lang-card ${uiMode === m ? 'active' : ''}`}
                          onClick={() => setUiMode(m)}
                        >
                          <div
                            className="macos-lang-flag"
                            style={m === 'vibe' ? { background: 'var(--vibe-gradient)', color: '#fff' } : undefined}
                          >
                            {m === 'vibe' ? '✦' : m === 'cowork' ? '⌗' : '</>'}
                          </div>
                          <div className="macos-lang-info">
                            <span className="macos-lang-name">{t(`vibe.${m}Title`)}</span>
                            <span className="macos-lang-sub" title={t(`vibe.${m}Desc`)}>
                              {t(`vibe.${m}Desc`)}
                            </span>
                          </div>
                          <span className="macos-radio-dot" />
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                {/* Language Selection */}
                <div className="macos-section">
                  <span className="macos-section-header">{t('settings.languageSection')}</span>
                  <div className="macos-inset-group" style={{ padding: '14px 16px' }}>
                    <div className="macos-lang-cards">
                      <button
                        type="button"
                        className={`macos-lang-card ${language === 'en' ? 'active' : ''}`}
                        onClick={() => {
                          setLanguage('en')
                          setSettings((prev) => ({ ...prev, language: 'en' }))
                        }}
                      >
                        <div className="macos-lang-flag">EN</div>
                        <div className="macos-lang-info">
                          <span className="macos-lang-name">{t('settings.langEnglish')}</span>
                          <span className="macos-lang-sub">{t('settings.langEnglishSub')}</span>
                        </div>
                        <span className="macos-radio-dot" />
                      </button>

                      <button
                        type="button"
                        className={`macos-lang-card ${language === 'zh-TW' ? 'active' : ''}`}
                        onClick={() => {
                          setLanguage('zh-TW')
                          setSettings((prev) => ({ ...prev, language: 'zh-TW' }))
                        }}
                      >
                        <div className="macos-lang-flag">繁</div>
                        <div className="macos-lang-info">
                          <span className="macos-lang-name">{t('settings.langZhTW')}</span>
                          <span className="macos-lang-sub">{t('settings.langZhTWSub')}</span>
                        </div>
                        <span className="macos-radio-dot" />
                      </button>
                    </div>
                  </div>
                </div>

                <div className="macos-section">
                  <span className="macos-section-header">{t('settings.themeSection')}</span>
                  <div className="macos-inset-group" style={{ padding: '14px 16px' }}>
                    {/* 與工作模式／語言同款單行小卡，整頁視覺統一 */}
                    <div className="macos-lang-cards">
                      {(
                        [
                          ['light', <IconSun size={16} />, 'settings.lightMode', 'settings.lightModeSub'],
                          ['dark', <IconMoon size={16} />, 'settings.darkMode', 'settings.darkModeSub'],
                          ['light-morandi', <IconPalette size={16} />, 'settings.lightMorandi', 'settings.lightMorandiSub'],
                          ['dark-morandi', <IconPalette size={16} />, 'settings.darkMorandi', 'settings.darkMorandiSub']
                        ] as const
                      ).map(([id, icon, name, sub]) => (
                        <button
                          key={id}
                          type="button"
                          className={`macos-lang-card ${theme === id ? 'active' : ''}`}
                          onClick={() => setTheme(id)}
                        >
                          <div className={`macos-lang-flag macos-theme-swatch swatch-${id}`}>{icon}</div>
                          <div className="macos-lang-info">
                            <span className="macos-lang-name">{t(name)}</span>
                            <span className="macos-lang-sub" title={t(sub)}>
                              {t(sub)}
                            </span>
                          </div>
                          <span className="macos-radio-dot" />
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="macos-section">
                  <span className="macos-section-header">{t('settings.integrationSection')}</span>
                  <div className="macos-inset-group">
                    <div className="macos-row">
                      <div className="macos-row-main">
                        <div className="macos-row-left">
                          <div
                            className="macos-row-badge-icon"
                            style={{ background: '#7C3AED' }}
                          >
                            AI
                          </div>
                          <div className="macos-row-info">
                            <div className="macos-row-title-row">
                              <span className="macos-row-title">{t('settings.autoOpenFiles')}</span>
                              <span className="macos-type-pill">{t('settings.liveLink')}</span>
                            </div>
                            <span className="macos-row-sub">
                              {t('settings.autoOpenFilesDesc')}
                            </span>
                          </div>
                        </div>
                        <div className="macos-row-right">
                          <label
                            className="apple-toggle"
                            title={
                              settings.autoOpenAgentModifiedFiles !== false
                                ? 'Disable auto-opening files'
                                : 'Enable auto-opening files'
                            }
                          >
                            <input
                              type="checkbox"
                              checked={settings.autoOpenAgentModifiedFiles !== false}
                              onChange={(e) =>
                                setSettings((prev) => ({
                                  ...prev,
                                  autoOpenAgentModifiedFiles: e.target.checked
                                }))
                              }
                            />
                            <span className="apple-toggle-slider" />
                          </label>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </>
          )}

          {tab === 'cli' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('settings.cliTitle')}</h2>
                    <p className="macos-settings-desc">
                      {t('settings.cliDesc')}
                    </p>
                  </div>
                  <button
                    type="button"
                    className="macos-close-btn"
                    onClick={onClose}
                    title={t('settings.closeEsc')}
                  >
                    <IconClose size={12} />
                  </button>
                </div>
              </div>

              <div className="macos-settings-body">
                {/* ── CLI Permissions & Bypass Mode Section ────────── */}
                <div className="macos-section">
                  <span className="macos-section-header">{t('settings.cliPermissionsSection')}</span>
                  <div className="macos-inset-group">
                    <div className="macos-row">
                      <div className="macos-row-main">
                        <div className="macos-row-left">
                          <div
                            className="macos-row-badge-icon"
                            style={{ background: settings.cliBypassPermissions ? '#EA580C' : '#64748B' }}
                          >
                            <IconShield size={14} />
                          </div>
                          <div className="macos-row-info">
                            <div className="macos-row-title-row">
                              <span className="macos-row-title">{t('settings.cliBypassTitle')}</span>
                              <span
                                className={`macos-type-pill ${settings.cliBypassPermissions ? 'bypass-active' : ''}`}
                              >
                                {settings.cliBypassPermissions ? t('settings.cliBypassModePill') : 'Standard'}
                              </span>
                            </div>
                            <span className="macos-row-sub">{t('settings.cliBypassSub')}</span>
                          </div>
                        </div>

                        <div className="macos-row-right">
                          <label
                            className="apple-toggle"
                            title={
                              settings.cliBypassPermissions
                                ? 'Disable bypass mode'
                                : 'Enable bypass mode'
                            }
                          >
                            <input
                              type="checkbox"
                              checked={!!settings.cliBypassPermissions}
                              onChange={(e) =>
                                setSettings((prev) => ({
                                  ...prev,
                                  cliBypassPermissions: e.target.checked
                                }))
                              }
                            />
                            <span className="apple-toggle-slider" />
                          </label>
                        </div>
                      </div>

                      {/* Expandable info drawer when bypass mode is active */}
                      {settings.cliBypassPermissions && (
                        <div className="macos-bypass-drawer">
                          <div className="macos-bypass-warning-banner">
                            <span className="macos-bypass-warning-icon">⚠️</span>
                            <span className="macos-bypass-warning-text">
                              {t('settings.cliBypassWarning')}
                            </span>
                          </div>

                          <div className="macos-bypass-flags-header">
                            <span>{t('settings.cliBypassActiveFlags')}</span>
                          </div>

                          <div className="macos-bypass-cards">
                            <div className="macos-bypass-agent-card">
                              <div className="macos-bypass-agent-top">
                                <span className="macos-bypass-agent-badge" style={{ background: '#E05D26' }}>
                                  Claude Code
                                </span>
                                <span className="macos-bypass-agent-target">claude</span>
                              </div>
                              <div className="macos-bypass-code-box">
                                <code>claude --permission-mode bypassPermissions</code>
                              </div>
                            </div>

                            <div className="macos-bypass-agent-card">
                              <div className="macos-bypass-agent-top">
                                <span className="macos-bypass-agent-badge" style={{ background: '#10B981' }}>
                                  Codex CLI
                                </span>
                                <span className="macos-bypass-agent-target">codex</span>
                              </div>
                              <div className="macos-bypass-code-box">
                                <code>codex --dangerously-bypass-approvals-and-sandbox</code>
                              </div>
                            </div>

                            <div className="macos-bypass-agent-card">
                              <div className="macos-bypass-agent-top">
                                <span className="macos-bypass-agent-badge" style={{ background: '#6366F1' }}>
                                  Antigravity
                                </span>
                                <span className="macos-bypass-agent-target">agy</span>
                              </div>
                              <div className="macos-bypass-code-box">
                                <code>agy --dangerously-skip-permissions</code>
                              </div>
                            </div>
                          </div>
                        </div>
                      )}
                    </div>
                  </div>
                </div>

                <div className="macos-section">
                  <div className="macos-section-header-bar">
                    <span className="macos-section-header">{t('settings.supportedTools')}</span>
                    <div className="macos-section-actions">
                      <button
                        type="button"
                        className="macos-btn-link"
                        onClick={handleResetToAuto}
                        title="Clear custom paths and use dynamic auto-detection"
                      >
                        {t('settings.resetToAuto')}
                      </button>
                      <button
                        type="button"
                        className="macos-btn-link highlight"
                        onClick={handleDetectAll}
                        title="Fill all detected paths into inputs"
                      >
                        {t('settings.autoDetectAll')}
                      </button>
                    </div>
                  </div>
                  <div className="macos-inset-group">
                    {cliConfigs.map((cfg) => {
                      const isEnabled = settings.cliEnabled?.[cfg.id] !== false
                      const currentVal = settings.cliPaths?.[cfg.id] || ''
                      const detected = detectedPaths[cfg.id]
                      const test = testResults[cfg.id] || { testing: false }
                      const isExpanded = !!expandedCli[cfg.id]

                      return (
                        <div key={cfg.id} className={`macos-row ${!isEnabled ? 'disabled' : ''}`}>
                          <div className="macos-row-main">
                            <div className="macos-row-left">
                              <div
                                className="macos-row-badge-icon"
                                style={{ background: cfg.badgeColor }}
                              >
                                {cfg.shortBadge}
                              </div>
                              <div className="macos-row-info">
                                <div className="macos-row-title-row">
                                  <span className="macos-row-title">{cfg.name}</span>
                                  <span className="macos-type-pill">
                                    {cfg.category === 'agent' ? 'Agent' : 'Shell'}
                                  </span>
                                  {detected && !currentVal && isEnabled && (
                                    <span className="macos-detected-badge" title={detected}>
                                      Auto-detected
                                    </span>
                                  )}
                                </div>
                                <span className="macos-row-sub">{cfg.desc}</span>
                              </div>
                            </div>

                            <div className="macos-row-right">
                              <button
                                type="button"
                                className={`macos-disclosure-btn ${isExpanded ? 'open' : ''}`}
                                onClick={() => toggleExpand(cfg.id)}
                                title="Configure executable path"
                              >
                                <span>Path</span>
                                <IconChevronRight size={11} className="macos-disclosure-arrow" />
                              </button>

                              <label
                                className="apple-toggle"
                                title={isEnabled ? `Disable ${cfg.name}` : `Enable ${cfg.name}`}
                              >
                                <input
                                  type="checkbox"
                                  checked={isEnabled}
                                  onChange={(e) => handleToggleCli(cfg.id, e.target.checked)}
                                />
                                <span className="apple-toggle-slider" />
                              </label>
                            </div>
                          </div>

                          {/* Expandable path configuration drawer */}
                          {isExpanded && (
                            <div className="macos-row-drawer">
                              <div className="macos-drawer-label-row">
                                <span className="macos-drawer-label">Executable Path</span>
                                <span className="macos-drawer-default">
                                  Default: <code>{cfg.defaultCmd}</code>
                                </span>
                              </div>
                              <div className="macos-input-bar">
                                <input
                                  type="text"
                                  className="macos-input"
                                  placeholder={detected || `System default (${cfg.defaultCmd})`}
                                  value={currentVal}
                                  disabled={!isEnabled}
                                  onChange={(e) => handlePathChange(cfg.id, e.target.value)}
                                />
                                <button
                                  type="button"
                                  className="macos-btn-secondary"
                                  onClick={() => handleBrowseCli(cfg.id, cfg.name)}
                                  disabled={!isEnabled}
                                  title={t('settings.browse')}
                                >
                                  <IconFolderOpen size={13} />
                                  <span>{t('settings.browse')}</span>
                                </button>
                                <button
                                  type="button"
                                  className="macos-btn-secondary"
                                  onClick={() => handleDetectCli(cfg.id)}
                                  disabled={!isEnabled || detectingCli[cfg.id]}
                                  title={t('settings.detect')}
                                >
                                  {detectingCli[cfg.id] ? t('settings.detecting') : t('settings.detect')}
                                </button>
                                {cfg.category === 'agent' && (
                                  <button
                                    type="button"
                                    className="macos-btn-secondary macos-btn-install"
                                    onClick={() => handleRequestInstall(cfg.id)}
                                    disabled={!isEnabled || installingAgent[cfg.id]}
                                    title={
                                      detected || currentVal
                                        ? `${t('settings.reinstallAgent')} (${cfg.name})`
                                        : t('settings.installAgent')
                                    }
                                  >
                                    <IconDownload size={12} className={installingAgent[cfg.id] ? 'macos-spin' : ''} />
                                    <span>
                                      {installingAgent[cfg.id]
                                        ? t('settings.installingAgent')
                                        : detected || currentVal
                                        ? t('settings.reinstallAgent')
                                        : t('settings.installAgent')}
                                    </span>
                                  </button>
                                )}
                                <button
                                  type="button"
                                  className="macos-btn-secondary"
                                  onClick={() => handleTestPath(cfg.id)}
                                  disabled={!isEnabled || test.testing}
                                  title={t('settings.test')}
                                >
                                  {test.testing ? t('settings.testing') : t('settings.test')}
                                </button>
                              </div>

                              {detectFeedback[cfg.id] && (
                                <div className={`macos-test-result ${detectFeedback[cfg.id].type === 'ok' ? 'ok' : 'fail'}`}>
                                  {detectFeedback[cfg.id].type === 'ok' ? (
                                    <span>✓ {detectFeedback[cfg.id].msg}</span>
                                  ) : (
                                    <span>✕ {detectFeedback[cfg.id].msg}</span>
                                  )}
                                </div>
                              )}

                              {test.ok !== undefined && (
                                <div className={`macos-test-result ${test.ok ? 'ok' : 'fail'}`}>
                                  {test.ok ? (
                                    <span>✓ {t('settings.validExecutable')}: <strong>{test.version || 'OK'}</strong></span>
                                  ) : (
                                    <span>✕ Error: <span className="macos-error-tag">{test.error || 'Failed to execute'}</span></span>
                                  )}
                                </div>
                              )}
                            </div>
                          )}
                        </div>
                      )
                    })}
                  </div>
                </div>
                <CustomAgentSettings value={settings.customAgent} onChange={(customAgent) => setSettings((prev) => ({ ...prev, customAgent }))} />
              </div>
            </>
          )}

          {tab === 'doctools' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">Document Tools & Openers</h2>
                    <p className="macos-settings-desc">
                      Configure custom application paths for opening Word, Excel, PowerPoint, and PDF files.
                      If left blank, files automatically open in your system default application.
                    </p>
                  </div>
                  <button
                    type="button"
                    className="macos-close-btn"
                    onClick={onClose}
                    title="Close (Esc)"
                  >
                    <IconClose size={12} />
                  </button>
                </div>
              </div>

              <div className="macos-settings-body">
                {/* Global Actions Bar */}
                <div className="macos-doctools-topbar">
                  <button
                    type="button"
                    className="macos-btn-secondary"
                    onClick={handleDetectAllDocTools}
                    title="Automatically scan and detect Office and PDF applications installed on this machine"
                  >
                    <IconExternalLink size={12} />
                    <span>Auto-Detect Installed Tools</span>
                  </button>
                  <button
                    type="button"
                    className="macos-btn-secondary"
                    onClick={handleResetAllDocTools}
                    title="Clear all paths and restore system default applications"
                  >
                    <span>Reset All to System Default</span>
                  </button>
                </div>

                {/* Document Type Cards */}
                <div className="macos-section">
                  <span className="macos-section-header">Configured Openers</span>
                  <div className="macos-doctools-list">
                    {docToolConfigs.map((cfg) => {
                      const customPath = settings.docToolPaths?.[cfg.id] || ''
                      const detected = detectedDocTools[cfg.id] || ''
                      const testRes = docToolTestResults[cfg.id]
                      const isCustom = Boolean(customPath.trim())

                      return (
                        <div key={cfg.id} className="macos-doctool-card">
                          <div className="macos-doctool-card-header">
                            <div className="macos-doctool-badge-row">
                              <span
                                className="macos-doctool-badge"
                                style={{ backgroundColor: cfg.badgeColor }}
                              >
                                {cfg.iconLetter}
                              </span>
                              <div>
                                <div className="macos-doctool-name">{cfg.name}</div>
                                <div className="macos-doctool-exts">{cfg.exts}</div>
                              </div>
                            </div>

                            <div className="macos-doctool-status-pill">
                              {isCustom ? (
                                <span className="pill-custom">Custom Tool</span>
                              ) : (
                                <span className="pill-default">System Default</span>
                              )}
                            </div>
                          </div>

                          <div className="macos-doctool-desc">{cfg.desc}</div>

                          <div className="macos-doctool-input-row">
                            <input
                              type="text"
                              className="macos-input macos-doctool-input"
                              placeholder={
                                detected
                                  ? `Detected: ${detected}`
                                  : `e.g. C:\\Program Files\\... (Leave empty for system default)`
                              }
                              value={customPath}
                              onChange={(e) => handleDocToolPathChange(cfg.id, e.target.value)}
                            />

                            <button
                              type="button"
                              className="macos-btn-sm"
                              onClick={() => handleBrowseDocTool(cfg.id, cfg.name)}
                              title="Browse for executable file"
                            >
                              <IconFolderOpen size={13} />
                              <span>Browse…</span>
                            </button>

                            {detected && customPath !== detected && (
                              <button
                                type="button"
                                className="macos-btn-sm"
                                onClick={() => handleDetectDocTool(cfg.id)}
                                title={`Use detected application: ${detected}`}
                              >
                                <span>Use Detected</span>
                              </button>
                            )}

                            <button
                              type="button"
                              className="macos-btn-sm"
                              onClick={() => handleTestDocTool(cfg.id)}
                              disabled={testRes?.testing}
                              title="Test path verification"
                            >
                              {testRes?.testing ? 'Testing…' : 'Test'}
                            </button>

                            {isCustom && (
                              <button
                                type="button"
                                className="macos-btn-sm macos-btn-danger"
                                onClick={() => handleClearDocTool(cfg.id)}
                                title="Revert to system default application"
                              >
                                Clear
                              </button>
                            )}
                          </div>

                          {testRes && testRes.ok !== undefined && !testRes.testing && (
                            <div className={`macos-doctool-test-msg ${testRes.ok ? 'ok' : 'err'}`}>
                              {testRes.ok ? (
                                <>
                                  <IconCheck size={12} />
                                  <span>{testRes.version || 'Ready and verified'}</span>
                                </>
                              ) : (
                                <span>{testRes.error || 'Verification failed'}</span>
                              )}
                            </div>
                          )}
                        </div>
                      )
                    })}
                  </div>
                </div>
              </div>
            </>
          )}

          {tab === 'extensions' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">Extensions & Skills</h2>
                    <p className="macos-settings-desc">
                      Manage Antigravity custom plugins, skills, and background tools.
                    </p>
                  </div>
                  <button
                    type="button"
                    className="macos-close-btn"
                    onClick={onClose}
                    title="Close (Esc)"
                  >
                    <IconClose size={12} />
                  </button>
                </div>
              </div>
              <div className="macos-settings-body-customized">
                <CustomizedPanel />
              </div>
            </>
          )}

          {tab === 'proxy' && <>
            <div className="macos-settings-header"><h2 className="macos-settings-title">{t('proxy.title')}</h2></div>
            <ProxySettings value={settings.proxy} onChange={proxy => setSettings(prev => ({ ...prev, proxy }))} />
          </>}

          {tab === 'remote' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('remote.title')}</h2>
                    <p className="macos-settings-desc">{t('remote.desc')}</p>
                  </div>
                  <button type="button" className="macos-close-btn" onClick={onClose} title="Close (Esc)">
                    <IconClose size={12} />
                  </button>
                </div>
              </div>
              <RemoteSettings bypassActive={!!settings.cliBypassPermissions} />
            </>
          )}

          {tab === 'memsync' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('memsync.title')}</h2>
                    <p className="macos-settings-desc">{t('memsync.desc')}</p>
                  </div>
                  <button type="button" className="macos-close-btn" onClick={onClose} title={t('settings.closeEsc')}>
                    <IconClose size={12} />
                  </button>
                </div>
              </div>
              <MemorySyncSettings />
            </>
          )}

          {tab === 'cowork' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('settings.coworkTitle')}</h2>
                    <p className="macos-settings-desc">{t('settings.coworkDesc')}</p>
                  </div>
                  <button type="button" className="macos-close-btn" onClick={onClose} title={t('settings.closeEsc')}>
                    <IconClose size={12} />
                  </button>
                </div>
              </div>
              <div className="macos-settings-body">
                <CoworkSettings
                  value={settings.cowork}
                  bypass={!!settings.cliBypassPermissions}
                  onChange={(cowork) => setSettings((prev) => ({ ...prev, cowork }))}
                />
              </div>
            </>
          )}

          {tab === 'about' && (
            <>
              <div className="macos-settings-header">
                <div className="macos-settings-header-top">
                  <div>
                    <h2 className="macos-settings-title">{t('settings.aboutTitle')}</h2>
                    <p className="macos-settings-desc">{t('settings.aboutDesc')}</p>
                  </div>
                  <button
                    type="button"
                    className="macos-close-btn"
                    onClick={onClose}
                    title={t('settings.closeEsc')}
                  >
                    <IconClose size={12} />
                  </button>
                </div>
              </div>

              <div className="macos-settings-body">
                {/* App Brand Header Card */}
                <div className="macos-inset-group macos-about-hero">
                  <div className="macos-about-hero-icon">
                    <IconAppLogo size={52} />
                  </div>
                  <div className="macos-about-hero-info">
                    <div className="macos-about-hero-title">
                      Agent Workbench
                    </div>
                    <div className="macos-about-hero-meta">
                      <span className="macos-about-version">v{updaterStatus?.currentVersion || '0.1.3'}</span>
                      <span className="macos-channel-badge">
                        {!updaterStatus?.isPackaged
                          ? t('settings.channelDev')
                          : updaterStatus?.isInstalled
                          ? t('settings.channelInstalled')
                          : t('settings.channelPortable')}
                      </span>
                      {updaterStatus?.upstream && (
                        <span className="macos-channel-badge" title={updaterStatus.channelRepo}>
                          {t('settings.forkBadge', { base: updaterStatus.upstream.baseVersion || '?' })}
                        </span>
                      )}
                    </div>
                  </div>
                </div>

                {/* Update Status Section */}
                <div className="macos-section">
                  <div className="macos-section-header-bar">
                    <span className="macos-section-header">{t('settings.aboutTab')}</span>
                    <div className="macos-section-actions">
                      <button
                        type="button"
                        className="macos-btn-secondary"
                        onClick={handleCheckUpdate}
                        disabled={checkingUpdate || updaterStatus?.checking || updaterStatus?.isDownloading}
                        style={{ fontSize: '11.5px', padding: '4px 11px', display: 'flex', alignItems: 'center', gap: '6px' }}
                      >
                        <IconRefresh
                          size={12}
                          className={checkingUpdate || updaterStatus?.checking ? 'macos-spin' : ''}
                        />
                        <span>
                          {checkingUpdate || updaterStatus?.checking ? t('settings.checkingUpdates') : t('settings.checkForUpdates')}
                        </span>
                      </button>
                    </div>
                  </div>

                  <div className="macos-inset-group">
                    {/* Status Feedback */}
                    {updaterStatus?.updateAvailable ? (
                      <div className="macos-update-card">
                        <div className="macos-update-header">
                          <div className="macos-update-title">
                            <span className="macos-update-icon">🚀</span>
                            <span>{t('settings.updateAvailable', { version: updaterStatus.updateInfo?.version || '' })}</span>
                          </div>
                          {updaterStatus.updateInfo?.releaseDate && (
                            <span className="macos-update-date">
                              {new Date(updaterStatus.updateInfo.releaseDate).toLocaleDateString()}
                            </span>
                          )}
                        </div>

                        {/* Release Notes */}
                        {updaterStatus.updateInfo?.releaseNotes && (
                          <div className="macos-update-notes">
                            {formatReleaseNotes(updaterStatus.updateInfo.releaseNotes)}
                          </div>
                        )}

                        {/* Actions */}
                        {updaterStatus.isInstalled ? (
                          updaterStatus.updateDownloaded ? (
                            <button
                              type="button"
                              className="macos-btn-primary macos-update-btn"
                              onClick={handleInstallUpdate}
                            >
                              <span>🚀</span>
                              {t('settings.restartAndUpdate')}
                            </button>
                          ) : updaterStatus.isDownloading ? (
                            <div className="macos-update-progress-container">
                              <div className="macos-update-progress-label">
                                <span>{t('settings.downloadingUpdate', { percent: updaterStatus.downloadProgress?.percent || 0 })}</span>
                              </div>
                              <div className="macos-update-progress-track">
                                <div
                                  className="macos-update-progress-fill"
                                  style={{
                                    width: `${updaterStatus.downloadProgress?.percent || 0}%`
                                  }}
                                />
                              </div>
                            </div>
                          ) : (
                            <button
                              type="button"
                              className="macos-btn-primary macos-update-btn"
                              onClick={handleDownloadUpdate}
                            >
                              <IconDownload size={14} />
                              {t('settings.downloadUpdate')}
                            </button>
                          )
                        ) : (
                          <button
                            type="button"
                            className="macos-btn-primary macos-update-btn"
                            onClick={handleOpenReleasePage}
                          >
                            <IconDownload size={14} />
                            {t('settings.downloadPortablePackage')}
                          </button>
                        )}
                      </div>
                    ) : (
                      <div className="macos-row">
                        <div className="macos-row-main" style={{ padding: '14px 16px' }}>
                          <div className="macos-row-left" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                            <IconCheck size={18} style={{ color: 'var(--green)', flexShrink: 0 }} />
                            <div className="macos-row-info">
                              <div className="macos-row-title-row">
                                <span className="macos-row-title">{t('settings.upToDate')}</span>
                              </div>
                              <span className="macos-row-sub">
                                {updaterStatus?.currentVersion ? `Agent Workbench v${updaterStatus.currentVersion}` : ''}
                              </span>
                            </div>
                          </div>
                        </div>
                      </div>
                    )}

                    {updaterStatus?.error && (
                      <div className="macos-update-error">
                        ⚠️ {updaterStatus.error}
                      </div>
                    )}

                    {/* fork 版：原作者的版本只提示、不自動安裝（會蓋掉 fork 的改動） */}
                    {updaterStatus?.upstream && (
                      <div className="macos-row">
                        <div className="macos-row-main" style={{ padding: '14px 16px' }}>
                          <div className="macos-row-left" style={{ display: 'flex', alignItems: 'center', gap: '12px', minWidth: 0 }}>
                            {updaterStatus.upstream.newer ? (
                              <IconDownload size={18} style={{ color: 'var(--orange)', flexShrink: 0 }} />
                            ) : (
                              <IconCheck size={18} style={{ color: 'var(--green)', flexShrink: 0 }} />
                            )}
                            <div className="macos-row-info">
                              <div className="macos-row-title-row">
                                <span className="macos-row-title">
                                  {updaterStatus.upstream.newer
                                    ? t('settings.upstreamNewer', { version: updaterStatus.upstream.latestVersion || '' })
                                    : updaterStatus.upstream.latestVersion
                                    ? t('settings.upstreamCurrent')
                                    : t('settings.upstreamUnknown')}
                                </span>
                              </div>
                              <span className="macos-row-sub">
                                {t('settings.upstreamSub', {
                                  repo: updaterStatus.upstream.repo,
                                  base: updaterStatus.upstream.baseVersion || '?',
                                  latest: updaterStatus.upstream.latestVersion || '—'
                                })}
                                {updaterStatus.upstream.error ? ` · ${updaterStatus.upstream.error}` : ''}
                              </span>
                            </div>
                          </div>
                          {updaterStatus.upstream.latestUrl && (
                            <button
                              type="button"
                              className="macos-btn-secondary"
                              style={{ fontSize: '11.5px', padding: '4px 11px', flexShrink: 0 }}
                              onClick={() => window.api.updater.openRelease(updaterStatus.upstream?.latestUrl)}
                            >
                              {t('settings.openUpstreamRelease')}
                            </button>
                          )}
                        </div>
                      </div>
                    )}
                  </div>
                </div>

                {/* Preferences Section */}
                <div className="macos-section">
                  <div className="macos-section-header-bar">
                    <span className="macos-section-header">{t('settings.updatePreferencesSection')}</span>
                  </div>
                  <div className="macos-inset-group">
                    <div className="macos-row">
                      <div className="macos-row-main">
                        <div className="macos-row-left">
                          <div className="macos-row-info">
                            <div className="macos-row-title-row">
                              <span className="macos-row-title">{t('settings.autoCheckUpdates')}</span>
                            </div>
                            <span className="macos-row-sub">
                              {t('settings.autoCheckUpdatesDesc')}
                            </span>
                          </div>
                        </div>
                        <div className="macos-row-right">
                          <label className="apple-toggle">
                            <input
                              type="checkbox"
                              checked={settings.autoCheckUpdates ?? true}
                              onChange={(e) => handleToggleAutoCheck(e.target.checked)}
                            />
                            <span className="apple-toggle-slider" />
                          </label>
                        </div>
                      </div>
                    </div>

                    <div className="macos-row">
                      <div className="macos-row-main">
                        <div className="macos-row-left">
                          <div className="macos-row-info">
                            <div className="macos-row-title-row">
                              <span className="macos-row-title">{t('settings.openGithubReleases')}</span>
                            </div>
                            <span className="macos-row-sub">
                              {t('settings.footerAboutHint')}
                            </span>
                          </div>
                        </div>
                        <div className="macos-row-right">
                          <button
                            type="button"
                            className="macos-btn-secondary"
                            onClick={handleOpenReleasePage}
                            style={{ fontSize: '11.5px', padding: '3px 10px', display: 'flex', alignItems: 'center', gap: '6px' }}
                          >
                            <IconExternalLink size={12} />
                            <span>GitHub</span>
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </>
          )}

          {/* macOS Sheet Footer */}
          <div className="macos-settings-footer">
            <span className="macos-footer-hint">
              {saveError ? (
                <span style={{ color: '#ef4444', fontWeight: 500 }}>⚠️ {saveError}</span>
              ) : tab === 'proxy' ? (
                t('proxy.scope')
              ) : tab === 'cli' ? (
                t('settings.footerCliHint')
              ) : tab === 'doctools' ? (
                t('settings.footerDocToolsHint')
              ) : tab === 'about' ? (
                t('settings.footerAboutHint')
              ) : tab === 'cowork' ? (
                t('settings.footerCoworkHint')
              ) : tab === 'memsync' ? (
                t('memsync.footerHint')
              ) : (
                t('settings.footerGeneralHint')
              )}
            </span>
            <div className="macos-footer-actions">
              <button type="button" className="macos-btn-cancel" onClick={onClose}>
                {t('settings.cancel')}
              </button>
              <button
                type="button"
                className="macos-btn-primary"
                onClick={handleSave}
                disabled={saving}
              >
                {saveSuccess ? `${t('common.saved')} ✓` : saving ? t('common.saving') : t('settings.saveChanges')}
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* Apple HIG Modal Alert: CLI Install Confirmation */}
      {installConfirmAgent && (
        <div
          className="macos-install-confirm-backdrop"
          onClick={() => setInstallConfirmAgent(null)}
        >
          <div
            className="macos-install-confirm-card"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="macos-install-confirm-header">
              <div className="macos-install-confirm-badge">
                <IconDownload size={18} />
              </div>
              <div className="macos-install-confirm-title-col">
                <h3 className="macos-install-confirm-title">
                  {t('settings.installConfirmTitle', { agent: installConfirmAgent.name })}
                </h3>
                <p className="macos-install-confirm-desc">
                  {t('settings.installConfirmDesc')}
                </p>
              </div>
              <button
                type="button"
                className="macos-close-btn"
                onClick={() => setInstallConfirmAgent(null)}
                title={t('settings.closeEsc')}
              >
                <IconClose size={12} />
              </button>
            </div>

            <div className="macos-install-confirm-body">
              <div className="macos-install-info-group">
                <span className="macos-install-label">{t('settings.installConfirmPathLabel')}</span>
                <div className="macos-install-path-box">
                  <code>{installConfirmAgent.targetPath}</code>
                </div>
              </div>

              <div className="macos-install-info-group">
                <span className="macos-install-label">{t('settings.installConfirmCmdLabel')}</span>
                <div className="macos-install-cmd-box">
                  <code>{installConfirmAgent.command}</code>
                </div>
              </div>

              <div className="macos-install-notice-banner">
                <span className="macos-install-notice-icon">ℹ️</span>
                <span className="macos-install-notice-text">
                  {t('settings.installConfirmNotice')}
                </span>
              </div>
            </div>

            <div className="macos-install-confirm-footer">
              <button
                type="button"
                className="macos-btn-cancel"
                onClick={() => setInstallConfirmAgent(null)}
              >
                {t('settings.cancel')}
              </button>
              <button
                type="button"
                className="macos-btn-primary"
                onClick={handleConfirmInstall}
              >
                <IconDownload size={13} />
                <span>{t('settings.confirmInstall')}</span>
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
