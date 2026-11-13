import React, { useEffect, useState, useRef, useMemo, useCallback } from 'react'
import { createPortal } from 'react-dom'
import type { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import './terminal.css'
import {
  useWorkbench,
  getDraggedSession,
  setLiveAgentSessionIds,
  reportDevUrl,
  pulseAgentActivity,
  workspaceChangedByUser,
  setUiMode
} from '@/store'
import type { DraggedSessionPayload } from '@/store'
import { useTranslation } from '@/i18n'
import { detectDevUrl } from './portDetect'
import { focusTerm, isComposing } from './imeGuard'
import { createTerminalSession, xtermTheme } from './terminalSession'
import type { TerminalSession as TermSession } from './terminalSession'
import AgentMark from '@/components/AgentMark'
import { CUSTOM_AGENT_ID } from '../../../../shared/customAgent'
import {
  IconPlus,
  IconChevronDown,
  IconClose,
  IconClear,
  IconPopout,
  IconAttach,
  IconHandoff,
  IconTrash,
  IconSplitSingle,
  IconSplitHorizontal,
  IconSplitVertical,
  IconSplitGrid,
  IconSparkles,
  IconSend,
  IconCowork
} from '@/components/Icons'
import type { CliLauncher, WorkbenchSettings } from '../../../../preload/index'

interface DragOverState {
  mode: 'open' | 'handoff'
  fromAgent: string
  toAgent?: string
  targetName?: string
  sessionTitle: string
}

interface TerminalSession {
  id: string
  title: string
  /** 啟動用的 key：launcher yaml 的 id，或內建 Agent CLI 的名稱 */
  launcherKey: string
  args?: string[]
  term: Terminal
  /** xterm 與 PTY 的綁定（terminalSession.ts）；關分頁時 kill 由這裡自己送 */
  ts: TermSession
  ptyId?: string
  isExited: boolean
  needsApproval: boolean
  associatedSessionId?: string
  cwd?: string
  /** 開分頁當下所屬的工作區（cwd 有指定時就是 cwd）：切換工作區時找回該專案最後用的分頁 */
  workspace?: string
  /** 手機遠端開的終端：pty 已在 main 跑著，只要 attach、不要再 spawn */
  attachPtyId?: string
}

export interface AgentDefinition {
  id: string
  name: string
  at: string
  sub: string
  desc: string
  badge: string
}

export const BUILTIN_AGENTS: AgentDefinition[] = [
  {
    id: 'claude',
    name: 'Claude Code',
    at: '@claude',
    sub: 'Anthropic Agent CLI',
    desc: 'Claude Code interactive coding agent',
    badge: 'Anthropic'
  },
  {
    id: 'antigravity',
    name: 'Antigravity',
    at: '@antigravity',
    sub: 'DeepMind Agent CLI',
    desc: 'Google Antigravity autonomous coding agent',
    badge: 'Google'
  },
  {
    id: 'codex',
    name: 'Codex',
    at: '@codex',
    sub: 'OpenAI Agent CLI',
    desc: 'OpenAI Codex agent sandbox runner',
    badge: 'OpenAI'
  }
]

export interface ShellDefinition {
  id: string
  label: string
  sub: string
  desc: string
  badge: string
}

const isWindows = typeof navigator !== 'undefined' && navigator.userAgent.includes('Windows')
export const BUILTIN_SHELLS: ShellDefinition[] = isWindows
  ? [
      {
        id: 'powershell',
        label: 'PowerShell',
        sub: 'Windows PowerShell',
        desc: 'Default system automation and scripting shell',
        badge: 'powershell'
      },
      {
        id: 'cmd',
        label: 'Command Prompt',
        sub: 'cmd.exe',
        desc: 'Classic Windows command interpreter',
        badge: 'cmd'
      }
    ]
  : [
      {
        id: 'bash',
        label: 'bash',
        sub: 'Bourne-Again SHell',
        desc: 'Standard Unix command shell',
        badge: 'bash'
      },
      {
        id: 'pwsh',
        label: 'PowerShell (pwsh)',
        sub: 'PowerShell Core',
        desc: 'Cross-platform PowerShell shell',
        badge: 'pwsh'
      }
    ]

const AGENT_IDS = [...BUILTIN_AGENTS.map((a) => a.id), CUSTOM_AGENT_ID]
const SHELL_IDS = BUILTIN_SHELLS.map((s) => s.id)
/** 直接以指令啟動（非 launcher yaml）的項目 */
const DIRECT_IDS: string[] = [...AGENT_IDS, ...SHELL_IDS]

/** 比對工作區路徑用：不分大小寫、斜線統一、去掉結尾斜線（Windows 路徑大小寫不敏感） */
function normPath(p?: string | null): string {
  return (p || '').replace(/\\+/g, '/').replace(/\/+$/, '').toLowerCase()
}

/**
 * 以 bracketed paste 包裝：終端與 readline 會把中間內容當成「貼上」，
 * 換行不會被解讀成 Enter。少了這層，多行內容會被一行一行送出去執行。
 */
function wrapPaste(text: string): string {
  const clean = text.replace(/\r\n/g, '\n').replace(/\n+$/, '')
  return `\x1b[200~${clean}\x1b[201~`
}

/** 取終端內容：優先使用者選取範圍，沒有就抓可視區尾端數行 */
function readTerm(term: Terminal, maxLines = 60): string {
  const sel = term.getSelection()
  if (sel && sel.trim()) return sel.trim()
  const buf = term.buffer.active
  const end = buf.baseY + buf.cursorY
  const lines: string[] = []
  for (let i = Math.max(0, end - maxLines); i <= end; i++) {
    lines.push(buf.getLine(i)?.translateToString(true) ?? '')
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim()
}

/** 分割模式與各自的面板數 */
const SPLIT_MODES = [
  { id: 'single', label: 'Single', title: 'Single viewport (1 pane)', panes: 1, icon: IconSplitSingle },
  { id: 'cols2', label: 'Split V', title: 'Two panes side by side', panes: 2, icon: IconSplitVertical },
  { id: 'rows2', label: 'Split H', title: 'Two panes stacked', panes: 2, icon: IconSplitHorizontal },
  { id: 'grid4', label: 'Grid', title: 'Four panes (2 × 2)', panes: 4, icon: IconSplitGrid }
] as const
type SplitMode = (typeof SPLIT_MODES)[number]['id']

export default function TerminalPanel(): JSX.Element {
  const { t } = useTranslation()
  const { theme, terminalDispatch, terminalOpenSession, settingsTick, workspaceRoot } = useWorkbench()
  const workspaceRootRef = useRef(workspaceRoot)
  workspaceRootRef.current = workspaceRoot
  const [settings, setSettings] = useState<WorkbenchSettings>({
    cliPaths: {},
    cliEnabled: {}
  })

  useEffect(() => {
    window.api.settings.get().then((s) => {
      setSettings(s)
    })
  }, [settingsTick])

  const isCliEnabled = useCallback(
    (id: string): boolean => {
      if (id === CUSTOM_AGENT_ID) return !!settings.customAgent?.enabled && !!settings.customAgent.command
      return settings.cliEnabled?.[id] !== false
    },
    [settings]
  )

  const isBypassActive = !!settings.cliBypassPermissions

  const enabledAgents = useMemo(() => {
    const agents = BUILTIN_AGENTS.filter((a) => isCliEnabled(a.id))
    const c = settings.customAgent
    if (c && isCliEnabled(CUSTOM_AGENT_ID)) agents.push({
      id: CUSTOM_AGENT_ID, name: c.name, at: c.name, sub: c.command, desc: c.command, badge: 'Custom'
    })
    return agents
  }, [isCliEnabled])

  const enabledShells = useMemo(() => {
    return BUILTIN_SHELLS.filter((s) => isCliEnabled(s.id))
  }, [isCliEnabled])

  const [launchers, setLaunchers] = useState<CliLauncher[]>([])
  const [selectedLauncher, setSelectedLauncher] = useState<string>('')
  const [sessions, setSessions] = useState<TerminalSession[]>([])
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null)
  const [splitMode, setSplitMode] = useState<SplitMode>('single')
  // 最近使用順序：分割時就顯示最近用過的前 N 個，不必再另外挑面板
  const [mru, setMru] = useState<string[]>([])

  // Agent Prompt Dispatcher state (@ button)
  const [dispatchOpen, setDispatchOpen] = useState(false)
  const [dispatchAgent, setDispatchAgent] = useState<string>('claude')
  const [dispatchPrompt, setDispatchPrompt] = useState<string>('')
  const [dispatchAttachContext, setDispatchAttachContext] = useState<boolean>(false)
  const dispatchInputRef = useRef<HTMLTextAreaElement>(null)

  // Terminal right-click context menu state
  const [termContextMenu, setTermContextMenu] = useState<{
    x: number
    y: number
    sessionId: string
  } | null>(null)

  // Add terminal popover state (+ button)
  const [addMenuOpen, setAddMenuOpen] = useState(false)
  const [addMenuPos, setAddMenuPos] = useState<{ top: number; left: number } | null>(null)
  const addMenuRef = useRef<HTMLDivElement>(null)

  // Split popover state
  const [splitMenuOpen, setSplitMenuOpen] = useState(false)
  const [splitMenuPos, setSplitMenuPos] = useState<{ top: number; left: number } | null>(null)
  const splitMenuRef = useRef<HTMLDivElement>(null)

  // Handoff modal / popover state
  const [handoffOpen, setHandoffOpen] = useState(false)
  const [handoffPos, setHandoffPos] = useState<{ top: number; left: number } | null>(null)
  const [handoffNote, setHandoffNote] = useState('')
  const [selectedTargetId, setSelectedTargetId] = useState<string | null>(null)
  const [selectedNewAgent, setSelectedNewAgent] = useState<'claude' | 'antigravity' | 'codex' | null>(null)
  const handoffRef = useRef<HTMLDivElement>(null)

  // Drag and drop state for dashboard session cards
  const [dragOverInfo, setDragOverInfo] = useState<DragOverState | null>(null)
  const [hoveredTabId, setHoveredTabId] = useState<string | null>(null)
  const [hoveredLaunchpadAgent, setHoveredLaunchpadAgent] = useState<string | null>(null)
  const dragCounterRef = useRef(0)

  const isDetached =
    typeof window !== 'undefined' && window.location.search.includes('mode=terminal-detached')

  const openAgentDispatch = (e: React.MouseEvent<HTMLElement>): void => {
    e.stopPropagation()
    const active = sessions.find((s) => s.id === activeSessionId)
    if (active && enabledAgents.some((a) => a.id === active.launcherKey)) {
      setDispatchAgent(active.launcherKey)
    } else {
      setDispatchAgent(enabledAgents[0]?.id || 'claude')
    }
    setDispatchOpen(true)
    setAddMenuOpen(false)
    setSplitMenuOpen(false)
    setHandoffOpen(false)
    setTimeout(() => {
      dispatchInputRef.current?.focus()
    }, 60)
  }

  const toggleAddMenu = (e: React.MouseEvent<HTMLElement>): void => {
    e.stopPropagation()
    if (addMenuOpen) {
      setAddMenuOpen(false)
      return
    }
    const rect = e.currentTarget.getBoundingClientRect()
    const left = Math.min(rect.left, window.innerWidth - 250)
    setAddMenuPos({ top: rect.bottom + 6, left: Math.max(10, left) })
    setAddMenuOpen(true)
    setSplitMenuOpen(false)
    setHandoffOpen(false)
  }

  const toggleSplitMenu = (e: React.MouseEvent<HTMLElement>): void => {
    e.stopPropagation()
    if (splitMenuOpen) {
      setSplitMenuOpen(false)
      return
    }
    const rect = e.currentTarget.getBoundingClientRect()
    const left = Math.min(rect.left, window.innerWidth - 210)
    setSplitMenuPos({ top: rect.bottom + 6, left: Math.max(10, left) })
    setSplitMenuOpen(true)
    setAddMenuOpen(false)
    setHandoffOpen(false)
  }

  const toggleHandoff = (e: React.MouseEvent<HTMLElement>): void => {
    e.stopPropagation()
    if (handoffOpen) {
      setHandoffOpen(false)
      return
    }
    const rect = e.currentTarget.getBoundingClientRect()
    const left = Math.min(rect.right - 330, window.innerWidth - 350)
    setHandoffPos({ top: rect.bottom + 6, left: Math.max(10, left) })
    setHandoffOpen(true)
    setAddMenuOpen(false)
    setSplitMenuOpen(false)

    // 預先選取目標：若有其他執行中 session 預設選第一個，否則預設開第一個啟用的 Agent
    const others = sessions.filter((s) => s.id !== activeSessionId && !s.isExited)
    if (others.length > 0) {
      setSelectedTargetId(others[0].id)
      setSelectedNewAgent(null)
    } else {
      setSelectedTargetId(null)
      const firstAgent = (['claude', 'antigravity', 'codex'] as const).find((k) => isCliEnabled(k))
      setSelectedNewAgent(firstAgent || null)
    }
  }

  // 點選外部或縮放時自動關閉懸浮 popover
  useEffect(() => {
    if (!addMenuOpen && !splitMenuOpen && !handoffOpen) return
    const onDocClick = (e: MouseEvent): void => {
      const target = e.target as Node
      if (addMenuOpen && addMenuRef.current && !addMenuRef.current.contains(target)) {
        setAddMenuOpen(false)
      }
      if (splitMenuOpen && splitMenuRef.current && !splitMenuRef.current.contains(target)) {
        setSplitMenuOpen(false)
      }
      if (handoffOpen && handoffRef.current && !handoffRef.current.contains(target)) {
        setHandoffOpen(false)
      }
    }
    const onScrollOrResize = (): void => {
      setAddMenuOpen(false)
      setSplitMenuOpen(false)
      setHandoffOpen(false)
    }
    document.addEventListener('mousedown', onDocClick)
    window.addEventListener('resize', onScrollOrResize)
    window.addEventListener('scroll', onScrollOrResize, true)
    return () => {
      document.removeEventListener('mousedown', onDocClick)
      window.removeEventListener('resize', onScrollOrResize)
      window.removeEventListener('scroll', onScrollOrResize, true)
    }
  }, [addMenuOpen, splitMenuOpen, handoffOpen])

  const containerRef = useRef<HTMLDivElement>(null)
  // 讓非同步的送出流程讀得到最新 sessions（ptyId 是 spawn 後才填的）
  const sessionsRef = useRef<TerminalSession[]>([])
  sessionsRef.current = sessions
  // 目前排進分割版面的分頁（下面算出 visibleIds 後更新），審批通知用
  const visibleIdsRef = useRef<string[]>([])
  const lastDispatch = useRef(0)

  /**
   * 送文字到指定 session。剛建立的終端 ptyId 還沒回來，短暫重試等它就緒。
   * ponytail: 輪詢上限 6 秒；要更精準得把 spawn 完成事件拉到 panel 層，目前不值得。
   */
  const sendToSession = useCallback((sessionId: string, text: string): void => {
    const tryOnce = (attempt = 0): void => {
      const s = sessionsRef.current.find((x) => x.id === sessionId)
      if (s?.ptyId) {
        window.api.pty.write(s.ptyId, wrapPaste(text))
        return
      }
      if (attempt < 40) setTimeout(() => tryOnce(attempt + 1), 150)
    }
    tryOnce()
  }, [])

  /**
   * 剛開的 CLI 還沒畫完畫面、還沒開 bracketed paste 時就貼，開頭幾行的換行會被當成一般按鍵吃掉
   * （2026-10-07 實測 Codex）。等它的輸出靜止 1.5 秒（最多 20 秒）才貼。
   */
  const pasteWhenReady = useCallback(
    (sessionId: string, text: string): void => {
      const t0 = Date.now()
      let lastWrite = 0
      let sub: { dispose(): void } | null = null
      const tick = (): void => {
        const s = sessionsRef.current.find((x) => x.id === sessionId)
        if (s && !sub) sub = s.term.onWriteParsed(() => (lastWrite = Date.now()))
        const settled = lastWrite > 0 && Date.now() - lastWrite > 1500
        if (settled || Date.now() - t0 > 20000) {
          sub?.dispose()
          sendToSession(sessionId, text)
          return
        }
        setTimeout(tick, 200)
      }
      tick()
    },
    [sendToSession]
  )

  const termTheme = useMemo(() => xtermTheme(theme), [theme])

  const minContrast = theme === 'light' || theme === 'light-morandi' ? 4.5 : 1

  useEffect(() => {
    sessions.forEach((s) => {
      s.term.options.theme = termTheme
      s.term.options.minimumContrastRatio = minContrast
      try {
        s.term.refresh(0, s.term.rows - 1)
      } catch {
        // ignore if not ready
      }
    })
  }, [termTheme, minContrast, sessions])

  useEffect(() => {
    window.api.pty.launchers().then((list) => {
      setLaunchers(list.filter((l) => l.id !== CUSTOM_AGENT_ID))
      const firstEnabled = enabledAgents[0]?.id || enabledShells[0]?.id || list[0]?.id || BUILTIN_SHELLS[0].id
      setSelectedLauncher(list.length > 0 ? list[0].id : firstEnabled)
    })
  }, [enabledAgents, enabledShells])

  // 切到某分頁即視為使用者已看到，清掉紅點；同時把它移到 MRU 最前面
  const selectSession = useCallback((id: string): void => {
    setActiveSessionId(id)
    setMru((prev) => [id, ...prev.filter((x) => x !== id)])
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, needsApproval: false } : s)))
  }, [])

  const handleNewTerminal = useCallback(
    (
      launcherOverride?: string,
      args?: string[],
      titleOverride?: string,
      associatedSessionId?: string,
      cwd?: string,
      attachPtyId?: string
    ): string => {
      let key = launcherOverride || selectedLauncher
      if (!key || (DIRECT_IDS.includes(key) && !isCliEnabled(key))) {
        key = enabledAgents[0]?.id || enabledShells[0]?.id || launchers[0]?.id || BUILTIN_SHELLS[0].id
      }
      const sessionId = crypto.randomUUID()
      const agentDef = enabledAgents.find((a) => a.id === key)
      const shellDef = BUILTIN_SHELLS.find((s) => s.id === key)
      const title =
        titleOverride ||
        launchers.find((l) => l.id === key)?.name ||
        (agentDef ? agentDef.at : shellDef ? shellDef.label : key)

      const isAgent = AGENT_IDS.includes(key)
      // 保留上一段尾巴，避免網址剛好被切在兩個 chunk 之間
      let urlTail = ''
      const patch = (change: (s: TerminalSession) => Partial<TerminalSession>): void =>
        setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, ...change(s) } : s)))
      const ts = createTerminalSession({
        theme: termTheme,
        minimumContrastRatio: minContrast,
        onPtyId: (ptyId) => patch(() => ({ ptyId })),
        onOutput: (data) => {
          if (isAgent) pulseAgentActivity()
          const scan = urlTail + data
          urlTail = scan.slice(-200)
          if (scan.includes('http://')) {
            const devUrl = detectDevUrl(scan)
            if (devUrl) reportDevUrl(devUrl)
          }
        },
        onApproval: (waiting) => {
          patch(() => ({ needsApproval: waiting }))
          // 看得到就不必再用系統通知打擾
          if (waiting && !visibleIdsRef.current.includes(sessionId)) {
            window.api.notify.show('Approval needed', `${title} is waiting for a response`)
          }
        },
        onExit: (code) => {
          patch((s) => ({ isExited: true, needsApproval: false, title: `${s.title} (exited ${code})` }))
          window.api.notify.show('Task finished', `${title} exited with code ${code}`)
        }
      })

      setSessions((prev) => [
        ...prev,
        {
          id: sessionId,
          title,
          launcherKey: key,
          args,
          term: ts.term,
          ts,
          isExited: false,
          needsApproval: false,
          associatedSessionId,
          cwd,
          workspace: cwd || workspaceRootRef.current || undefined,
          attachPtyId
        }
      ])
      setMru((prev) => [sessionId, ...prev])
      setActiveSessionId(sessionId)
      return sessionId
    },
    [selectedLauncher, isCliEnabled, enabledAgents, enabledShells, launchers, termTheme, minContrast]
  )

  // 手機遠端開的終端：在這個視窗開一個分頁 attach 上去，桌面與手機看到同一個 CLI
  const handleNewTerminalRef = useRef(handleNewTerminal)
  handleNewTerminalRef.current = handleNewTerminal
  useEffect(() => {
    return window.api.pty.onRemoteSpawned((info) => {
      handleNewTerminalRef.current(info.launcherKey, undefined, info.title, undefined, undefined, info.ptyId)
    })
  }, [])

  /** 開啟或切換至特定 session 的 CLI 終端（點選或拖曳時共用） */
  const openOrResumeSession = useCallback(
    (req: { id?: string; agent: string; title?: string; status?: string; workspacePath?: string }) => {
      // 1. 若現有終端 session 中有匹配者（ptyId、session id 或 associatedSessionId 匹配）
      const matchedPty = sessionsRef.current.find(
        (s) => req.id && (s.id === req.id || s.ptyId === req.id || s.associatedSessionId === req.id)
      )
      if (matchedPty) {
        selectSession(matchedPty.id)
        setTimeout(() => focusTerm(matchedPty.term), 60)
        return matchedPty.id
      }

      // 這裡刻意沒有「status==='active' 就切到該 Agent 任一執行中分頁」的後備：
      // 那個比對只看 agent 種類、不看 session，結果是終端已經開著 Claude 時，
      // 點 Dashboard 上另一支 Claude 會話只會跳回既有分頁，永遠開不了第二支。
      // 真正同一支會話的情況上面第 1 步（id / ptyId / associatedSessionId）已經涵蓋。

      // 2. 歷史 session 或尚無對應分頁：新開該 Agent CLI 並傳入 resume / conversation 參數
      let args: string[] | undefined
      if (req.agent === 'claude' && req.id) {
        args = ['--resume', req.id]
      } else if (req.agent === 'antigravity' && req.id) {
        args = ['--conversation', req.id]
      } else if (req.agent === 'codex' && req.id) {
        // codex 的 resume 是子命令＋位置參數（`codex resume <id>`），不是 flag
        args = ['resume', req.id]
      }

      const titlePrefix =
        req.agent === 'claude'
          ? '@claude'
          : req.agent === 'antigravity'
          ? '@antigravity'
          : `@${req.agent}`
      const title = req.title ? `${titlePrefix}: ${req.title.slice(0, 18)}` : titlePrefix

      const newId = handleNewTerminal(req.agent, args, title, req.id, req.workspacePath)
      selectSession(newId)
      return newId
    },
    [handleNewTerminal, selectSession]
  )

  // ── 切換工作區 → 切到該專案最後用的 CLI ─────────────────────────────
  // 有開著的分頁：切到該專案最近用過的那個。沒有：從儀表板紀錄找該專案最後一次的
  // Claude／Codex／Antigravity 會話並接續（openOrResumeSession）。啟動時從後端還原工作區不算切換；
  // 但從空白工作區（預設啟動狀態）第一次切到資料夾要算。
  const mruRef = useRef<string[]>([])
  mruRef.current = mru
  const prevWorkspaceRef = useRef(workspaceRoot)
  useEffect(() => {
    const prev = prevWorkspaceRef.current
    prevWorkspaceRef.current = workspaceRoot
    if ((!prev && !workspaceChangedByUser) || !workspaceRoot || isDetached || normPath(prev) === normPath(workspaceRoot)) return
    const root = normPath(workspaceRoot)
    const inWorkspace = (p?: string): boolean => {
      const n = normPath(p)
      return !!n && (n === root || n.startsWith(root + '/'))
    }
    const lastOpenTab = (): TerminalSession | undefined => {
      const mine = sessionsRef.current.filter((s) => !s.isExited && inWorkspace(s.workspace))
      if (!mine.length) return undefined
      const rank = (s: TerminalSession): number => {
        const i = mruRef.current.indexOf(s.id)
        return i === -1 ? Number.MAX_SAFE_INTEGER : i
      }
      return [...mine].sort((a, b) => rank(a) - rank(b))[0]
    }
    const open = lastOpenTab()
    if (open) {
      selectSession(open.id)
      setTimeout(() => focusTerm(open.term), 60)
      return
    }
    let cancelled = false
    window.api.dashboard
      .data(false)
      .then((data) => {
        // 等資料的期間使用者又切走了，或已經有該專案的分頁（例如從儀表板點會話開的）→ 不再自動開
        if (cancelled || normPath(workspaceRootRef.current) !== root || lastOpenTab()) return
        const last = data.sessions
          .filter(
            (s) =>
              inWorkspace(s.workspacePath) &&
              !s.isArchived &&
              ['claude', 'codex', 'antigravity'].includes(s.agent) &&
              isCliEnabled(s.agent)
          )
          .sort((a, b) => new Date(b.lastActiveTime).getTime() - new Date(a.lastActiveTime).getTime())[0]
        if (!last) return
        openOrResumeSession({ id: last.id, agent: last.agent, title: last.title, status: last.status, workspacePath: workspaceRoot })
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [workspaceRoot])

  /** 將 Session 資料轉化為跨 Agent Handoff 提示詞並送入目標終端 */
  const handleHandoffFromSession = useCallback(
    (
      dragged: DraggedSessionPayload,
      targetSession: TerminalSession,
      customNote?: string
    ) => {
      let recentOutput = ''
      const srcRunning = sessionsRef.current.find(
        (s) => s.id === dragged.id || s.ptyId === dragged.id
      )
      if (srcRunning) {
        recentOutput = readTerm(srcRunning.term, 35)
      }

      const noteText = customNote?.trim()
        ? `\nNote / Instruction: ${customNote.trim()}\n`
        : '\nInstruction: Please review the context above, take over the task, and continue working on this project.\n'

      const prompt =
        `[Cross-Agent Handoff: Task Transfer from @${dragged.agent}]\n` +
        `• Task: ${dragged.title}\n` +
        `• Source Session ID: ${dragged.id}\n` +
        (dragged.workspace ? `• Workspace: ${dragged.workspace}\n` : '') +
        (dragged.workspacePath ? `• Workspace Path: ${dragged.workspacePath}\n` : '') +
        (dragged.model ? `• Model: ${dragged.model}\n` : '') +
        (recentOutput ? `\nRecent Source Session Output:\n---\n${recentOutput}\n---\n` : '') +
        noteText

      sendToSession(targetSession.id, prompt + '\r\n')
      selectSession(targetSession.id)
      setTimeout(() => focusTerm(targetSession.term), 60)

      window.api.notify?.show?.(
        'Agent Handoff Dispatched',
        `Task from @${dragged.agent} handed off to ${targetSession.title}`
      )
    },
    [sendToSession, selectSession]
  )

  /** 建立新 Agent 終端並即時交接 Session 任務 */
  const handleLaunchAndHandoff = useCallback(
    (dragged: DraggedSessionPayload, targetAgentKey: string) => {
      const newId = handleNewTerminal(targetAgentKey)
      selectSession(newId)
      setTimeout(() => {
        const target = sessionsRef.current.find((x) => x.id === newId)
        if (target) {
          handleHandoffFromSession(dragged, target)
        }
      }, 80)
    },
    [handleNewTerminal, selectSession, handleHandoffFromSession]
  )

  /** 取得拖曳資料（支援記憶體快顯與原生 dataTransfer） */
  const getSessionFromDrag = useCallback((e: React.DragEvent): DraggedSessionPayload | null => {
    const mem = getDraggedSession()
    if (mem) return mem
    try {
      const data = e.dataTransfer.getData('application/x-agent-session')
      if (data) return JSON.parse(data)
    } catch {
      // ignore
    }
    return null
  }, [])

  const handleStageDragEnter = (e: React.DragEvent): void => {
    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
    dragCounterRef.current++
    const dragged = getDraggedSession()
    if (!dragged) return

    const activeSession = sessions.find((s) => s.id === activeSessionId)
    if (activeSession && activeSession.launcherKey !== dragged.agent) {
      setDragOverInfo({
        mode: 'handoff',
        fromAgent: dragged.agent,
        toAgent: activeSession.launcherKey,
        targetName: activeSession.title,
        sessionTitle: dragged.title
      })
    } else {
      setDragOverInfo({
        mode: 'open',
        fromAgent: dragged.agent,
        sessionTitle: dragged.title
      })
    }
  }

  const handleStageDragLeave = (e: React.DragEvent): void => {
    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
    dragCounterRef.current--
    if (dragCounterRef.current <= 0) {
      dragCounterRef.current = 0
      setDragOverInfo(null)
    }
  }

  const handleStageDragOver = (e: React.DragEvent): void => {
    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  }

  const handleStageDrop = (e: React.DragEvent): void => {
    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
    e.preventDefault()
    dragCounterRef.current = 0
    setDragOverInfo(null)
    setHoveredTabId(null)
    setHoveredLaunchpadAgent(null)

    const dragged = getSessionFromDrag(e)
    if (!dragged) return

    const activeSession = sessions.find((s) => s.id === activeSessionId)
    if (activeSession && activeSession.launcherKey !== dragged.agent) {
      handleHandoffFromSession(dragged, activeSession)
    } else {
      openOrResumeSession(dragged)
    }
  }

  // 外部（如 Markdown code block）送指令進來：送給作用中或開啟中的 terminal/agent
  useEffect(() => {
    if (!terminalDispatch || terminalDispatch.nonce === lastDispatch.current) return
    lastDispatch.current = terminalDispatch.nonce

    if (terminalDispatch.target === 'active' && activeSessionId) {
      sendToSession(activeSessionId, terminalDispatch.text)
      return
    }
    if (terminalDispatch.newShell) {
      // 只接受內建 shell；剛開的 shell 還在載入設定檔，等輸出靜止再貼，免得開頭被吃掉
      const key = SHELL_IDS.includes(terminalDispatch.newShell) ? terminalDispatch.newShell : enabledShells[0]?.id || BUILTIN_SHELLS[0].id
      const fresh = handleNewTerminal(key)
      selectSession(fresh)
      pasteWhenReady(fresh, terminalDispatch.text)
      return
    }
    // target 'shell' 就要真的是 shell：先前是「任何還活著的分頁」，
    // 於是 markdown 裡的指令會落進正在跑的 agent 提示字元，被當成對 agent 說的話。
    const running = sessionsRef.current.find((s) => !s.isExited && SHELL_IDS.includes(s.launcherKey))
    const fallbackKey = enabledShells[0]?.id || BUILTIN_SHELLS[0].id
    const id = running ? running.id : handleNewTerminal(fallbackKey)
    selectSession(id)
    sendToSession(id, terminalDispatch.text)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalDispatch, enabledShells, enabledAgents])

  // 把「還活著的 Agent 分頁」回報給 store。Dashboard 有兩個用途：
  //   1. 帶 associatedSessionId 的（從卡片開的）→ 對應卡片標成 active。
  //   2. 從終端直接開的 CLI 沒有會話 id，這裡用分頁自身的 id 當佔位——它比對不到任何卡片，
  //      但會讓這個陣列改變，Dashboard 據此立刻重抓，新會話才不用等到下次重開 App 才出現。
  useEffect(() => {
    setLiveAgentSessionIds(
      sessions
        .filter((s) => !s.isExited && (s.associatedSessionId || AGENT_IDS.includes(s.launcherKey)))
        .map((s) => s.associatedSessionId || s.id)
    )
  }, [sessions])

  const lastOpenSessionNonce = useRef(0)

  // 跨面板（如 Dashboard）請求開啟/切換特定 Session 的 CLI 終端
  useEffect(() => {
    if (!terminalOpenSession || terminalOpenSession.nonce === lastOpenSessionNonce.current) return
    lastOpenSessionNonce.current = terminalOpenSession.nonce
    openOrResumeSession(terminalOpenSession)
  }, [terminalOpenSession, openOrResumeSession])

  /** 把「目前分頁的選取內容（或尾端數行）」送到另一個 session */
  const sendActiveTo = (targetId: string): void => {
    const src = sessionsRef.current.find((s) => s.id === activeSessionId)
    if (!src) return
    const text = readTerm(src.term)
    if (!text) return
    sendToSession(targetId, text)
    selectSession(targetId)
  }

  const closeTerminal = (id: string, e?: React.MouseEvent): void => {
    e?.stopPropagation()
    setMru((prev) => prev.filter((x) => x !== id))
    setSessions((prev) => {
      const idx = prev.findIndex((s) => s.id === id)
      if (idx === -1) return prev
      const session = prev[idx]

      if (session.ptyId) window.api.pty.kill(session.ptyId)
      session.ts.dispose()

      const next = [...prev]
      next.splice(idx, 1)

      // 關閉到剩 1 個或 0 個 session 時，自動回歸 single 模式，避免留置空白死區
      if (next.length <= 1) {
        setSplitMode('single')
      }

      if (activeSessionId === id) {
        setActiveSessionId(next.length > 0 ? next[Math.max(0, idx - 1)].id : null)
      }
      return next
    })
  }

  const handleHandoff = (
    fromId: string,
    target: { type: 'existing'; id: string } | { type: 'new'; key: string },
    instruction?: string
  ): void => {
    const fromSession = sessions.find((s) => s.id === fromId)
    if (!fromSession) return

    let targetId: string
    let targetTitle: string

    if (target.type === 'existing') {
      const toSession = sessions.find((s) => s.id === target.id)
      if (!toSession) return
      targetId = toSession.id
      targetTitle = toSession.title
    } else {
      targetId = handleNewTerminal(target.key)
      targetTitle =
        target.key === 'claude'
          ? 'Claude Code'
          : target.key === 'antigravity'
          ? 'Antigravity'
          : target.key === 'codex'
          ? 'Codex'
          : target.key
    }

    const recentOutput = readTerm(fromSession.term, 35)
    const customNote = instruction?.trim()
      ? `\nNote / Instruction: ${instruction.trim()}\n`
      : '\nPlease continue and verify the work:\n'

    const prompt =
      `[Cross-Agent Handoff from ${fromSession.title}]:` +
      customNote +
      `Here is the latest status from my session:\n` +
      `---\n${recentOutput}\n---`

    sendToSession(targetId, prompt + '\r\n')
    selectSession(targetId)
    setHandoffOpen(false)
    setHandoffNote('')
    window.api.notify.show('Handoff Dispatched', `Context piped from ${fromSession.title} to ${targetTitle}`)
  }

  const pendingCount = sessions.filter((s) => s.needsApproval).length

  // 分割時要顯示哪幾個：MRU 前 N 個（若 session 數小於設定分割數，自動收斂避免空白凍結死區）
  const configuredPanes = SPLIT_MODES.find((m) => m.id === splitMode)!.panes
  const effectiveSplitMode: SplitMode =
    sessions.length <= 1 ? 'single' : sessions.length < configuredPanes ? 'single' : splitMode
  const paneCount = SPLIT_MODES.find((m) => m.id === effectiveSplitMode)!.panes

  const order = [
    ...mru.filter((id) => sessions.some((s) => s.id === id)),
    ...sessions.filter((s) => !mru.includes(s.id)).map((s) => s.id)
  ]
  const visibleIds = order.slice(0, paneCount)
  visibleIdsRef.current = visibleIds

  // 渲染極簡 Apple 風格啟動下拉選單（透過 createPortal 浮動於頂層，不被 overflow-x 捲動容器干擾）
  const activeSession = sessions.find((s) => s.id === activeSessionId)

  // 送出提示與問題至指定的 Agent CLI
  const handleDispatchToAgent = (): void => {
    const prompt = dispatchPrompt.trim()
    if (!prompt) return

    // 尋找目標 agent 目前是否已有執行中 session
    const existing = sessions.find((s) => s.launcherKey === dispatchAgent && !s.isExited)
    let targetId = existing?.id

    // 若尚未開啟該 agent session，自動新建一個！
    if (!targetId) {
      targetId = handleNewTerminal(dispatchAgent)
    }

    let fullPayload = prompt
    if (dispatchAttachContext && activeSession) {
      const text = readTerm(activeSession.term, 50)
      if (text) {
        fullPayload = `[Context from terminal: ${activeSession.title}]\n\`\`\`\n${text}\n\`\`\`\n\n[Task / Question]:\n${prompt}`
      }
    }

    sendToSession(targetId, fullPayload + '\r\n')
    selectSession(targetId)
    setDispatchOpen(false)
    setDispatchPrompt('')
  }

  // 渲染 Apple Spotlight 風格之 @ Agent Prompt Dispatcher
  const renderAgentDispatch = (): JSX.Element => (
    <div className="term-agent-picker-wrap">
      <button
        className={`term-agent-dispatch-btn ${dispatchOpen ? 'open' : ''}`}
        onClick={openAgentDispatch}
        title={
          enabledAgents.length > 0
            ? "Prompt Agent CLI: Ask question or send task to @claude, @antigravity, @codex"
            : "All AI Agents disabled in Settings"
        }
        disabled={enabledAgents.length === 0}
      >
        <span className="at-sign">@</span>
        <span>Prompt</span>
        <IconSparkles size={11} />
      </button>

      {dispatchOpen &&
        createPortal(
          <div
            className="term-dispatch-backdrop"
            onClick={() => setDispatchOpen(false)}
          >
            <div
              className="term-dispatch-modal"
              onClick={(e) => e.stopPropagation()}
            >
              {/* Header: Title + Agent Selector Chips */}
              <div className="term-dispatch-header">
                <div className="term-dispatch-header-title">
                  <IconSparkles size={14} style={{ color: 'var(--accent)' }} />
                  <span>Prompt Agent CLI</span>
                </div>
                <div className="term-dispatch-agent-chips">
                  {enabledAgents.map((agent) => (
                    <div
                      key={agent.id}
                      data-agent={agent.id}
                      className={`term-dispatch-chip ${dispatchAgent === agent.id ? 'selected' : ''}`}
                      onClick={() => setDispatchAgent(agent.id)}
                    >
                      <AgentMark agent={agent.id as any} size={13} />
                      <span>{agent.at}</span>
                    </div>
                  ))}
                </div>
              </div>

              {/* Body: Prompt input + optional context attach */}
              <div className="term-dispatch-body">
                <div className="term-dispatch-input-wrap">
                  <textarea
                    ref={dispatchInputRef}
                    className="term-dispatch-textarea"
                    placeholder={`Ask @${dispatchAgent} a question, explain errors, or assign a task...`}
                    value={dispatchPrompt}
                    onChange={(e) => setDispatchPrompt(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault()
                        handleDispatchToAgent()
                      } else if (e.key === 'Escape') {
                        e.preventDefault()
                        setDispatchOpen(false)
                      }
                    }}
                  />
                </div>

                {activeSession && (
                  <div className="term-dispatch-context-row">
                    <label className="term-dispatch-context-label">
                      <input
                        type="checkbox"
                        checked={dispatchAttachContext}
                        onChange={(e) => setDispatchAttachContext(e.target.checked)}
                      />
                      <span>Attach terminal output from <strong>{activeSession.title}</strong></span>
                    </label>
                    <span className="term-dispatch-context-preview">
                      {dispatchAttachContext ? '✓ Output buffer included' : 'No context attached'}
                    </span>
                  </div>
                )}
              </div>

              {/* Footer: Shortcut hints + Send Button */}
              <div className="term-dispatch-footer">
                <div className="term-dispatch-hints">
                  <kbd>↵</kbd> Send &nbsp;•&nbsp; <kbd>⇧↵</kbd> Newline &nbsp;•&nbsp; <kbd>esc</kbd> Dismiss
                </div>
                <div className="term-dispatch-actions">
                  <button
                    type="button"
                    className="term-dispatch-btn-cancel"
                    onClick={() => setDispatchOpen(false)}
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="term-dispatch-btn-send"
                    disabled={!dispatchPrompt.trim()}
                    onClick={handleDispatchToAgent}
                  >
                    <IconSend size={12} />
                    <span>Send to @{dispatchAgent}</span>
                  </button>
                </div>
              </div>
            </div>
          </div>,
          document.body
        )}
    </div>
  )

  // Cowork 2.0 是獨立的畫布模式，不再是終端區的分頁；這裡只是入口
  const openCowork = (): void => {
    setAddMenuOpen(false)
    setUiMode('cowork')
  }

  // 渲染新增終端選單（支援 PowerShell, Command Prompt 與 AI Agents）
  const renderAddMenu = (): JSX.Element => (
    <div className="term-agent-picker-wrap">
      <button
        className={`term-add-dropdown-btn ${addMenuOpen ? 'open' : ''}`}
        onClick={toggleAddMenu}
        title="New Terminal or Cowork"
      >
        <IconPlus size={11} />
        <IconChevronDown size={8} />
      </button>

      {addMenuOpen &&
        addMenuPos &&
        createPortal(
          <div
            ref={addMenuRef}
            className="term-popover-portal term-agent-popover"
            style={{
              position: 'fixed',
              top: addMenuPos.top,
              left: addMenuPos.left,
              zIndex: 99999
            }}
          >
            {enabledShells.length > 0 && (
              <>
                <div className="term-popover-header">System Terminals</div>
                {enabledShells.map((sh) => {
                  const isCurrent = activeSession?.launcherKey === sh.id
                  return (
                    <div
                      key={sh.id}
                      className={`term-popover-item ${isCurrent ? 'selected' : ''}`}
                      onClick={() => {
                        handleNewTerminal(sh.id)
                        setAddMenuOpen(false)
                      }}
                    >
                      <AgentMark agent="shell" size={15} />
                      <div className="term-popover-item-details">
                        <div className="term-popover-item-top">
                          <span className="term-popover-name">{sh.label}</span>
                          <span className="term-popover-badge">{sh.sub}</span>
                        </div>
                        <span className="term-popover-desc">{sh.desc}</span>
                      </div>
                    </div>
                  )
                })}
              </>
            )}

            {enabledAgents.length > 0 && (
              <>
                {enabledShells.length > 0 && <div className="term-popover-divider" />}
                <div className="term-popover-header">AI Agents</div>
                {enabledAgents.map((agent) => {
                  const isCurrent = activeSession?.launcherKey === agent.id
                  return (
                    <div
                      key={agent.id}
                      className={`term-popover-item ${isCurrent ? 'selected' : ''}`}
                      onClick={() => {
                        handleNewTerminal(agent.id)
                        setAddMenuOpen(false)
                      }}
                    >
                      <AgentMark agent={agent.id as any} size={15} />
                      <div className="term-popover-item-details">
                        <div className="term-popover-item-top">
                          <span className="term-popover-name">{agent.at}</span>
                          <span className="term-popover-badge">{agent.badge}</span>
                          {isBypassActive && agent.id !== CUSTOM_AGENT_ID && (
                            <span className="term-popover-bypass-pill">Bypass</span>
                          )}
                        </div>
                        <span className="term-popover-desc">{agent.desc}</span>
                      </div>
                    </div>
                  )
                })}
              </>
            )}

            {(enabledShells.length > 0 || enabledAgents.length > 0) && <div className="term-popover-divider" />}
            <button type="button" className="term-popover-item" onClick={openCowork}>
              <IconCowork size={15} />
              <div className="term-popover-item-details">
                <span className="term-popover-name">{t('cowork.tab')}</span>
                <span className="term-popover-desc">{t('cowork.buttonTitle')}</span>
              </div>
            </button>

            {launchers.length > 0 && (
              <>
                {(enabledShells.length > 0 || enabledAgents.length > 0) && <div className="term-popover-divider" />}
                <div className="term-popover-header">Custom Launchers</div>
                {launchers.map((l) => (
                  <div
                    key={l.id}
                    className="term-popover-item"
                    onClick={() => {
                      handleNewTerminal(l.id)
                      setAddMenuOpen(false)
                    }}
                  >
                    <AgentMark agent="shell" size={14} />
                    <span className="term-popover-name">@{l.name}</span>
                  </div>
                ))}
              </>
            )}

          </div>,
          document.body
        )}
    </div>
  )

  const currentSplitObj = SPLIT_MODES.find((m) => m.id === effectiveSplitMode) || SPLIT_MODES[0]
  const CurrentSplitIcon = currentSplitObj.icon

  const renderSplitDropdown = (): JSX.Element | null => {
    if (!splitMenuOpen || !splitMenuPos) return null
    return createPortal(
      <div
        ref={splitMenuRef}
        className="term-popover-portal term-split-popover"
        style={{
          position: 'fixed',
          top: splitMenuPos.top,
          left: splitMenuPos.left,
          zIndex: 99999
        }}
      >
        <div className="term-popover-header">Terminal Split Layout</div>
        {SPLIT_MODES.map((m) => {
          const ModeIcon = m.icon
          const isActive = splitMode === m.id
          return (
            <div
              key={m.id}
              className={`term-popover-item ${isActive ? 'selected' : ''}`}
              onClick={() => {
                setSplitMode(m.id)
                setSplitMenuOpen(false)
              }}
            >
              <ModeIcon size={14} />
              <span className="term-popover-name">{m.title}</span>
              {isActive && <span className="term-popover-check">✓</span>}
            </div>
          )
        })}
      </div>,
      document.body
    )
  }

  const renderHandoffModal = (): JSX.Element | null => {
    if (!handoffOpen || !handoffPos) return null
    const activeSession = sessions.find((s) => s.id === activeSessionId)
    if (!activeSession) return null
    const otherSessions = sessions.filter((s) => s.id !== activeSessionId && !s.isExited)

    return createPortal(
      <div
        ref={handoffRef}
        className="term-popover-portal term-handoff-popover"
        style={{
          position: 'fixed',
          top: handoffPos.top,
          left: handoffPos.left,
          zIndex: 99999
        }}
      >
        <div className="term-handoff-header">
          <div className="term-handoff-title-row">
            <IconHandoff size={14} className="term-handoff-icon" />
            <span className="term-handoff-title">Agent Handoff</span>
          </div>
          <p className="term-handoff-desc">
            Pass terminal context from <strong>{activeSession.title}</strong> to another agent.
          </p>
        </div>

        {/* Target Session Selection */}
        <div className="term-handoff-section">
          <div className="term-handoff-section-title">
            {otherSessions.length > 0 ? 'Send to Running Agent:' : 'Target Agent:'}
          </div>
          <div className="term-handoff-targets">
            {otherSessions.map((s) => {
              const isSelected = selectedTargetId === s.id && !selectedNewAgent
              return (
                <div
                  key={s.id}
                  className={`term-handoff-card ${isSelected ? 'selected' : ''}`}
                  onClick={() => {
                    setSelectedTargetId(s.id)
                    setSelectedNewAgent(null)
                  }}
                >
                  <AgentMark agent={s.launcherKey as any} size={15} />
                  <div className="term-handoff-card-info">
                    <span className="term-handoff-card-title">{s.title}</span>
                    <span className="term-handoff-card-status">Active Session</span>
                  </div>
                  <button
                    type="button"
                    className="term-handoff-quick-btn"
                    title="Quick dispatch with default note"
                    onClick={(e) => {
                      e.stopPropagation()
                      handleHandoff(activeSession.id, { type: 'existing', id: s.id }, handoffNote)
                    }}
                  >
                    Quick Send ➔
                  </button>
                </div>
              )
            })}

            {/* Launch new agent options */}
            <div className="term-handoff-new-row">
              <span className="term-handoff-new-label">
                {otherSessions.length > 0 ? 'Or new:' : 'Launch agent to receive:'}
              </span>
              {(['claude', 'antigravity', 'codex'] as const)
                .filter((k) => isCliEnabled(k))
                .map((agentKey) => {
                const isSelected = selectedNewAgent === agentKey
                const name =
                  agentKey === 'claude'
                    ? 'Claude'
                    : agentKey === 'antigravity'
                    ? 'Antigravity'
                    : 'Codex'
                return (
                  <button
                    key={agentKey}
                    type="button"
                    className={`term-handoff-chip ${isSelected ? 'selected' : ''}`}
                    onClick={() => {
                      setSelectedNewAgent(agentKey)
                      setSelectedTargetId(null)
                    }}
                  >
                    <AgentMark agent={agentKey} size={12} />
                    <span>+ {name}</span>
                  </button>
                )
              })}
            </div>
          </div>
        </div>

        {/* Custom instruction note */}
        <div className="term-handoff-section">
          <div className="term-handoff-section-title">Instructions / Follow-up Note:</div>
          <textarea
            className="term-handoff-textarea"
            rows={2}
            value={handoffNote}
            onChange={(e) => setHandoffNote(e.target.value)}
            placeholder="e.g. Please run typecheck and test the UI changes..."
          />
        </div>

        {/* Footer */}
        <div className="term-handoff-footer">
          <span className="term-handoff-meta">
            📄 Last 35 lines of output attached
          </span>
          <div className="term-handoff-actions">
            <button
              type="button"
              className="term-handoff-cancel-btn"
              onClick={() => setHandoffOpen(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="term-handoff-submit-btn"
              disabled={!selectedTargetId && !selectedNewAgent}
              onClick={() => {
                if (selectedTargetId) {
                  handleHandoff(activeSession.id, { type: 'existing', id: selectedTargetId }, handoffNote)
                } else if (selectedNewAgent) {
                  handleHandoff(activeSession.id, { type: 'new', key: selectedNewAgent }, handoffNote)
                }
              }}
            >
              Dispatch Handoff ➔
            </button>
          </div>
        </div>
      </div>,
      document.body
    )
  }

  const getLaunchpadDragProps = (agentKey: string) => {
    const isHovered = hoveredLaunchpadAgent === agentKey
    const dragged = getDraggedSession()
    const isHandoff = isHovered && dragged && dragged.agent !== agentKey
    const isOpen = isHovered && dragged && dragged.agent === agentKey

    return {
      classNameExtra: `${isHandoff ? 'drag-handoff' : ''} ${isOpen ? 'drag-open' : ''}`,
      actionLabel: isHandoff ? 'Drop to Handoff ➔' : isOpen ? 'Drop to Open ➔' : undefined,
      handlers: {
        onDragEnter: (e: React.DragEvent) => {
          if (!e.dataTransfer.types.includes('application/x-agent-session')) return
          setHoveredLaunchpadAgent(agentKey)
        },
        onDragLeave: () => {
          if (hoveredLaunchpadAgent === agentKey) setHoveredLaunchpadAgent(null)
        },
        onDragOver: (e: React.DragEvent) => {
          if (!e.dataTransfer.types.includes('application/x-agent-session')) return
          e.preventDefault()
          e.stopPropagation()
          e.dataTransfer.dropEffect = 'copy'
        },
        onDrop: (e: React.DragEvent) => {
          if (!e.dataTransfer.types.includes('application/x-agent-session')) return
          e.preventDefault()
          e.stopPropagation()
          setHoveredLaunchpadAgent(null)
          setDragOverInfo(null)
          const sData = getSessionFromDrag(e)
          if (!sData) return
          if (sData.agent !== agentKey) {
            handleLaunchAndHandoff(sData, agentKey)
          } else {
            openOrResumeSession(sData)
          }
        }
      }
    }
  }

  return (
    <div className="term-root apple-theme">
      {/* Launchboard-style Unified Terminal Strip (Single Unified Row) */}
      <div className="term-unified-strip">
        <div className="term-strip-left">
          <span className="term-idle-title">Terminals</span>
          {renderAddMenu()}
          {renderAgentDispatch()}
        </div>

        {/* Action Controls: Popout, Clear, Delete, Handoff, Split */}
        <div className="term-strip-actions">
          {/* Pop out to independent window */}
          <button
            className="term-btn-icon"
            onClick={async () => {
              if (isDetached) {
                await window.api.window?.attachTerminal()
              } else {
                await window.api.window?.detachTerminal()
              }
            }}
            title={
              isDetached
                ? 'Attach terminals back to main window'
                : 'Pop out terminals into independent window'
            }
          >
            {isDetached ? <IconAttach size={13} /> : <IconPopout size={13} />}
          </button>

          {/* Clear terminal buffer (Ctrl+L) */}
          {sessions.length > 0 && activeSessionId && (
            <button
              className="term-btn-icon"
              onClick={() => {
                const s = sessions.find((x) => x.id === activeSessionId)
                if (s) {
                  s.term.clear()
                  sendToSession(activeSessionId, '\x0c')
                }
              }}
              title="Clear terminal buffer (Ctrl+L)"
            >
              <IconClear size={13} />
            </button>
          )}

          {/* Delete active session button (Trash) */}
          {activeSession && (
            <button
              className="term-btn-icon term-btn-delete"
              onClick={(e) => closeTerminal(activeSession.id, e)}
              title={`Delete / Kill session (${activeSession.title})`}
            >
              <IconTrash size={13} />
            </button>
          )}

          {/* Agent Handoff Button (Available when an active session exists) */}
          {activeSessionId && sessions.length > 0 && (
            <button
              className={`term-btn-action ${handoffOpen ? 'active' : ''}`}
              onClick={toggleHandoff}
              title="Hand off output & context to another agent"
            >
              <IconHandoff size={12} />
              <span>Handoff</span>
            </button>
          )}

          {/* Split Mode Dropdown Button (Floating popover menu) */}
          {sessions.length > 1 && (
            <button
              className={`term-btn-action ${splitMenuOpen ? 'active' : ''}`}
              onClick={toggleSplitMenu}
              title={`Layout: ${currentSplitObj.title}`}
            >
              <CurrentSplitIcon size={12} />
              <span>{currentSplitObj.label}</span>
              <IconChevronDown size={9} />
            </button>
          )}

          {pendingCount > 0 && (
            <div className="term-pending-bubble" title="A terminal needs your authorization">
              <span className="term-pending-dot" />
              <span>{pendingCount} Awaiting Approval</span>
            </div>
          )}
        </div>
      </div>

      {/* Row 2: Dedicated Session Tabs Bar (Displayed when one or more sessions exist) */}
      {sessions.length > 0 && (
        <div
          className="term-tabs-row"
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes('application/x-agent-session')) return
            e.preventDefault()
            e.dataTransfer.dropEffect = 'copy'
          }}
          onDrop={(e) => {
            if (!e.dataTransfer.types.includes('application/x-agent-session')) return
            e.preventDefault()
            const sData = getSessionFromDrag(e)
            if (sData) openOrResumeSession(sData)
          }}
        >
          <div className="term-strip-tabs">
            {sessions.map((s) => {
              const isHovered = hoveredTabId === s.id
              const dragged = getDraggedSession()
              const isHandoffTab = isHovered && dragged && dragged.agent !== s.launcherKey
              const isOpenTab = isHovered && dragged && dragged.agent === s.launcherKey

              return (
                <div
                  key={s.id}
                  className={`term-unified-tab ${activeSessionId === s.id ? 'active' : ''} ${
                    s.isExited ? 'exited' : ''
                  } ${paneCount > 1 && visibleIds.includes(s.id) ? 'shown' : ''} ${
                    isHandoffTab ? 'drag-handoff-target' : ''
                  } ${isOpenTab ? 'drag-open-target' : ''}`}
                  onClick={() => selectSession(s.id)}
                  onDragEnter={(e) => {
                    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
                    setHoveredTabId(s.id)
                  }}
                  onDragLeave={() => {
                    if (hoveredTabId === s.id) setHoveredTabId(null)
                  }}
                  onDragOver={(e) => {
                    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
                    e.preventDefault()
                    e.stopPropagation()
                    e.dataTransfer.dropEffect = 'copy'
                  }}
                  onDrop={(e) => {
                    if (!e.dataTransfer.types.includes('application/x-agent-session')) return
                    e.preventDefault()
                    e.stopPropagation()
                    setHoveredTabId(null)
                    setDragOverInfo(null)
                    const sData = getSessionFromDrag(e)
                    if (!sData) return
                    if (sData.agent !== s.launcherKey) {
                      handleHandoffFromSession(sData, s)
                    } else {
                      selectSession(s.id)
                    }
                  }}
                  title={
                    isHandoffTab
                      ? `Drop to Handoff task to ${s.title}`
                      : s.needsApproval
                      ? 'Waiting for your approval'
                      : s.title
                  }
                >
                  {s.needsApproval ? (
                    <span className="term-tab-badge-pulse" />
                  ) : (
                    <AgentMark agent={s.launcherKey as any} size={13} />
                  )}
                  <span className="term-tab-title">{s.title}</span>
                  {isHandoffTab && <span className="term-tab-handoff-tag">⇄ Handoff</span>}
                  <button
                    className="term-tab-close"
                    onClick={(e) => closeTerminal(s.id, e)}
                    title="Close / Delete session"
                  >
                    <IconClose size={10} />
                  </button>
                </div>
              )
            })}
          </div>
        </div>
      )}

      {/* Terminal Viewport / Launchpad Empty State */}
      <div
        ref={containerRef}
        className={`term-stage split-${sessions.length === 0 ? 'single' : effectiveSplitMode}`}
        onDragEnter={handleStageDragEnter}
        onDragLeave={handleStageDragLeave}
        onDragOver={handleStageDragOver}
        onDrop={handleStageDrop}
      >
        {dragOverInfo && (
          <div className={`term-drag-overlay mode-${dragOverInfo.mode}`}>
            <div className={`term-drag-pill mode-${dragOverInfo.mode}`}>
              {dragOverInfo.mode === 'handoff' ? (
                <>
                  <div className="term-drag-pill-badge handoff">
                    <IconHandoff size={16} />
                    <span>HANDOFF</span>
                  </div>
                  <div className="term-drag-pill-body">
                    <div className="term-drag-pill-title">
                      Drop to Handoff to <strong>{dragOverInfo.targetName}</strong>
                    </div>
                    <div className="term-drag-pill-desc">
                      Delegate task from <code>@{dragOverInfo.fromAgent}</code>: &ldquo;{dragOverInfo.sessionTitle}&rdquo;
                    </div>
                  </div>
                </>
              ) : (
                <>
                  <div className="term-drag-pill-badge open">
                    <AgentMark agent={dragOverInfo.fromAgent as any} size={16} />
                    <span>OPEN</span>
                  </div>
                  <div className="term-drag-pill-body">
                    <div className="term-drag-pill-title">
                      Drop to Open in Terminal
                    </div>
                    <div className="term-drag-pill-desc">
                      Resume <code>@{dragOverInfo.fromAgent}</code>: &ldquo;{dragOverInfo.sessionTitle}&rdquo;
                    </div>
                  </div>
                </>
              )}
            </div>
          </div>
        )}

        {sessions.length === 0 && (
          <div className="term-launchpad">
            <div className="term-launchpad-glow" />
            <div className="term-launchpad-content">
              <h2 className="term-launchpad-title">New Terminal Session</h2>
              <p className="term-launchpad-desc">
                Launch an interactive Claude, Antigravity, or Codex agent session, or a native PowerShell / CMD terminal.
              </p>

              <div className="term-launchpad-cards">
                {isCliEnabled('claude') && (() => {
                  const lp = getLaunchpadDragProps('claude')
                  return (
                    <div
                      className={`launchpad-card card-claude ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('claude')}
                      {...lp.handlers}
                    >
                      {isBypassActive && (
                        <span className="launchpad-bypass-badge" title="Bypass mode: --permission-mode bypassPermissions">
                          Bypass
                        </span>
                      )}
                      <div className="launchpad-card-icon">
                        <AgentMark agent="claude" size={28} />
                      </div>
                      <div className="launchpad-card-name">@claude</div>
                      <div className="launchpad-card-sub">Claude Code</div>
                      <div className="launchpad-card-meta"><code>claude</code> · Anthropic</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Session →'}</div>
                    </div>
                  )
                })()}

                {isCliEnabled('antigravity') && (() => {
                  const lp = getLaunchpadDragProps('antigravity')
                  return (
                    <div
                      className={`launchpad-card card-antigravity ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('antigravity')}
                      {...lp.handlers}
                    >
                      {isBypassActive && (
                        <span className="launchpad-bypass-badge" title="Bypass mode: --dangerously-skip-permissions">
                          Bypass
                        </span>
                      )}
                      <div className="launchpad-card-icon">
                        <AgentMark agent="antigravity" size={28} />
                      </div>
                      <div className="launchpad-card-name">@antigravity</div>
                      <div className="launchpad-card-sub">Antigravity</div>
                      <div className="launchpad-card-meta"><code>agy</code> · DeepMind</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Session →'}</div>
                    </div>
                  )
                })()}

                {isCliEnabled('codex') && (() => {
                  const lp = getLaunchpadDragProps('codex')
                  return (
                    <div
                      className={`launchpad-card card-codex ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('codex')}
                      {...lp.handlers}
                    >
                      {isBypassActive && (
                        <span className="launchpad-bypass-badge" title="Bypass mode: --dangerously-bypass-approvals-and-sandbox">
                          Bypass
                        </span>
                      )}
                      <div className="launchpad-card-icon">
                        <AgentMark agent="codex" size={28} />
                      </div>
                      <div className="launchpad-card-name">@codex</div>
                      <div className="launchpad-card-sub">Codex CLI</div>
                      <div className="launchpad-card-meta"><code>codex</code> · OpenAI</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Session →'}</div>
                    </div>
                  )
                })()}

                {isCliEnabled(CUSTOM_AGENT_ID) && <button type="button" className="launchpad-card card-shell"
                  onClick={() => handleNewTerminal(CUSTOM_AGENT_ID)}>
                  <div className="launchpad-card-icon"><AgentMark agent="custom-agent" size={28} /></div>
                  <div className="launchpad-card-name">{settings.customAgent?.name}</div>
                  <div className="launchpad-card-sub">{t('customAgent.title')}</div>
                  <div className="launchpad-card-meta"><code>{settings.customAgent?.command}</code></div>
                  <div className="launchpad-card-action">{t('customAgent.launch')}</div>
                </button>}

                {isCliEnabled('powershell') && (() => {
                  const lp = getLaunchpadDragProps('powershell')
                  return (
                    <div
                      className={`launchpad-card card-powershell ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('powershell')}
                      {...lp.handlers}
                    >
                      <div className="launchpad-card-icon">
                        <AgentMark agent="shell" size={28} />
                      </div>
                      <div className="launchpad-card-name">PowerShell</div>
                      <div className="launchpad-card-sub">Windows PowerShell</div>
                      <div className="launchpad-card-meta"><code>pwsh</code> · direct pty</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Shell →'}</div>
                    </div>
                  )
                })()}

                {isCliEnabled('cmd') && (() => {
                  const lp = getLaunchpadDragProps('cmd')
                  return (
                    <div
                      className={`launchpad-card card-cmd ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('cmd')}
                      {...lp.handlers}
                    >
                      <div className="launchpad-card-icon">
                        <AgentMark agent="shell" size={28} />
                      </div>
                      <div className="launchpad-card-name">Command Prompt</div>
                      <div className="launchpad-card-sub">Windows CMD</div>
                      <div className="launchpad-card-meta"><code>cmd</code> · direct pty</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Shell →'}</div>
                    </div>
                  )
                })()}

                <button type="button" className="launchpad-card card-cowork" onClick={openCowork} title={t('cowork.buttonTitle')}>
                  <div className="launchpad-card-icon"><IconCowork size={28} /></div>
                  <div className="launchpad-card-name">{t('cowork.tab')}</div>
                  <div className="launchpad-card-sub">{t('cowork.newMeeting')}</div>
                  <div className="launchpad-card-meta" title="Claude · Codex · Antigravity">
                    <AgentMark agent="claude" size={14} />
                    <AgentMark agent="codex" size={14} />
                    <AgentMark agent="antigravity" size={14} />
                  </div>
                  <div className="launchpad-card-action">{t('cowork.open')} →</div>
                </button>

                {!isWindows && isCliEnabled('bash') && (() => {
                  const lp = getLaunchpadDragProps('bash')
                  return (
                    <div
                      className={`launchpad-card card-shell ${lp.classNameExtra}`}
                      onClick={() => handleNewTerminal('bash')}
                      {...lp.handlers}
                    >
                      <div className="launchpad-card-icon">
                        <AgentMark agent="shell" size={28} />
                      </div>
                      <div className="launchpad-card-name">bash</div>
                      <div className="launchpad-card-sub">Unix Bash Shell</div>
                      <div className="launchpad-card-meta"><code>bash</code> · direct pty</div>
                      <div className="launchpad-card-action">{lp.actionLabel || 'Launch Shell →'}</div>
                    </div>
                  )
                })()}

                {enabledAgents.length === 0 && enabledShells.length === 0 && (
                  <div className="term-launchpad-empty-notice">
                    <p>All built-in CLIs are currently disabled in Settings.</p>
                    <span>Open Settings (⚙) → CLI & Agents to re-enable them.</span>
                  </div>
                )}
              </div>
            </div>
          </div>
        )}
        {sessions.map((s) => (
          <TerminalInstance
            key={s.id}
            session={s}
            isVisible={visibleIds.includes(s.id)}
            isActive={s.id === activeSessionId}
            multi={paneCount > 1}
            onFocusPane={() => selectSession(s.id)}
            onContextMenu={(x, y) => setTermContextMenu({ x, y, sessionId: s.id })}
            launcherKey={s.launcherKey}
          />
        ))}
      </div>
      {renderSplitDropdown()}
      {renderHandoffModal()}

      {/* Terminal Right-Click Context Menu */}
      {termContextMenu && (() => {
        const targetSession = sessions.find((s) => s.id === termContextMenu.sessionId)
        if (!targetSession) return null
        const hasSel = targetSession.term.hasSelection()
        return (
          <>
            <div
              className="dash-context-menu-backdrop"
              onClick={() => setTermContextMenu(null)}
              onContextMenu={(e) => {
                e.preventDefault()
                setTermContextMenu(null)
              }}
            />
            <div
              className="dash-context-menu"
              style={{
                top: Math.min(termContextMenu.y, window.innerHeight - 170),
                left: Math.min(termContextMenu.x, window.innerWidth - 180)
              }}
              onClick={(e) => e.stopPropagation()}
            >
              <button
                type="button"
                className="dash-context-item"
                disabled={!hasSel}
                style={{ opacity: hasSel ? 1 : 0.45, cursor: hasSel ? 'pointer' : 'default' }}
                onClick={() => {
                  setTermContextMenu(null)
                  const sel = targetSession.term.getSelection()
                  if (sel) {
                    navigator.clipboard.writeText(sel).catch(() => {})
                  }
                }}
              >
                <span>{t('terminal.copy')}</span>
                <span style={{ marginLeft: 'auto', fontSize: '10.5px', color: 'var(--fg-dim)', fontFamily: 'var(--mono)' }}>Ctrl+C</span>
              </button>
              <button
                type="button"
                className="dash-context-item"
                onClick={() => {
                  setTermContextMenu(null)
                  navigator.clipboard.readText().then((text) => {
                    if (text) targetSession.term.paste(text)
                  }).catch(() => {})
                }}
              >
                <span>{t('terminal.paste')}</span>
                <span style={{ marginLeft: 'auto', fontSize: '10.5px', color: 'var(--fg-dim)', fontFamily: 'var(--mono)' }}>Ctrl+V</span>
              </button>
              <div className="dash-context-divider" />
              <button
                type="button"
                className="dash-context-item"
                onClick={() => {
                  setTermContextMenu(null)
                  targetSession.term.selectAll()
                }}
              >
                <span>{t('terminal.selectAll')}</span>
              </button>
              <button
                type="button"
                className="dash-context-item"
                onClick={() => {
                  setTermContextMenu(null)
                  targetSession.term.clear()
                }}
              >
                <span>{t('terminal.clearBuffer')}</span>
              </button>
              <div className="dash-context-divider" />
              <button
                type="button"
                className="dash-context-item is-destructive"
                onClick={(e) => {
                  setTermContextMenu(null)
                  closeTerminal(targetSession.id, e)
                }}
              >
                <span>{t('terminal.killSession')}</span>
              </button>
            </div>
          </>
        )
      })()}
    </div>
  )
}

function TerminalInstance({
  session,
  isVisible,
  isActive,
  multi,
  onFocusPane,
  onContextMenu,
  launcherKey
}: {
  session: TerminalSession
  /** 是否被排進目前的分割版面 */
  isVisible: boolean
  /** 是否為作用中面板（多面板時畫外框、決定 Send to 的來源） */
  isActive: boolean
  multi: boolean
  onFocusPane: () => void
  onContextMenu: (x: number, y: number) => void
  launcherKey: string
}): JSX.Element {
  const elRef = useRef<HTMLDivElement>(null)
  const mounted = useRef(false)

  useEffect(() => {
    if (!elRef.current) return
    mounted.current = true
    const ts = session.ts

    // remount 時 term 要重新掛進新的 DOM 節點，但底下的 spawn 只能做一次（ts.started 記在 session 上）。
    ts.attach(elRef.current)
    if (!ts.started) {
      // Antigravity CLI 沒有自己的「Resuming...」提示，resume 後畫面長得跟全新 session
      // 幾乎一樣（只差在最後停在舊對話的最後一句話），使用者很容易誤以為沒接上舊紀錄。
      // 這裡純粹在本地終端畫面插入一行提示，不會送進 pty，不影響 CLI 本身的輸入輸出。
      if (session.launcherKey === 'antigravity' && session.args?.[0] === '--conversation') {
        session.term.writeln('\x1b[2m▸ Resuming Antigravity session...\x1b[0m')
        session.term.writeln('')
      }

      // 手機遠端開的終端已經在跑：接上去；其餘照窗格量到的尺寸 spawn
      ts.start(
        session.attachPtyId
          ? { ptyId: session.attachPtyId }
          : {
              spawn: {
                args: session.args,
                cwd: session.cwd,
                sessionId: session.associatedSessionId,
                title: session.title,
                ...(DIRECT_IDS.includes(launcherKey) ? { command: launcherKey } : { launcherId: launcherKey })
              }
            }
      )
    }
    return () => ts.detach()
  }, [])

  // 從隱藏變回顯示時（display:none 期間尺寸為 0），重新 fit 一次
  useEffect(() => {
    if (!isVisible || !mounted.current) return
    const timer = setTimeout(() => {
      if (isComposing(session.term)) return
      try {
        session.ts.fit()
        if (isActive) focusTerm(session.term)
      } catch {
        /* 尺寸尚未穩定時忽略 */
      }
    }, 0)
    return () => clearTimeout(timer)
  }, [isVisible, isActive])

  return (
    <div
      ref={elRef}
      className={`term-surface ${isVisible ? 'on' : ''} ${multi && isActive ? 'pane-active' : ''}`}
      onMouseDown={() => {
        if (multi) onFocusPane()
        focusTerm(session.term)
      }}
      onAuxClick={(e) => {
        if (e.button === 1) {
          e.preventDefault()
          e.stopPropagation()
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
        onContextMenu(e.clientX, e.clientY)
      }}
    />
  )
}
