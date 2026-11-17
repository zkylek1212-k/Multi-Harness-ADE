import { useEffect, useState, useCallback, useMemo } from 'react'
import type {
  DashboardData,
  AgentId,
  AgentSessionInfo,
  AgentUsageSummary
} from '../../../../preload/index'
import AgentMark from '@/components/AgentMark'
import { IconArchive, IconTrash, IconTerminalBox, IconFolder, IconGripVertical, IconWindowNew, IconGitClone, IconEyeOff, IconFolderMinus, IconShieldCheck, IconPin } from '@/components/Icons'
import AppleAlertDialog from '@/components/AppleAlertDialog'
import { openTerminalSession, setDraggedSession, useWorkbench, switchWorkspace, setSidebarTab, openSettings } from '@/store'
import { useTranslation } from '@/i18n'
import UsageRangeSwitch, { formatTokens } from '@/components/UsageRangeSwitch'
import './dashboard.css'

interface SessionFolderGroup {
  key: string
  name: string
  path?: string
  isCurrentWorkspace: boolean
  sessions: AgentSessionInfo[]
  totalTokens: number
  activeCount: number
}

const AGENT_CONFIG: Record<
  AgentId,
  { name: string; color: string }
> = {
  claude: {
    name: 'Claude Code',
    color: '#D97706'
  },
  antigravity: {
    name: 'Antigravity',
    color: '#7C3AED'
  },
  codex: {
    name: 'Codex',
    color: '#10A37F'
  }
}


function normalizePath(p?: string): string {
  if (!p) return ''
  return p.replace(/\\+/g, '/').replace(/\/+/g, '/').toLowerCase().replace(/\/+$/, '')
}

export default function DashboardPanel({
  sessionsOnly = false
}: {
  /** Vibe 模式懸浮面板：只顯示 Session 清單，不顯示用量卡（用量改在終端上方狀態列） */
  sessionsOnly?: boolean
} = {}): JSX.Element {
  const [data, setData] = useState<DashboardData | null>(null)
  const [loading, setLoading] = useState(true)
  const [viewFilter, setViewFilter] = useState<'all' | 'archived'>('all')
  const [selectedAgent, setSelectedAgent] = useState<AgentId | 'all'>('all')
  const [expandedSessionId, setExpandedSessionId] = useState<string | null>(null)
  const [lastRefreshed, setLastRefreshed] = useState<string>('')
  const [sessionToDelete, setSessionToDelete] = useState<AgentSessionInfo | null>(null)
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(new Set())
  const [dragOverFolderKey, setDragOverFolderKey] = useState<string | null>(null)
  const [folderOrder, setFolderOrder] = useState<string[]>(() => {
    try {
      const saved = localStorage.getItem('agent-workbench:dashboard-folder-order')
      return saved ? JSON.parse(saved) : []
    } catch {
      return []
    }
  })
  // Pinned folders stay above the rest; within each of the two tiers the usual order applies
  const [pinnedFolders, setPinnedFolders] = useState<string[]>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem('agent-workbench:dashboard-folder-pins') || '[]')
      return Array.isArray(saved) ? saved.filter((k) => typeof k === 'string') : []
    } catch {
      return []
    }
  })
  const pinnedSet = useMemo(() => new Set(pinnedFolders), [pinnedFolders])
  const toggleFolderPin = useCallback((key: string) => {
    setPinnedFolders((prev) => {
      const next = prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]
      try {
        localStorage.setItem('agent-workbench:dashboard-folder-pins', JSON.stringify(next))
      } catch {}
      return next
    })
  }, [])
  const [draggedFolderKey, setDraggedFolderKey] = useState<string | null>(null)
  const [dragOverFolderTarget, setDragOverFolderTarget] = useState<{
    key: string
    pos: 'top' | 'bottom'
  } | null>(null)
  const [folderContextMenu, setFolderContextMenu] = useState<{
    key: string
    name: string
    path?: string
    isCurrentWorkspace?: boolean
    sessions: AgentSessionInfo[]
    x: number
    y: number
  } | null>(null)
  const [sessionContextMenu, setSessionContextMenu] = useState<{
    session: AgentSessionInfo
    groupKey: string
    x: number
    y: number
  } | null>(null)
  const [folderToDelete, setFolderToDelete] = useState<{
    key: string
    name: string
    path?: string
    sessions: AgentSessionInfo[]
  } | null>(null)

  // Clone Repository Modal 狀態
  const [showCloneModal, setShowCloneModal] = useState(false)
  const [cloneUrl, setCloneUrl] = useState('')
  const [cloneTargetDir, setCloneTargetDir] = useState('')
  const [cloneLoading, setCloneLoading] = useState(false)
  const [cloneError, setCloneError] = useState<string | null>(null)

  const inferRepoName = (url: string): string => {
    const clean = url.trim().replace(/\.git$/i, '').replace(/\/+$/, '')
    const parts = clean.split(/[/:\\\\]/).filter(Boolean)
    return parts.pop() || ''
  }

  const handleOpenFolder = async (): Promise<void> => {
    if (window.api?.files?.pickWorkspace) {
      const selected = await window.api.files.pickWorkspace()
      if (selected) {
        if (window.api?.dashboard?.unmarkWorkspace) {
          await window.api.dashboard.unmarkWorkspace(selected)
        }
        await switchWorkspace(selected)
        setSidebarTab('files')
        await loadData(true, true)
      }
    }
  }

  const handleOpenCloneModal = (): void => {
    setCloneUrl('')
    setCloneError(null)
    setCloneLoading(false)
    if (workspaceRoot) {
      const parentDir = workspaceRoot.replace(/[/\\][^/\\]+$/, '')
      setCloneTargetDir(parentDir)
    } else {
      setCloneTargetDir('')
    }
    setShowCloneModal(true)
  }

  const handleBrowseCloneTarget = async (): Promise<void> => {
    if (window.api?.files?.pickWorkspace) {
      const selected = await window.api.files.pickWorkspace()
      if (selected) {
        const repoName = inferRepoName(cloneUrl)
        setCloneTargetDir(repoName ? `${selected}\\${repoName}` : selected)
      }
    }
  }

  const handleExecuteClone = async (): Promise<void> => {
    if (!cloneUrl.trim()) {
      setCloneError(t('dashboard.repoUrlLabel') + ' is required')
      return
    }
    if (!cloneTargetDir.trim()) {
      setCloneError(t('dashboard.targetDirLabel') + ' is required')
      return
    }
    if (!window.api?.git?.clone) {
      setCloneError('Git clone API is not available')
      return
    }

    setCloneLoading(true)
    setCloneError(null)

    try {
      const res = await window.api.git.clone(cloneUrl.trim(), cloneTargetDir.trim())
      if (res.success && res.targetDir) {
        setShowCloneModal(false)
        if (window.api?.dashboard?.unmarkWorkspace) {
          await window.api.dashboard.unmarkWorkspace(res.targetDir)
        }
        await switchWorkspace(res.targetDir)
        setSidebarTab('files')
        await loadData(true, true)
      } else {
        setCloneError(res.error || t('dashboard.cloneFailed'))
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err)
      setCloneError(msg)
    } finally {
      setCloneLoading(false)
    }
  }

  // 自訂會話排序（各分組 key 對應之 session ID 陣列）與自訂分組覆寫，支援本地持久化
  const [customOrder, setCustomOrder] = useState<Record<string, string[]>>(() => {
    try {
      const saved = localStorage.getItem('agent-workbench:dashboard-order')
      return saved ? JSON.parse(saved) : {}
    } catch {
      return {}
    }
  })
  const [folderOverrides, setFolderOverrides] = useState<Record<string, string>>(() => {
    try {
      const saved = localStorage.getItem('agent-workbench:dashboard-folder-overrides')
      return saved ? JSON.parse(saved) : {}
    } catch {
      return {}
    }
  })

  const { workspaceRoot, settingsTick, liveAgentSessionIds, closedAgentSessions, usageRange } = useWorkbench()
  const { t } = useTranslation()
  const [cliEnabled, setCliEnabled] = useState<Record<string, boolean | undefined>>({
    claude: true,
    antigravity: true,
    codex: true
  })

  // 關閉右鍵選單（避免點擊選單內部時因 mousedown 搶先觸發而吞掉按鈕 click）
  useEffect(() => {
    if (!folderContextMenu && !sessionContextMenu) return
    const onMouseDown = (e: MouseEvent): void => {
      const target = e.target as HTMLElement | null
      if (target && target.closest('.dash-context-menu')) {
        return
      }
      setFolderContextMenu(null)
      setSessionContextMenu(null)
    }
    const onScroll = (): void => {
      setFolderContextMenu(null)
      setSessionContextMenu(null)
    }

    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [folderContextMenu, sessionContextMenu])

  // 右鍵選單動作：切換至此專案
  const handleContextSwitchFolder = async (folder: typeof folderContextMenu): Promise<void> => {
    if (!folder) return
    const targetPath = folder.path || (folder.isCurrentWorkspace ? workspaceRoot : undefined)
    setFolderContextMenu(null)
    if (targetPath) {
      if (window.api?.dashboard?.unmarkWorkspace) {
        await window.api.dashboard.unmarkWorkspace(targetPath)
      }
      await switchWorkspace(targetPath)
      await loadData(true, true)
    } else {
      setSidebarTab('files')
    }
  }

  // 右鍵選單動作：在新視窗開啟
  const handleContextOpenNewWindow = async (folder: typeof folderContextMenu): Promise<void> => {
    if (!folder) return
    const targetPath = folder.path || (folder.isCurrentWorkspace ? workspaceRoot : undefined)
    setFolderContextMenu(null)
    if (targetPath && window.api?.window?.openProjectWindow) {
      await window.api.window.openProjectWindow(targetPath)
    }
  }

  // 右鍵選單動作：封存／解除封存專案
  const handleContextArchiveFolder = async (folder: typeof folderContextMenu): Promise<void> => {
    if (!folder) return
    const isArchiving = viewFilter !== 'archived'
    const targetWs = folder.path || (folder.isCurrentWorkspace ? workspaceRoot : undefined) || folder.name || folder.key
    const sessionIds = folder.sessions.map((s) => s.id)
    setFolderContextMenu(null)

    if (targetWs && window.api?.dashboard?.archiveWorkspace) {
      await window.api.dashboard.archiveWorkspace(targetWs, isArchiving, sessionIds)
    } else if (sessionIds.length > 0 && window.api?.dashboard?.archiveSessions) {
      await window.api.dashboard.archiveSessions(sessionIds, isArchiving)
    }
    await loadData(true, true)
  }

  // 右鍵選單動作：從清單中隱藏此專案
  const handleContextHideFolderPrompt = (folder: typeof folderContextMenu): void => {
    if (!folder) return
    const targetWs = folder.path || (folder.isCurrentWorkspace ? workspaceRoot : undefined) || folder.name || folder.key
    const normalizedTarget = {
      ...folder,
      path: folder.path || (folder.isCurrentWorkspace ? workspaceRoot : undefined),
      key: folder.key || targetWs
    }
    setFolderContextMenu(null)
    setFolderToDelete(normalizedTarget)
  }


  const loadData = useCallback(async (silent = false, force = false) => {
    if (!silent) setLoading(true)
    try {
      const res = await window.api.dashboard.data(force)
      setData(res)
      setLastRefreshed(new Date().toLocaleTimeString())
    } catch (e) {
      console.error('Failed to load dashboard data:', e)
    } finally {
      if (!silent) setLoading(false)
    }
  }, [])

  useEffect(() => {
    let active = true
    window.api.settings.get().then((s) => {
      if (!active) return
      if (s?.cliEnabled) {
        setCliEnabled(s.cliEnabled)
      }
      loadData(true)
    }).catch((e) => {
      console.error('Failed to load settings in dashboard:', e)
    })
    return () => {
      active = false
    }
  }, [settingsTick, loadData])

  useEffect(() => {
    loadData(true)
  }, [workspaceRoot, loadData])

  useEffect(() => {
    loadData()
    const interval = setInterval(() => loadData(true), 5000)
    return () => clearInterval(interval)
  }, [loadData])

  // 終端開／關 Agent 分頁時立刻重抓，不必等下一次輪詢或重開 App。
  // 新會話的 .jsonl 是 CLI 起來後才寫出來的，所以隔幾秒再補抓一次。
  useEffect(() => {
    loadData(true)
    const t = setTimeout(() => loadData(true), 3000)
    return () => clearTimeout(t)
  }, [liveAgentSessionIds, loadData])

  const isAgentEnabled = useCallback(
    (agentId: AgentId) => cliEnabled[agentId] !== false,
    [cliEnabled]
  )

  const allAgentIds: AgentId[] = ['claude', 'antigravity', 'codex']
  const enabledAgentIds = allAgentIds.filter(isAgentEnabled)

  useEffect(() => {
    if (selectedAgent !== 'all' && !isAgentEnabled(selectedAgent)) {
      setSelectedAgent('all')
    }
  }, [selectedAgent, isAgentEnabled])

  const toggleExpand = (id: string): void => {
    setExpandedSessionId((prev) => (prev === id ? null : id))
  }

  const handleArchive = async (id: string, currentArchived: boolean, e?: React.MouseEvent): Promise<void> => {
    e?.stopPropagation()
    await window.api.dashboard.archiveSession(id, !currentArchived)
    await loadData(true, true)
  }

  const handleDeletePrompt = (session: AgentSessionInfo, e?: React.MouseEvent): void => {
    e?.stopPropagation()
    setSessionToDelete(session)
  }

  const handleConfirmDelete = async (): Promise<void> => {
    if (!sessionToDelete) return
    const id = sessionToDelete.id
    setSessionToDelete(null)
    await window.api.dashboard.deleteSession(id)
    await loadData(true, true)
  }

  const agentList: AgentUsageSummary[] = data
    ? Object.values(data.agents).filter((a) => isAgentEnabled(a.agent))
    : []

  // 依選定區間取用量（main 掃全部歷史後切 all/30d/7d/1d）
  const rangeUsage = (agentId: AgentId): { input: number; cacheRead: number; output: number } =>
    data?.usageByRange?.[agentId]?.[usageRange] ?? { input: 0, cacheRead: 0, output: 0 }
  const totalTokens: number = agentList.reduce((acc: number, a: AgentUsageSummary) => {
    const u = rangeUsage(a.agent)
    return acc + u.input + u.cacheRead + u.output
  }, 0)

  // 用 renderer 手上的事實修正 main 端的推斷（main 只能靠 PTY meta ＋ 日誌 mtime 猜）：
  //   1. 終端分頁還活著 → 一定是 active（修掉 resume 後瞬間跳回 completed）。
  //   2. 分頁是在這裡被關掉的、而且關掉之後檔案沒再被寫過 → 壓成 idle
  //      （修掉關掉後因為 mtime 還很新而卡在 active 好幾分鐘）。
  //      關閉後檔案又有寫入，代表 App 外面有人在跑它，就尊重 main 的判斷。
  const allSessions = (data?.sessions || [])
    .filter((s) => isAgentEnabled(s.agent))
    .map((s) => {
      if (liveAgentSessionIds.includes(s.id)) return { ...s, status: 'active' as const }
      const closedAt = closedAgentSessions[s.id]
      if (
        s.status === 'active' &&
        closedAt &&
        // 5 秒寬限：CLI 收工時常會再補寫最後一筆
        new Date(s.lastActiveTime).getTime() <= closedAt + 5000
      ) {
        return { ...s, status: 'idle' as const }
      }
      return s
    })
  const activeSessionsCount = allSessions.filter((s) => s.status === 'active').length

  const archivedSessions = allSessions.filter((s) => s.isArchived)
  const unarchivedSessions = allSessions.filter((s) => !s.isArchived)
  const baseSessions = viewFilter === 'archived' ? archivedSessions : unarchivedSessions
  const displayedSessions =
    selectedAgent === 'all'
      ? baseSessions
      : baseSessions.filter((s) => s.agent === selectedAgent)

  // 依執行資料夾歸類 Session（結合使用者在 Workbench 中開啟過之工作區與自訂分組/排序）
  const folderGroups = useMemo<SessionFolderGroup[]>(() => {
    const map = new Map<string, SessionFolderGroup>()
    const normRoot = workspaceRoot ? normalizePath(workspaceRoot) : ''

    const isWorkspaceDeleted = (targetPath?: string, targetName?: string, targetKey?: string): boolean => {
      const deletedList = data?.deletedWorkspaces || []
      if (deletedList.length === 0) return false
      const normP = targetPath ? normalizePath(targetPath) : ''
      const normN = targetName ? targetName.toLowerCase().trim() : ''
      const normK = targetKey ? normalizePath(targetKey) : ''
      for (const d of deletedList) {
        const normD = normalizePath(d)
        const baseD = (d.split(/[\\/]/).filter(Boolean).pop() || d).toLowerCase().trim()
        if (normP && (normP === normD || normP.endsWith('/' + normD) || normD.endsWith('/' + normP) || normP.endsWith('/' + baseD))) return true
        if (normN && (normN === baseD || normN === normD)) return true
        if (normK && (normK === normD || normK === baseD)) return true
      }
      return false
    }

    const isWorkspaceArchived = (targetPath?: string, targetName?: string, targetKey?: string): boolean => {
      const archivedList = data?.archivedWorkspaces || []
      if (archivedList.length === 0) return false
      const normP = targetPath ? normalizePath(targetPath) : ''
      const normN = targetName ? targetName.toLowerCase().trim() : ''
      const normK = targetKey ? normalizePath(targetKey) : ''
      for (const a of archivedList) {
        const normA = normalizePath(a)
        const baseA = (a.split(/[\\/]/).filter(Boolean).pop() || a).toLowerCase().trim()
        if (normP && (normP === normA || normP.endsWith('/' + normA) || normA.endsWith('/' + normP) || normP.endsWith('/' + baseA))) return true
        if (normN && (normN === baseA || normN === normA)) return true
        if (normK && (normK === normA || normK === baseA)) return true
      }
      return false
    }

    // 1. 先初始化使用者在 IDE 中開啟過之合法工作區（依目前 viewFilter 過濾正常或已封存）
    const userWorkspaces = data?.userWorkspaces || []
    for (const ws of userWorkspaces) {
      if (isWorkspaceDeleted(ws.path, ws.name, ws.path)) continue
      const isArch = isWorkspaceArchived(ws.path, ws.name, ws.path) || Boolean(ws.isArchived)
      if (viewFilter === 'archived' && !isArch) continue
      if (viewFilter === 'all' && isArch) continue

      const normWs = normalizePath(ws.path)
      const groupKey = normWs || ws.name.toLowerCase()
      // 只認本視窗自己的 workspaceRoot；多視窗共用同一份 main 資料，不能看 main 端的「目前」
      const isCurrentWs = Boolean(normRoot && normWs && normRoot === normWs)
      map.set(groupKey, {
        key: groupKey,
        name: ws.name, // 永遠保持該專案原始資料夾名稱，絕不被 workspaceRoot 覆蓋！
        path: ws.path,
        isCurrentWorkspace: isCurrentWs,
        sessions: [],
        totalTokens: 0,
        activeCount: 0
      })
    }

    // 確保當前開啟之工作區（workspaceRoot）即使剛切換尚未收到 data 或尚無會話紀錄，也必定出現在專案清單中
    // 但若使用者明確將其「隱藏 (Hide)」或「封存 (Archive)」，必須嚴格尊重使用者意圖，絕不暴力復活！
    if (workspaceRoot) {
      const normWRoot = normalizePath(workspaceRoot)
      const wsName = workspaceRoot.split(/[\\/]/).filter(Boolean).pop() || workspaceRoot
      const isDeleted = isWorkspaceDeleted(workspaceRoot, wsName, normWRoot)
      const isArchived = isWorkspaceArchived(workspaceRoot, wsName, normWRoot)

      const shouldShow = !isDeleted && (
        (viewFilter === 'all' && !isArchived) ||
        (viewFilter === 'archived' && isArchived)
      )

      if (shouldShow && !map.has(normWRoot)) {
        map.set(normWRoot, {
          key: normWRoot,
          name: wsName,
          path: workspaceRoot,
          isCurrentWorkspace: true,
          sessions: [],
          totalTokens: 0,
          activeCount: 0
        })
      }
    }

    // 2. 將匹配的 Session 加入對應的工作區分組中
    for (const session of displayedSessions) {
      const rawPath = session.workspacePath?.trim() || ''
      const normPath = rawPath ? normalizePath(rawPath) : ''
      const wsName = session.workspace?.trim() || (rawPath ? rawPath.split(/[\\/]/).filter(Boolean).pop() || '' : 'Other')
      const overrideKey = folderOverrides[session.id]
      const groupKey = overrideKey || normPath || wsName.toLowerCase()

      // 若該 session 所屬之工作區已被使用者隱藏，絕不在清單中逆向復活卡片
      if (isWorkspaceDeleted(rawPath, wsName, groupKey)) {
        continue
      }
      const isArch = isWorkspaceArchived(rawPath, wsName, groupKey) || Boolean(session.isArchived)
      // 若在全部檢視（all），且該工作區已封存，不顯示
      if (isArch && viewFilter === 'all') {
        continue
      }
      // 若在封存檢視（archived），且該工作區未封存且 session 本身未封存，不顯示
      if (!isArch && viewFilter === 'archived') {
        continue
      }

      let grp = map.get(groupKey)
      if (!grp && normPath) {
        grp = map.get(normPath)
      }
      if (!grp && wsName) {
        for (const existing of map.values()) {
          if (existing.name.toLowerCase() === wsName.toLowerCase()) {
            grp = existing
            break
          }
        }
      }

      if (!grp) {
        // 如果該 Session 的工作區不在 userWorkspaces 中（例如是獨立會話），建立其所屬專案分組
        const isCurrentWs = Boolean(
          normRoot && normPath && normRoot === normPath
        )
        grp = {
          key: groupKey,
          name: wsName,
          path: rawPath || (isCurrentWs ? workspaceRoot : undefined),
          isCurrentWorkspace: isCurrentWs,
          sessions: [],
          totalTokens: 0,
          activeCount: 0
        }
        map.set(groupKey, grp)
      }

      if (!grp.path && rawPath) {
        grp.path = rawPath
      }

      grp.sessions.push(session)
      grp.totalTokens += session.totalTokens || 0
      if (session.status === 'active') {
        grp.activeCount += 1
      }
    }

    // 依自訂手動排序對各群組內的 sessions 進行排定
    for (const grp of map.values()) {
      const order = customOrder[grp.key]
      if (order && order.length > 0) {
        grp.sessions.sort((a, b) => {
          const idxA = order.indexOf(a.id)
          const idxB = order.indexOf(b.id)
          if (idxA !== -1 && idxB !== -1) return idxA - idxB
          if (idxA !== -1) return -1
          if (idxB !== -1) return 1
          return (new Date(b.lastActiveTime).getTime() || 0) - (new Date(a.lastActiveTime).getTime() || 0)
        })
      }
    }

    const list = Array.from(map.values())
    // 排序：釘選的資料夾永遠在最上面；同一層內若有手動自訂資料夾順序則優先套用，其餘則當前工作區置頂，其次為含有活躍 Session 者，最後依各組中最新 session 排序
    list.sort((a, b) => {
      const pinA = pinnedSet.has(a.key)
      if (pinA !== pinnedSet.has(b.key)) return pinA ? -1 : 1
      if (folderOrder.length > 0) {
        const idxA = folderOrder.indexOf(a.key)
        const idxB = folderOrder.indexOf(b.key)
        if (idxA !== -1 && idxB !== -1) return idxA - idxB
        if (idxA !== -1) return -1
        if (idxB !== -1) return 1
      }
      if (a.isCurrentWorkspace && !b.isCurrentWorkspace) return -1
      if (!a.isCurrentWorkspace && b.isCurrentWorkspace) return 1
      if (a.activeCount > 0 && b.activeCount === 0) return -1
      if (a.activeCount === 0 && b.activeCount > 0) return 1
      const aLatest = Math.max(0, ...a.sessions.map((s) => new Date(s.lastActiveTime).getTime() || 0))
      const bLatest = Math.max(0, ...b.sessions.map((s) => new Date(s.lastActiveTime).getTime() || 0))
      return bLatest - aLatest
    })

    return list
  }, [
    displayedSessions,
    workspaceRoot,
    customOrder,
    folderOverrides,
    folderOrder,
    pinnedSet,
    data?.userWorkspaces,
    data?.deletedWorkspaces,
    data?.archivedWorkspaces,
    viewFilter
  ])

  // 處理在 Dashboard 視窗內拖曳資料夾重新排序
  const handleReorderFolder = useCallback(
    (sourceKey: string, targetKey: string, pos: 'top' | 'bottom') => {
      if (sourceKey === targetKey) return
      setFolderOrder((prev) => {
        const currentKeys = folderGroups.map((g) => g.key)
        const baseOrder = prev.length > 0 ? [...prev] : [...currentKeys]
        for (const k of currentKeys) {
          if (!baseOrder.includes(k)) baseOrder.push(k)
        }
        const filtered = baseOrder.filter((k) => k !== sourceKey)
        const targetIdx = filtered.indexOf(targetKey)
        if (targetIdx === -1) {
          filtered.push(sourceKey)
        } else {
          const insertIdx = pos === 'top' ? targetIdx : targetIdx + 1
          filtered.splice(insertIdx, 0, sourceKey)
        }
        try {
          localStorage.setItem('agent-workbench:dashboard-folder-order', JSON.stringify(filtered))
        } catch {}
        return filtered
      })
    },
    [folderGroups]
  )

  // 處理在 Dashboard 視窗內拖曳卡片重新排序與跨群組移動
  const handleReorderSession = useCallback(
    (
      draggedId: string,
      targetId: string | null,
      position: 'before' | 'after' | 'inside',
      targetGroupKey: string
    ) => {
      const draggedSession = displayedSessions.find((s) => s.id === draggedId)
      const rawPath = draggedSession?.workspacePath?.trim() || ''
      const normPath = rawPath ? rawPath.toLowerCase().replace(/\\/g, '/').replace(/\/$/, '') : ''
      const wsName = draggedSession?.workspace?.trim() || (rawPath ? rawPath.split(/[\\/]/).filter(Boolean).pop() || '' : 'Other')
      const naturalKey = normPath || wsName.toLowerCase()

      // 1. 若拖曳跨資料夾，更新分組覆寫（若放回原自然分組則清除覆寫）
      setFolderOverrides((prev) => {
        const next = { ...prev }
        if (targetGroupKey === naturalKey) {
          delete next[draggedId]
        } else {
          next[draggedId] = targetGroupKey
        }
        try {
          localStorage.setItem('agent-workbench:dashboard-folder-overrides', JSON.stringify(next))
        } catch {}
        return next
      })

      // 2. 更新目標分組中的排序清單
      setCustomOrder((prev) => {
        const next: Record<string, string[]> = {}
        for (const [k, arr] of Object.entries(prev)) {
          next[k] = arr.filter((id) => id !== draggedId)
        }

        const currentGroupSessions =
          folderGroups.find((g) => g.key === targetGroupKey)?.sessions.map((s) => s.id) || []
        const existingOrder = next[targetGroupKey] || [...currentGroupSessions]
        const targetList = existingOrder.filter((id) => id !== draggedId)

        if (!targetId || position === 'inside') {
          targetList.push(draggedId)
        } else {
          const targetIdx = targetList.indexOf(targetId)
          if (targetIdx === -1) {
            targetList.push(draggedId)
          } else {
            const insertIdx = position === 'before' ? targetIdx : targetIdx + 1
            targetList.splice(insertIdx, 0, draggedId)
          }
        }

        next[targetGroupKey] = targetList
        try {
          localStorage.setItem('agent-workbench:dashboard-order', JSON.stringify(next))
        } catch {}
        return next
      })
    },
    [displayedSessions, folderGroups]
  )

  const handleSessionOpenCli = useCallback(
    (session: AgentSessionInfo) => {
      // 樂觀更新：點選後立即將本卡片狀態設為 active，提供零延遲之即時視覺反饋
      setData((prev) => {
        if (!prev) return prev
        return {
          ...prev,
          sessions: prev.sessions.map((s) =>
            s.id === session.id
              ? { ...s, status: 'active', lastActiveTime: new Date().toISOString() }
              : s
          )
        }
      })

      openTerminalSession({
        id: session.id,
        agent: session.agent,
        title: session.title,
        status: session.status,
        workspacePath: session.workspacePath,
        ensureRightDock: true
      })

      // 快速重新整理同步後端真實 PTY 進程
      setTimeout(() => loadData(true), 600)
      setTimeout(() => loadData(true), 2500)
    },
    [loadData]
  )

  const toggleFolderCollapse = (key: string): void => {
    setCollapsedFolders((prev) => {
      const next = new Set(prev)
      if (next.has(key)) {
        next.delete(key)
      } else {
        next.add(key)
      }
      return next
    })
  }

  const handleCollapseAll = (): void => {
    setCollapsedFolders(new Set(folderGroups.map((g) => g.key)))
  }

  const handleExpandAll = (): void => {
    setCollapsedFolders(new Set())
  }

  return (
    <div className="dash-root">
      {!sessionsOnly && (
      <>
      {/* Dashboard Header */}
      <div className="dash-header">
        <div className="dash-title-block">
          <h2 className="dash-title">{t('dashboard.telemetryTitle')}</h2>
          <span className="dash-subtitle">
            {lastRefreshed ? t('dashboard.updatedAt', { time: lastRefreshed }) : t('dashboard.analyzingSessions')}
          </span>
        </div>
        <button className="dash-refresh-btn" onClick={() => loadData(false, true)} disabled={loading} title={t('dashboard.refreshTooltip')}>
          <span className={loading ? 'dash-spinning' : ''}>↻</span>
        </button>
      </div>

      {/* Overview Metric Banner */}
      <div className="dash-banner">
        <div className="dash-banner-metric">
          <span className="dash-metric-label">{t('usage.totalLabel', { range: t(`usage.range_${usageRange}`) })}</span>
          <strong className="dash-metric-val">{data ? formatTokens(totalTokens) : '—'}</strong>
        </div>
        <div className="dash-banner-divider" />
        <div className="dash-banner-metric">
          <span className="dash-metric-label">{t('dashboard.activeProcesses')}</span>
          <strong className="dash-metric-val">{activeSessionsCount}</strong>
        </div>
        <div className="dash-banner-divider" />
        <div className="dash-banner-metric">
          <span className="dash-metric-label">{t('dashboard.totalSessions')}</span>
          <strong className="dash-metric-val">{allSessions.length}</strong>
        </div>
      </div>

      <div className="dash-range-row">
        <span className="dash-range-hint">{t('usage.rangeHint')}</span>
        <UsageRangeSwitch />
      </div>

      {/* Agent Usage Trio Cards (filtered by CLI settings) */}
      {enabledAgentIds.length === 0 ? (
        <div className="dash-no-agents-banner">
          <div className="dash-no-agents-content">
            <span className="dash-no-agents-icon">⚙️</span>
            <div className="dash-no-agents-text">
              <strong className="dash-no-agents-title">{t('dashboard.noAgentsEnabled')}</strong>
              <span className="dash-no-agents-desc">{t('dashboard.noAgentsEnabledDesc')}</span>
            </div>
          </div>
          <button
            type="button"
            className="dash-no-agents-btn"
            onClick={() => openSettings('cli')}
          >
            {t('sidebar.settings')} ➔
          </button>
        </div>
      ) : (
        <div className="dash-agents-grid">
          {enabledAgentIds.map((agentId) => {
            const cfg = AGENT_CONFIG[agentId]
            const usage = data?.agents[agentId]
            const activeCount = usage?.activeSessions ?? 0
            const ru = rangeUsage(agentId)
            const agentTokens = ru.input + ru.cacheRead + ru.output
            const isSelected = selectedAgent === agentId

            const total = agentTokens || 1
            const promptTokens = ru.input
            const toolTokens = ru.cacheRead
            const completionTokens = ru.output

            const pctPrompt = Math.min(100, Math.round((promptTokens / total) * 100))
            const pctTools = Math.min(100, Math.round((toolTokens / total) * 100))
            const pctComp = Math.max(0, 100 - pctPrompt - pctTools)

            return (
              <div
                key={agentId}
                className={`dash-agent-card dash-agent-${agentId} ${isSelected ? 'selected' : ''}`}
                onClick={() => setSelectedAgent((prev) => (prev === agentId ? 'all' : agentId))}
                title={`Click to filter sessions by ${cfg.name} (Total: ${agentTokens.toLocaleString()} tokens)`}
              >
                <div className="dash-agent-head">
                  <div className="dash-agent-brand">
                    <AgentMark agent={agentId} size={18} />
                    <div className="dash-agent-meta">
                      <strong className="dash-agent-name">{cfg.name}</strong>
                      <span className="dash-agent-sessions">
                        {usage?.totalSessions ?? 0} {usage?.totalSessions === 1 ? t('dashboard.sessionSingular') : t('dashboard.sessionPlural')}
                      </span>
                    </div>
                  </div>
                  <div className="dash-agent-badges">
                    {activeCount > 0 && (
                      <span className="dash-active-pill" title={`${activeCount} active terminal session(s)`}>
                        <span className="dash-pulse-dot" /> {t('dashboard.activeCount', { count: activeCount })}
                      </span>
                    )}
                  </div>
                </div>

                {/* Total Tokens Display */}
                <div className="dash-agent-token-stat">
                  <div className="dash-agent-stat-number-row">
                    <span className="dash-agent-stat-number">{formatTokens(agentTokens)}</span>
                    <span className="dash-agent-stat-unit">
                      {t('dashboard.totalTokensUnit')}
                      {usage?.estimated ? ` · ${t('usage.estimated')}` : ''}
                    </span>
                  </div>
                </div>

                {/* Segmented Token Distribution Bar */}
                <div
                  className="dash-agent-tokens-meter"
                  title={`In: ${promptTokens.toLocaleString()} (${pctPrompt}%) | Cache read: ${toolTokens.toLocaleString()} (${pctTools}%) | Out: ${completionTokens.toLocaleString()} (${pctComp}%)`}
                >
                  <div className="dash-agent-tokens-track">
                    <div
                      className="dash-agent-tokens-seg seg-prompt"
                      style={{ width: `${pctPrompt}%` }}
                    />
                    <div
                      className="dash-agent-tokens-seg seg-tools"
                      style={{ width: `${pctTools}%` }}
                    />
                    <div
                      className="dash-agent-tokens-seg seg-comp"
                      style={{ width: `${pctComp}%` }}
                    />
                  </div>
                </div>

                {/* Usage Breakdown 3-Column Grid */}
                <div className="dash-agent-breakdown-grid">
                  <div
                    className="dash-agent-breakdown-col"
                    title={`${t('usage.inTip')}: ${promptTokens.toLocaleString()} (${pctPrompt}%)`}
                  >
                    <div className="dash-agent-col-label">
                      <span className="dash-agent-chip-dot dot-prompt" />
                      <span>In</span>
                    </div>
                    <span className="dash-agent-col-val">{formatTokens(promptTokens)}</span>
                  </div>
                  <div
                    className="dash-agent-breakdown-col"
                    title={`${t('usage.cacheTip')}: ${toolTokens.toLocaleString()} (${pctTools}%)`}
                  >
                    <div className="dash-agent-col-label">
                      <span className="dash-agent-chip-dot dot-tools" />
                      <span>Cache</span>
                    </div>
                    {/* Antigravity 紀錄沒有快取概念：顯示 — 而非誤導的 0 */}
                    <span className="dash-agent-col-val">
                      {usage?.estimated ? '—' : formatTokens(toolTokens)}
                    </span>
                  </div>
                  <div
                    className="dash-agent-breakdown-col"
                    title={`${t('usage.outTip')}: ${completionTokens.toLocaleString()} (${pctComp}%)`}
                  >
                    <div className="dash-agent-col-label">
                      <span className="dash-agent-chip-dot dot-comp" />
                      <span>Out</span>
                    </div>
                    <span className="dash-agent-col-val">{formatTokens(completionTokens)}</span>
                  </div>
                </div>
              </div>
            )
          })}
        </div>
      )}
      </>
      )}

      {/* Sessions & Token Breakdown Section */}
      <div className="dash-sessions-section">
        <div className="dash-section-header">
          <div className="dash-filter-segmented">
            <button
              className={viewFilter === 'all' ? 'active' : ''}
              onClick={() => setViewFilter('all')}
            >
              {t('dashboard.sessionsTab', { count: unarchivedSessions.length })}
            </button>
            <button
              className={viewFilter === 'archived' ? 'active' : ''}
              onClick={() => setViewFilter('archived')}
            >
              {t('dashboard.archivedTab', { count: archivedSessions.length })}
            </button>
          </div>

          <div className="dash-section-actions">
            {selectedAgent !== 'all' && (
              <button
                className="dash-active-filter-badge"
                onClick={() => setSelectedAgent('all')}
                title="Click to clear filter"
              >
                {t('dashboard.filterClear', { name: AGENT_CONFIG[selectedAgent].name })}
              </button>
            )}

            <button
              type="button"
              className="dash-open-folder-btn"
              onClick={handleOpenFolder}
              title={t('dashboard.openFolderBtn')}
            >
              <IconFolder size={12} />
              <span>{t('dashboard.openFolderBtn')}</span>
            </button>

            <button
              type="button"
              className="dash-clone-repo-btn"
              onClick={handleOpenCloneModal}
              title={t('dashboard.cloneRepoTitle')}
            >
              <IconGitClone size={12} />
              <span>{t('dashboard.cloneRepoBtn')}</span>
            </button>

            {folderGroups.length > 0 && (
              <div className="dash-folder-toggle-group">
                <button
                  type="button"
                  className="dash-folder-tool-btn"
                  onClick={handleExpandAll}
                  title={t('dashboard.expandAll')}
                  aria-label={t('dashboard.expandAll')}
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="7 11 12 6 17 11" />
                    <polyline points="7 18 12 13 17 18" />
                  </svg>
                </button>
                <button
                  type="button"
                  className="dash-folder-tool-btn"
                  onClick={handleCollapseAll}
                  title={t('dashboard.collapseAll')}
                  aria-label={t('dashboard.collapseAll')}
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="7 13 12 18 17 13" />
                    <polyline points="7 6 12 11 17 6" />
                  </svg>
                </button>
              </div>
            )}
          </div>
        </div>

        {folderGroups.length === 0 ? (
          <div className="dash-empty-state">
            <div className="dash-empty-icon">{enabledAgentIds.length === 0 ? '⚙️' : '📋'}</div>
            <p className="dash-empty-title">
              {enabledAgentIds.length === 0
                ? t('dashboard.noAgentsEnabled')
                : viewFilter === 'archived'
                ? t('dashboard.noArchived')
                : t('dashboard.noSessions')}
            </p>
            <p className="dash-empty-desc">
              {enabledAgentIds.length === 0
                ? t('dashboard.noAgentsEnabledDesc')
                : viewFilter === 'archived'
                ? t('dashboard.noArchivedDesc')
                : t('dashboard.noSessionsDesc')}
            </p>
            {enabledAgentIds.length === 0 ? (
              <button
                type="button"
                className="dash-no-agents-btn"
                style={{ marginTop: 12 }}
                onClick={() => openSettings('cli')}
              >
                {t('sidebar.settings')} ➔
              </button>
            ) : (
              <div className="dash-empty-actions-row">
                <button
                  type="button"
                  className="dash-no-agents-btn"
                  onClick={handleOpenFolder}
                >
                  <IconFolder size={13} style={{ marginRight: 6 }} />
                  {t('dashboard.openFolderBtn')}
                </button>
                <button
                  type="button"
                  className="dash-no-agents-btn"
                  onClick={handleOpenCloneModal}
                >
                  <IconGitClone size={13} style={{ marginRight: 6 }} />
                  {t('dashboard.cloneRepoTitle')}
                </button>
              </div>
            )}
          </div>
        ) : (
          <div className="dash-session-list">
            {folderGroups.map((group) => {
              const isCollapsed = collapsedFolders.has(group.key)
              const isDraggingThis = draggedFolderKey === group.key
              const isPinned = pinnedSet.has(group.key)
              const dropIndicator = dragOverFolderTarget?.key === group.key ? dragOverFolderTarget.pos : null
              return (
                <div
                  key={group.key}
                  className={`dash-folder-group ${group.isCurrentWorkspace ? 'is-current' : ''} ${isPinned ? 'is-pinned' : ''} ${
                    isDraggingThis ? 'is-dragging' : ''
                  } ${dropIndicator ? `drag-over-folder-${dropIndicator}` : ''} ${
                    dragOverFolderKey === group.key ? 'drag-over-folder' : ''
                  }`}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    e.stopPropagation()
                    setSessionContextMenu(null)
                    setFolderContextMenu({
                      key: group.key,
                      name: group.name,
                      path: group.path,
                      isCurrentWorkspace: group.isCurrentWorkspace,
                      sessions: group.sessions,
                      x: e.clientX,
                      y: e.clientY
                    })
                  }}
                  onDragOver={(e) => {
                    if (e.dataTransfer.types.includes('application/x-dashboard-folder-key')) {
                      e.preventDefault()
                      e.dataTransfer.dropEffect = 'move'
                      const rect = e.currentTarget.getBoundingClientRect()
                      const midY = rect.top + rect.height / 2
                      const pos: 'top' | 'bottom' = e.clientY < midY ? 'top' : 'bottom'
                      if (dragOverFolderTarget?.key !== group.key || dragOverFolderTarget?.pos !== pos) {
                        setDragOverFolderTarget({ key: group.key, pos })
                      }
                    } else if (e.dataTransfer.types.includes('application/x-dashboard-session-id')) {
                      e.preventDefault()
                      e.dataTransfer.dropEffect = 'move'
                      if (dragOverFolderKey !== group.key) {
                        setDragOverFolderKey(group.key)
                      }
                    }
                  }}
                  onDragLeave={(e) => {
                    if (!e.currentTarget.contains(e.relatedTarget as Node)) {
                      if (dragOverFolderTarget?.key === group.key) {
                        setDragOverFolderTarget(null)
                      }
                      if (dragOverFolderKey === group.key) {
                        setDragOverFolderKey(null)
                      }
                    }
                  }}
                  onDrop={(e) => {
                    if (e.dataTransfer.types.includes('application/x-dashboard-folder-key')) {
                      const srcKey = e.dataTransfer.getData('application/x-dashboard-folder-key')
                      if (srcKey) {
                        e.preventDefault()
                        e.stopPropagation()
                        const pos = dragOverFolderTarget?.pos || 'bottom'
                        setDragOverFolderTarget(null)
                        setDraggedFolderKey(null)
                        handleReorderFolder(srcKey, group.key, pos)
                      }
                    } else {
                      const draggedId = e.dataTransfer.getData('application/x-dashboard-session-id')
                      if (draggedId) {
                        e.preventDefault()
                        e.stopPropagation()
                        setDragOverFolderKey(null)
                        handleReorderSession(draggedId, null, 'inside', group.key)
                      }
                    }
                  }}
                >
                  <div
                    className={`dash-folder-header ${isCollapsed ? 'collapsed' : 'expanded'}`}
                    onClick={() => toggleFolderCollapse(group.key)}
                    onContextMenu={(e) => {
                      e.preventDefault()
                      e.stopPropagation()
                      setSessionContextMenu(null)
                      setFolderContextMenu({
                        key: group.key,
                        name: group.name,
                        path: group.path,
                        isCurrentWorkspace: group.isCurrentWorkspace,
                        sessions: group.sessions,
                        x: e.clientX,
                        y: e.clientY
                      })
                    }}
                    title={`Click to ${isCollapsed ? 'expand' : 'collapse'} sessions in ${group.name}${group.path ? ` (${group.path})` : ''} • Right-click for options`}
                  >
                    {/* Top Row: Grip, Chevron, Folder Icon, Name, and Total Tokens */}
                    <div className="dash-folder-top">
                      <div className="dash-folder-title-left">
                        {/* Drag grip for folder reordering */}
                        <span
                          className="dash-folder-drag-handle"
                          draggable={true}
                          onDragStart={(e) => {
                            e.stopPropagation()
                            e.dataTransfer.setData('application/x-dashboard-folder-key', group.key)
                            e.dataTransfer.effectAllowed = 'move'
                            setDraggedFolderKey(group.key)
                          }}
                          onDragEnd={() => {
                            setDraggedFolderKey(null)
                            setDragOverFolderTarget(null)
                          }}
                          onClick={(e) => e.stopPropagation()}
                          title="Drag to reorder folder"
                        >
                          <IconGripVertical size={13} />
                        </span>
                        <span className={`dash-folder-chevron ${isCollapsed ? '' : 'expanded'}`}>
                          <svg
                            width="12"
                            height="12"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2.5"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                          >
                            <polyline points="9 18 15 12 9 6" />
                          </svg>
                        </span>
                        <span className="dash-folder-icon">
                          <IconFolder size={14} />
                        </span>
                        <strong className="dash-folder-name" title={group.path || group.name}>
                          {group.name}
                        </strong>
                        {group.isCurrentWorkspace && (
                          <span className="dash-folder-badge current">{t('dashboard.currentWorkspace')}</span>
                        )}
                        <button
                          type="button"
                          className={`dash-folder-pin-btn ${isPinned ? 'pinned' : ''}`}
                          aria-pressed={isPinned}
                          title={t(isPinned ? 'dashboard.unpinFolder' : 'dashboard.pinFolder')}
                          onClick={(e) => {
                            e.stopPropagation()
                            toggleFolderPin(group.key)
                          }}
                        >
                          <IconPin size={12} />
                        </button>
                      </div>
                      <span className="dash-folder-tokens" title={`${group.totalTokens.toLocaleString()} tokens`}>
                        {formatTokens(group.totalTokens)} {t('common.tokens')}
                      </span>
                    </div>

                    {/* Sub Row: Active indicator, Session count, and Switch Folder button */}
                    <div className="dash-folder-sub">
                      <div className="dash-folder-sub-left">
                        {group.activeCount > 0 && (
                          <span className="dash-folder-active-tag">
                            <span className="dash-pulse-dot" /> {t('dashboard.activeCount', { count: group.activeCount })}
                          </span>
                        )}
                        <span className="dash-folder-badge count">
                          {group.sessions.length} {group.sessions.length === 1 ? t('dashboard.sessionSingular') : t('dashboard.sessionPlural')}
                        </span>
                      </div>
                      {group.path && (
                        <div className="dash-folder-actions" onClick={(e) => e.stopPropagation()}>
                          {!group.isCurrentWorkspace && (
                            <button
                              type="button"
                              className="dash-folder-switch-btn"
                              onClick={() => {
                                if (group.path) {
                                  switchWorkspace(group.path)
                                } else {
                                  setSidebarTab('files')
                                }
                              }}
                              title={`${t('dashboard.switchFolder')}: ${group.path}`}
                            >
                              <span>{t('dashboard.switchFolder')} ➔</span>
                            </button>
                          )}
                          <button
                            type="button"
                            className="dash-folder-open-window-btn"
                            onClick={() => {
                              if (group.path && window.api?.window?.openProjectWindow) {
                                window.api.window.openProjectWindow(group.path)
                              }
                            }}
                            title={`${t('dashboard.openFolderInNewWindow')}: ${group.path}`}
                          >
                            <IconWindowNew size={11} />
                            <span>{t('dashboard.openProjectWindow')}</span>
                          </button>
                        </div>
                      )}
                    </div>
                  </div>

                  {!isCollapsed && (
                    <div className="dash-folder-sessions">
                      {group.sessions.length === 0 ? (
                        <div className="dash-empty-folder-hint">
                          <span>{t('dashboard.noSessionsInFolder')}</span>
                        </div>
                      ) : (
                        group.sessions.map((session: AgentSessionInfo) => (
                          <SessionCard
                            key={session.id}
                            session={session}
                            groupKey={group.key}
                            isExpanded={expandedSessionId === session.id}
                            onToggle={() => toggleExpand(session.id)}
                            onArchive={(e) => handleArchive(session.id, !!session.isArchived, e)}
                            onDelete={(e) => handleDeletePrompt(session, e)}
                            onReorder={handleReorderSession}
                            onOpenCli={handleSessionOpenCli}
                            onContextMenu={(s, gKey, x, y) => {
                              setFolderContextMenu(null)
                              setSessionContextMenu({ session: s, groupKey: gKey, x, y })
                            }}
                          />
                        ))
                      )}
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>

      {/* Apple HIG Destructive Alert Dialog */}
      <AppleAlertDialog
        isOpen={Boolean(sessionToDelete)}
        title={t('dashboard.deleteDialogTitle')}
        description={t('dashboard.deleteDialogDesc')}
        confirmLabel={t('dashboard.deleteConfirm')}
        cancelLabel={t('common.cancel')}
        isDestructive={true}
        detail={
          sessionToDelete ? (
            <div className="dash-alert-session-preview">
              <div className="dash-alert-session-top">
                <div className="dash-alert-session-agent">
                  <AgentMark agent={sessionToDelete.agent} size={15} />
                  <span className="dash-alert-session-agent-name">
                    {AGENT_CONFIG[sessionToDelete.agent]?.name || sessionToDelete.agent}
                  </span>
                </div>
                <span className={`dash-status-badge ${sessionToDelete.status}`}>
                  {sessionToDelete.status === 'active'
                    ? t('dashboard.statusActive')
                    : sessionToDelete.status === 'waiting_approval'
                    ? t('dashboard.statusWaitingApproval')
                    : sessionToDelete.status === 'completed'
                    ? t('dashboard.statusCompleted')
                    : t('dashboard.statusIdle')}
                </span>
              </div>
              <div className="dash-alert-session-title" title={sessionToDelete.title}>
                {sessionToDelete.title}
              </div>
              <div className="dash-alert-session-meta">
                <span>ID: {sessionToDelete.id.slice(0, 8)}</span>
                {sessionToDelete.workspace && (
                  <>
                    <span>•</span>
                    <span className="dash-alert-workspace" title={sessionToDelete.workspacePath || sessionToDelete.workspace}>
                      <IconFolder size={11} />
                      <span>{sessionToDelete.workspace}</span>
                    </span>
                  </>
                )}
                <span>•</span>
                <span>{formatTokens(sessionToDelete.totalTokens)} tokens</span>
              </div>
            </div>
          ) : null
        }
        onConfirm={handleConfirmDelete}
        onClose={() => setSessionToDelete(null)}
      />

      {/* Folder Hide from Dashboard Dialog */}
      <AppleAlertDialog
        isOpen={Boolean(folderToDelete)}
        title={t('dashboard.hideProjectConfirmTitle')}
        description={t('dashboard.hideProjectPrompt')}
        icon={<IconFolderMinus size={24} />}
        confirmLabel={t('dashboard.hideBtn')}
        cancelLabel={t('common.cancel')}
        isDestructive={false}
        detail={
          folderToDelete ? (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, width: '100%' }}>
              <div className="apple-alert-target-card">
                <IconFolder size={15} className="apple-alert-target-icon" />
                <span className="apple-alert-target-name">{folderToDelete.name}</span>
                {folderToDelete.path && (
                  <span className="apple-alert-target-path" title={folderToDelete.path}>
                    {folderToDelete.path}
                  </span>
                )}
              </div>
              <div className="apple-alert-safe-callout">
                <div className="apple-alert-safe-header">
                  <IconShieldCheck size={14} />
                  <span>{t('dashboard.hideProjectSafeTitle')}</span>
                </div>
                <p className="apple-alert-safe-desc">{t('dashboard.hideProjectSafeDesc')}</p>
              </div>
            </div>
          ) : null
        }
        onConfirm={async () => {
          if (!folderToDelete) return
          const target = folderToDelete
          setFolderToDelete(null)
          const targetWs = target.path || target.name || target.key
          const sessionIds = target.sessions.map((s) => s.id)
          if (targetWs && window.api?.dashboard?.deleteWorkspace) {
            await window.api.dashboard.deleteWorkspace(targetWs, sessionIds)
          } else if (sessionIds.length > 0) {
            await window.api.dashboard.deleteSessions(sessionIds)
          }
          // 同步從本機自訂排序中清除
          setFolderOrder((prev) => {
            const next = prev.filter((k) => k !== target.key && k !== target.name && k !== target.path)
            try {
              localStorage.setItem('agent-workbench:dashboard-folder-order', JSON.stringify(next))
            } catch {}
            return next
          })
          await loadData(true, true)
        }}
        onClose={() => setFolderToDelete(null)}
      />

      {/* Folder Header Context Menu */}
      {folderContextMenu && (
        <>
          <div
            className="dash-context-menu-backdrop"
            onClick={() => setFolderContextMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault()
              setFolderContextMenu(null)
            }}
          />
          <div
            className="dash-context-menu"
            style={{
              top: Math.min(folderContextMenu.y, window.innerHeight - 190),
              left: Math.min(folderContextMenu.x, window.innerWidth - 220)
            }}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              type="button"
              className="dash-context-item"
              onClick={() => {
                toggleFolderPin(folderContextMenu.key)
                setFolderContextMenu(null)
              }}
            >
              <IconPin size={13} />
              <span>{t(pinnedSet.has(folderContextMenu.key) ? 'dashboard.unpinFolder' : 'dashboard.pinFolder')}</span>
            </button>
            {folderContextMenu.path && !folderContextMenu.isCurrentWorkspace && (
              <button
                type="button"
                className="dash-context-item"
                onClick={() => handleContextSwitchFolder(folderContextMenu)}
              >
                <IconFolder size={13} />
                <span>{t('dashboard.switchFolder')}</span>
              </button>
            )}
            {folderContextMenu.path && (
              <button
                type="button"
                className="dash-context-item"
                onClick={() => handleContextOpenNewWindow(folderContextMenu)}
              >
                <IconWindowNew size={13} />
                <span>{t('dashboard.openProjectWindow')}</span>
              </button>
            )}
            <button
              type="button"
              className="dash-context-item"
              onClick={() => handleContextArchiveFolder(folderContextMenu)}
            >
              <IconArchive size={13} />
              <span>{viewFilter === 'archived' ? t('dashboard.restoreProject') : t('dashboard.archiveProject')}</span>
            </button>
            <div className="dash-context-divider" />
            <button
              type="button"
              className="dash-context-item"
              onClick={() => handleContextHideFolderPrompt(folderContextMenu)}
            >
              <IconEyeOff size={13} />
              <span>{t('dashboard.hideProject')}</span>
            </button>
          </div>
        </>
      )}

      {/* Session Card Context Menu */}
      {sessionContextMenu && (
        <>
          <div
            className="dash-context-menu-backdrop"
            onClick={() => setSessionContextMenu(null)}
            onContextMenu={(e) => {
              e.preventDefault()
              setSessionContextMenu(null)
            }}
          />
          <div
            className="dash-context-menu"
            style={{
              top: Math.min(sessionContextMenu.y, window.innerHeight - 200),
              left: Math.min(sessionContextMenu.x, window.innerWidth - 220)
            }}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              type="button"
              className="dash-context-item"
              onClick={() => {
                const s = sessionContextMenu.session
                setSessionContextMenu(null)
                handleSessionOpenCli(s)
              }}
            >
              <IconTerminalBox size={13} />
              <span>{sessionContextMenu.session.status === 'active' ? t('dashboard.switchCli') : t('dashboard.resumeCli')}</span>
            </button>

            {sessionContextMenu.session.workspacePath && (
              <button
                type="button"
                className="dash-context-item"
                onClick={async () => {
                  const s = sessionContextMenu.session
                  setSessionContextMenu(null)
                  if (s.workspacePath && window.api?.window?.openProjectWindow) {
                    await window.api.window.openProjectWindow(s.workspacePath)
                  }
                }}
              >
                <IconWindowNew size={13} />
                <span>{t('dashboard.openProjectWindow')}</span>
              </button>
            )}

            {sessionContextMenu.session.workspacePath && (
              <button
                type="button"
                className="dash-context-item"
                onClick={async () => {
                  const s = sessionContextMenu.session
                  setSessionContextMenu(null)
                  if (s.workspacePath) {
                    await switchWorkspace(s.workspacePath)
                    await loadData(true, true)
                  }
                }}
              >
                <IconFolder size={13} />
                <span>{t('dashboard.switchFolder')}</span>
              </button>
            )}

            <div className="dash-context-divider" />

            <button
              type="button"
              className="dash-context-item"
              onClick={async () => {
                const s = sessionContextMenu.session
                setSessionContextMenu(null)
                await handleArchive(s.id, !s.isArchived)
              }}
            >
              <IconArchive size={13} />
              <span>{sessionContextMenu.session.isArchived ? t('dashboard.restore') : t('dashboard.archive')}</span>
            </button>

            <button
              type="button"
              className="dash-context-item is-destructive"
              onClick={() => {
                const s = sessionContextMenu.session
                setSessionContextMenu(null)
                handleDeletePrompt(s)
              }}
            >
              <IconTrash size={13} />
              <span>{t('dashboard.delete')}</span>
            </button>
          </div>
        </>
      )}

      {/* Clone Repository Modal */}
      {showCloneModal && (
        <div className="dash-modal-backdrop" onClick={() => !cloneLoading && setShowCloneModal(false)}>
          <div className="dash-modal-dialog" onClick={(e) => e.stopPropagation()}>
            <div className="dash-modal-header">
              <div className="dash-modal-title-row">
                <IconGitClone size={18} className="dash-modal-icon" />
                <h3 className="dash-modal-title">{t('dashboard.cloneRepoTitle')}</h3>
              </div>
              <p className="dash-modal-desc">{t('dashboard.cloneRepoDesc')}</p>
            </div>

            <div className="dash-modal-body">
              <div className="dash-form-group">
                <label className="dash-form-label">{t('dashboard.repoUrlLabel')}</label>
                <input
                  type="text"
                  className="dash-form-input"
                  placeholder={t('dashboard.repoUrlPlaceholder')}
                  value={cloneUrl}
                  disabled={cloneLoading}
                  autoFocus
                  onChange={(e) => {
                    const newUrl = e.target.value
                    setCloneUrl(newUrl)
                    if (!cloneTargetDir || cloneTargetDir.endsWith('\\') || cloneTargetDir.endsWith('/')) {
                      const repoName = inferRepoName(newUrl)
                      if (repoName) {
                        const parent = workspaceRoot ? workspaceRoot.replace(/[/\\][^/\\]+$/, '') : 'D:\\'
                        setCloneTargetDir(`${parent}\\${repoName}`)
                      }
                    }
                  }}
                />
              </div>

              <div className="dash-form-group">
                <label className="dash-form-label">{t('dashboard.targetDirLabel')}</label>
                <div className="dash-form-input-with-btn">
                  <input
                    type="text"
                    className="dash-form-input"
                    placeholder={t('dashboard.targetDirPlaceholder')}
                    value={cloneTargetDir}
                    disabled={cloneLoading}
                    onChange={(e) => setCloneTargetDir(e.target.value)}
                  />
                  <button
                    type="button"
                    className="dash-form-browse-btn"
                    disabled={cloneLoading}
                    onClick={handleBrowseCloneTarget}
                  >
                    {t('dashboard.browseBtn')}
                  </button>
                </div>
              </div>

              {cloneError && (
                <div className="dash-modal-error">
                  <span>{cloneError}</span>
                </div>
              )}
            </div>

            <div className="dash-modal-footer">
              <button
                type="button"
                className="dash-modal-btn-cancel"
                disabled={cloneLoading}
                onClick={() => setShowCloneModal(false)}
              >
                {t('common.cancel')}
              </button>
              <button
                type="button"
                className="dash-modal-btn-primary"
                disabled={cloneLoading || !cloneUrl.trim() || !cloneTargetDir.trim()}
                onClick={handleExecuteClone}
              >
                {cloneLoading ? (
                  <>
                    <span className="dash-spinner" />
                    <span>{t('dashboard.cloningBtn')}</span>
                  </>
                ) : (
                  <>
                    <IconGitClone size={13} />
                    <span>{t('dashboard.cloneBtn')}</span>
                  </>
                )}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

function SessionCard({
  session,
  isExpanded,
  onToggle,
  onArchive,
  onDelete,
  groupKey,
  onReorder,
  onOpenCli,
  onContextMenu
}: {
  session: AgentSessionInfo
  isExpanded: boolean
  onToggle: () => void
  onArchive: (e: React.MouseEvent) => void
  onDelete: (e: React.MouseEvent) => void
  groupKey: string
  onReorder: (
    draggedId: string,
    targetId: string | null,
    position: 'before' | 'after' | 'inside',
    targetGroupKey: string
  ) => void
  onOpenCli?: (session: AgentSessionInfo) => void
  onContextMenu?: (session: AgentSessionInfo, groupKey: string, x: number, y: number) => void
}): JSX.Element {
  const { t } = useTranslation()
  const cfg = AGENT_CONFIG[session.agent]
  const { tokenBreakdown, totalTokens } = session
  const total = totalTokens || 1

  // Compute percentages for Apple Health-style meter
  const promptTokens = tokenBreakdown?.promptTokens ?? 0
  const toolTokens = tokenBreakdown?.toolReadTokens ?? 0
  const completionTokens = tokenBreakdown?.completionTokens ?? 0

  const pctPrompt = Math.min(100, Math.round((promptTokens / total) * 100))
  const pctTools = Math.min(100, Math.round((toolTokens / total) * 100))
  const pctComp = Math.max(0, 100 - pctPrompt - pctTools)

  const statusLabel =
    session.status === 'active'
      ? t('dashboard.statusActive')
      : session.status === 'waiting_approval'
      ? t('dashboard.statusWaitingApproval')
      : session.status === 'completed'
      ? t('dashboard.statusCompleted')
      : t('dashboard.statusIdle')

  const startTimeStr = new Date(session.startTime).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit'
  })

  const [isDragging, setIsDragging] = useState(false)
  const [dropIndicator, setDropIndicator] = useState<'top' | 'bottom' | null>(null)

  const handleOpenCli = (e: React.MouseEvent): void => {
    e.stopPropagation()
    if (onOpenCli) {
      onOpenCli(session)
    } else {
      openTerminalSession({
        id: session.id,
        agent: session.agent,
        title: session.title,
        status: session.status,
        workspacePath: session.workspacePath,
        ensureRightDock: true
      })
    }
  }

  const handleWorkspaceClick = async (e: React.MouseEvent): Promise<void> => {
    e.stopPropagation()
    const targetPath = session.workspacePath
    if (targetPath) {
      await switchWorkspace(targetPath)
    } else {
      setSidebarTab('files')
    }
  }

  const handleDragStart = (e: React.DragEvent<HTMLDivElement>): void => {
    setIsDragging(true)
    const sessionData = {
      type: 'agent-session' as const,
      id: session.id,
      agent: session.agent,
      title: session.title,
      status: session.status,
      workspace: session.workspace,
      workspacePath: session.workspacePath,
      model: session.model,
      totalTokens: session.totalTokens
    }
    setDraggedSession(sessionData)
    e.dataTransfer.setData('application/x-agent-session', JSON.stringify(sessionData))
    e.dataTransfer.setData('application/x-dashboard-session-id', session.id)
    e.dataTransfer.setData('application/x-dashboard-group-key', groupKey)
    e.dataTransfer.setData('text/plain', `[Session: @${session.agent}] ${session.title}`)
    e.dataTransfer.effectAllowed = 'copyMove'
  }

  const handleDragEnd = (): void => {
    setIsDragging(false)
    setDropIndicator(null)
    setDraggedSession(null)
  }

  const handleCardDragOver = (e: React.DragEvent<HTMLDivElement>): void => {
    if (isDragging) return
    if (e.dataTransfer.types.includes('application/x-dashboard-session-id')) {
      e.preventDefault()
      e.stopPropagation()
      e.dataTransfer.dropEffect = 'move'
      const rect = e.currentTarget.getBoundingClientRect()
      const midY = rect.top + rect.height / 2
      const pos: 'top' | 'bottom' = e.clientY < midY ? 'top' : 'bottom'
      if (dropIndicator !== pos) {
        setDropIndicator(pos)
      }
    }
  }

  const handleCardDragLeave = (e: React.DragEvent<HTMLDivElement>): void => {
    if (!e.currentTarget.contains(e.relatedTarget as Node)) {
      setDropIndicator(null)
    }
  }

  const handleCardDrop = (e: React.DragEvent<HTMLDivElement>): void => {
    const draggedId = e.dataTransfer.getData('application/x-dashboard-session-id')
    if (draggedId) {
      e.preventDefault()
      e.stopPropagation()
      const pos = dropIndicator === 'top' ? 'before' : 'after'
      setDropIndicator(null)
      if (draggedId !== session.id) {
        onReorder(draggedId, session.id, pos, groupKey)
      }
    }
  }

  return (
    <div
      className={`dash-session-card status-${session.status} ${isDragging ? 'dragging' : ''} ${dropIndicator === 'top' ? 'drag-over-top' : ''} ${dropIndicator === 'bottom' ? 'drag-over-bottom' : ''}`}
      draggable
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
      onDragOver={handleCardDragOver}
      onDragLeave={handleCardDragLeave}
      onDrop={handleCardDrop}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
        onContextMenu?.(session, groupKey, e.clientX, e.clientY)
      }}
      title="Click to open CLI, drag to reorder/move folder, or right-click for options"
    >
      <div className="dash-session-row">
        <div className="dash-drag-grip" title="Drag to reorder, move to another folder, or drag to terminal">
          <IconGripVertical size={13} />
        </div>

        <div
          className="dash-session-click-area"
          onClick={handleOpenCli}
          title={
            session.status === 'active'
              ? 'Click to switch to active CLI session in right terminal'
              : 'Click to open / resume this session in right terminal'
          }
        >
          <AgentMark agent={session.agent} size={18} />

          <div className="dash-session-main">
            <div className="dash-session-title-line">
              <strong className="dash-session-title" title={session.title}>
                {session.title}
              </strong>
              <span className={`dash-status-badge ${session.status}`}>{statusLabel}</span>
            </div>

            <div className="dash-session-meta-line">
              <span>{cfg.name}</span>
              {session.model && (
                <>
                  <span>•</span>
                  <span className="dash-meta-model">{session.model}</span>
                </>
              )}
              {session.workspace && (
                <>
                  <span>•</span>
                  <div className="dash-session-ws-group">
                    <button
                      type="button"
                      className="dash-session-workspace"
                      onClick={handleWorkspaceClick}
                      title={
                        session.workspacePath
                          ? `${t('dashboard.switchFolder')}: ${session.workspacePath}`
                          : `${t('dashboard.switchFolder')}: ${session.workspace}`
                      }
                    >
                      <IconFolder size={11} />
                      <span>{session.workspace}</span>
                    </button>
                    {session.workspacePath && (
                      <button
                        type="button"
                        className="dash-session-open-window-btn"
                        onClick={(e) => {
                          e.stopPropagation()
                          if (session.workspacePath && window.api?.window?.openProjectWindow) {
                            window.api.window.openProjectWindow(session.workspacePath)
                          }
                        }}
                        title={`${t('dashboard.openFolderInNewWindow')}: ${session.workspacePath}`}
                      >
                        <IconWindowNew size={10} />
                      </button>
                    )}
                  </div>
                </>
              )}
              <span>•</span>
              <span>{startTimeStr}</span>
            </div>
          </div>
        </div>

        <div className="dash-session-right-col">
          <div
            className="dash-session-token-summary"
            onClick={onToggle}
            title="Click to toggle token breakdown"
          >
            <strong className="dash-token-amount">{formatTokens(totalTokens)}</strong>
            <span className="dash-token-unit">{t('common.tokens')}</span>
          </div>

          <button className="dash-expand-chevron" onClick={onToggle} title="Show token breakdown">
            {isExpanded ? '▲' : '▼'}
          </button>
        </div>
      </div>

      {/* Apple Health Segmented Token Meter Bar */}
      <div className="dash-meter-container" title="Token Consumption Distribution">
        <div className="dash-meter-bar">
          <div
            className="dash-meter-seg seg-prompt"
            style={{ width: `${pctPrompt}%` }}
            title={`Prompts & System Context: ${formatTokens(promptTokens)} (${pctPrompt}%)`}
          />
          <div
            className="dash-meter-seg seg-tools"
            style={{ width: `${pctTools}%` }}
            title={`Tool Execution & Files: ${formatTokens(toolTokens)} (${pctTools}%)`}
          />
          <div
            className="dash-meter-seg seg-comp"
            style={{ width: `${pctComp}%` }}
            title={`Thinking & Generation: ${formatTokens(completionTokens)} (${pctComp}%)`}
          />
        </div>
      </div>

      {/* Action Bar: Open in CLI, Archive & Delete */}
      <div className="dash-session-actions-bar">
        <button
          className="dash-action-btn dash-action-open"
          onClick={handleOpenCli}
          title={
            session.status === 'active'
              ? 'Switch to this active CLI terminal'
              : 'Open / Resume this session in right CLI terminal'
          }
        >
          <IconTerminalBox size={12} />
          <span>{session.status === 'active' ? t('dashboard.switchCli') : t('dashboard.resumeCli')}</span>
        </button>
        <div className="dash-session-actions-right">
          <button
            className="dash-action-btn dash-action-icon-btn"
            onClick={onArchive}
            title={session.isArchived ? t('dashboard.restore') : t('dashboard.archive')}
          >
            <IconArchive size={12} />
            <span className="dash-action-label">{session.isArchived ? t('dashboard.restore') : t('dashboard.archive')}</span>
          </button>
          <button
            className="dash-action-btn dash-action-delete dash-action-icon-btn"
            onClick={onDelete}
            title={t('dashboard.delete')}
          >
            <IconTrash size={12} />
            <span className="dash-action-label">{t('dashboard.delete')}</span>
          </button>
        </div>
      </div>

      {/* Expanded Token Breakdown Analysis */}
      {isExpanded && (
        <div className="dash-session-breakdown">
          {tokenBreakdown?.details && tokenBreakdown.details.length > 0 && (
            <div className="dash-breakdown-analysis">
              <span className="dash-analysis-badge">{t('dashboard.analysisBadge')}</span>
              <p className="dash-analysis-text">
                {tokenBreakdown.details
                  .map(
                    (d: { category: string; tokens: number; percentage: number }) => {
                      let catName = d.category
                      if (/context|系統|提示/i.test(catName)) catName = t('dashboard.categoryContext')
                      else if (/tool|檔案|代碼|工具/i.test(catName)) catName = t('dashboard.categoryTools')
                      else if (/thinking|推論|思考|回覆/i.test(catName)) catName = t('dashboard.categoryThinking')
                      return `${catName}: ${d.percentage}% (${formatTokens(d.tokens)})`
                    }
                  )
                  .join(' • ')}
              </p>
            </div>
          )}

          <div className="dash-breakdown-grid">
            <div className="dash-breakdown-pill">
              <span className="dash-pill-indicator seg-prompt" />
              <div className="dash-pill-text">
                <span className="dash-pill-label">{t('dashboard.contextPrompt')}</span>
                <strong>{formatTokens(promptTokens)} ({pctPrompt}%)</strong>
              </div>
            </div>

            <div className="dash-breakdown-pill">
              <span className="dash-pill-indicator seg-tools" />
              <div className="dash-pill-text">
                <span className="dash-pill-label">{t('dashboard.toolExecution')}</span>
                <strong>{formatTokens(toolTokens)} ({pctTools}%)</strong>
              </div>
            </div>

            <div className="dash-breakdown-pill">
              <span className="dash-pill-indicator seg-comp" />
              <div className="dash-pill-text">
                <span className="dash-pill-label">{t('dashboard.outputGeneration')}</span>
                <strong>{formatTokens(completionTokens)} ({pctComp}%)</strong>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
