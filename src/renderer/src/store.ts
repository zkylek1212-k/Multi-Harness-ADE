import { useSyncExternalStore } from 'react'
import type { AgentId } from '../../preload/index'

// ── 極簡跨 panel 狀態（第二個整合縫合處）──────────────────────────────
// panel 只透過這些函式/hook 互動，彼此不直接 import。不要在 panel 內改這個檔。
//   openFile(path)  → 中央進入編輯模式並載入該檔（FileTree 點檔用）
//   openDiff(path)  → 中央進入 Git diff 模式（Git panel 點變更檔用）
//   bumpGit()       → 通知 Git panel 重新抓 status（存檔 / commit 後呼叫）
//   toggleTheme()   → 切換亮暗；panel 需 JS 感知主題時讀 useWorkbench().theme
export type Theme = 'light' | 'dark' | 'light-morandi' | 'dark-morandi'
export type Language = 'en' | 'zh-TW'
export type SidebarTab = 'dashboard' | 'files' | 'git'
/** developer＝現行程式碼優先版面；vibe＝對話＋即時成品優先版面 */
export type UiMode = 'developer' | 'vibe' | 'cowork'
export type UsageRange = 'all' | '30d' | '7d' | '1d'

/** 要送進終端的文字（例如 markdown code block 的指令）。nonce 遞增即代表有新的一筆。 */
export interface TerminalDispatch {
  text: string
  /** shell = 送到（必要時新開）一般 shell；active = 目前分頁 */
  target: 'shell' | 'active'
  /** 指定時一律新開這個 shell（例如安裝指令要在乾淨的 PowerShell 裡跑，不貼進使用者正在用的分頁） */
  newShell?: string
  nonce: number
}

/** 跨面板請求開啟/切換對應之 Agent CLI 終端會話 */
export interface TerminalOpenSessionRequest {
  id?: string
  agent: AgentId
  title?: string
  status?: string
  workspacePath?: string
  ensureRightDock?: boolean
  nonce: number
}

/** 跨面板檢視特定 Git Commit 比對差異 */
export interface GitCommitDiffTarget {
  commitHash: string
  parentHash?: string
  commitMessage?: string
  filePath: string
  files?: Array<{ path: string; status: string }>
}

export type SettingsTab = 'appearance' | 'cli' | 'doctools' | 'extensions' | 'remote' | 'cowork' | 'memsync' | 'about'

export interface SettingsModalState {
  isOpen: boolean
  tab: SettingsTab
}

export interface WorkbenchState {
  activeFilePath: string | null
  /** 已開啟的檔案分頁（依開啟順序） */
  openTabs: string[]
  viewMode: 'edit' | 'diff'
  activeCommitDiff: GitCommitDiffTarget | null
  gitTick: number // 遞增即代表 git 狀態該刷新
  settingsTick: number // 遞增即代表設定（如 CLI 啟用狀態）該刷新
  theme: Theme
  terminalDispatch: TerminalDispatch | null
  terminalOpenSession: TerminalOpenSessionRequest | null
  workspaceRoot: string
  /** 編輯器裡尚未存檔的檔案（Cowork 開會前提醒：這些內容不會被規劃看到） */
  editorDirtyPaths: string[]
  centerMaximized: boolean
  isTerminalDetached: boolean
  settingsModal: SettingsModalState
  language: Language
  sidebarTab: SidebarTab
  /** 檔案樹變更計數器，促使 FileTreePanel 自動更新 */
  fileTreeTick: number
  /** 記錄被終端 Agent 修改過的工作區檔案集合（供 Editor Tabs 呈現視覺標記） */
  agentModifiedFiles: Set<string>
  /** 每個檔案的外部變更計數器，促使 Editor 自動重新載入最新磁碟內容 */
  fileReloadTick: Record<string, number>
  /** 編輯器即時內容草稿（供 PreviewPanel 等即時自動更新 Markdown/HTML 預覽，無需手動按 Refresh） */
  editorDraft: { path: string; text: string; version: number } | null
  /**
   * 目前終端裡活著的 Agent 會話 id（TerminalPanel 的 associatedSessionId）。
   * Dashboard 用它判斷 active：分頁開著就是活的，這是 renderer 手上的事實，
   * 不必繞去 main 比對 PTY meta / jsonl mtime 再猜一次。
   */
  liveAgentSessionIds: string[]
  /**
   * 在這個 App 裡被關掉的 Agent 會話 → 關閉當下的時間戳。
   *
   * main 端用日誌 mtime 推狀態（幾分鐘內寫過就算 active），這對「在 App 外面自己跑的 CLI」
   * 是必要的，但會讓剛從終端關掉的會話卡在 active 好幾分鐘。這裡記下關閉時間，
   * 讓 Dashboard 把那種誤判壓成 idle。若之後檔案又被寫入（lastActiveTime 晚於關閉時間），
   * 代表外面真的有人在跑它，標記自動失效。
   */
  closedAgentSessions: Record<string, number>
  uiMode: UiMode
  /** 終端輸出偵測到的最新本機開發伺服器網址（nonce 遞增＝新偵測） */
  detectedDevUrl: { url: string; nonce: number } | null
  /** 請求內建 Browser 開啟某網址（nonce 遞增＝新請求） */
  browserRequest: { url: string; nonce: number } | null
  /** Agent 終端近期有輸出（約 1.5 秒內），供 Vibe 狀態列顯示「工作中」 */
  agentBusy: boolean
  /** Dashboard／Vibe 狀態列共用的 token 用量區間 */
  usageRange: UsageRange
}

function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem('wb-theme')
    if (
      saved === 'light' ||
      saved === 'dark' ||
      saved === 'light-morandi' ||
      saved === 'dark-morandi'
    ) {
      return saved as Theme
    }
  } catch {
    /* localStorage 不可用時忽略 */
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

function initialUiMode(): UiMode {
  try {
    const m = localStorage.getItem('wb-ui-mode')
    if (m === 'vibe' || m === 'cowork') return m
  } catch {
    /* localStorage 不可用時忽略 */
  }
  return 'developer'
}

function initialUsageRange(): UsageRange {
  try {
    const v = localStorage.getItem('wb-usage-range')
    if (v === 'all' || v === '30d' || v === '7d' || v === '1d') return v
  } catch {
    /* 忽略 */
  }
  return '30d'
}

function applyUiMode(mode: UiMode): void {
  document.documentElement.setAttribute('data-mode', mode)
}

function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute('data-theme', theme)
  try {
    const isDark = theme === 'dark' || theme === 'dark-morandi'
    window.api?.window?.setTitleBarTheme?.(isDark ? 'dark' : 'light')
  } catch {
    /* ignore */
  }
}

function initialLanguage(): Language {
  try {
    const saved = localStorage.getItem('wb-language')
    if (saved === 'en' || saved === 'zh-TW') {
      return saved
    }
  } catch {
    /* localStorage 不可用時忽略 */
  }
  if (typeof navigator !== 'undefined' && navigator.language) {
    if (navigator.language.toLowerCase().startsWith('zh')) {
      return 'zh-TW'
    }
  }
  return 'en'
}

let state: WorkbenchState = {
  activeFilePath: null,
  openTabs: [],
  viewMode: 'edit',
  activeCommitDiff: null,
  gitTick: 0,
  settingsTick: 0,
  theme: initialTheme(),
  language: initialLanguage(),
  terminalDispatch: null,
  terminalOpenSession: null,
  workspaceRoot: '',
  editorDirtyPaths: [],
  centerMaximized: false,
  isTerminalDetached: false,
  settingsModal: { isOpen: false, tab: 'appearance' },
  sidebarTab: 'files',
  fileTreeTick: 0,
  agentModifiedFiles: new Set<string>(),
  fileReloadTick: {},
  editorDraft: null,
  liveAgentSessionIds: [],
  closedAgentSessions: {},
  uiMode: initialUiMode(),
  detectedDevUrl: null,
  browserRequest: null,
  agentBusy: false,
  usageRange: initialUsageRange()
}
applyTheme(state.theme)
applyUiMode(state.uiMode)

// 異步載入後端工作區根目錄與偏好設定
if (typeof window !== 'undefined' && window.api?.files?.workspaceRoot) {
  window.api.files
    .workspaceRoot()
    .then((root) => {
      if (root && !state.workspaceRoot) {
        set({ workspaceRoot: root })
      }
    })
    .catch(() => {})
}

// 異步同步後端儲存之語言設定（若本機尚未指定或後端有更優先紀錄）
if (typeof window !== 'undefined' && window.api?.settings?.get) {
  window.api.settings
    .get()
    .then((s) => {
      if (s?.language && (s.language === 'en' || s.language === 'zh-TW')) {
        const local = localStorage.getItem('wb-language')
        if (!local && s.language !== state.language) {
          set({ language: s.language })
        }
      }
    })
    .catch(() => {})
}

// 監聽工作區檔案樹整體變更（新增/刪除/改名等）自動遞增 fileTreeTick 與 gitTick
if (typeof window !== 'undefined' && window.api?.files?.onTreeChange) {
  window.api.files.onTreeChange(() => {
    set({
      fileTreeTick: state.fileTreeTick + 1,
      gitTick: state.gitTick + 1
    })
  })
}

// 監聽後端工作區檔案變更（終端機 Agent 修改檔案時自動開檔與標記）
if (typeof window !== 'undefined' && window.api?.files?.onExternalChange) {
  window.api.files.onExternalChange(async ({ path }) => {
    try {
      const settings = await window.api.settings.get().catch(() => null)
      const autoOpen = settings?.autoOpenAgentModifiedFiles ?? true

      // 1. 標記該檔案為 Agent 所修改
      const nextModified = new Set(state.agentModifiedFiles)
      nextModified.add(path)

      // 2. 遞增檔案重載計數器，促使 Editor 自動載入磁碟最新文字
      const nextTicks = {
        ...state.fileReloadTick,
        [path]: (state.fileReloadTick[path] || 0) + 1
      }

      // 3. 遞增 Git 刷新計數器，讓左側 Git Panel 即時更新狀態
      const nextGitTick = state.gitTick + 1

      // 4. 若開啟自動分頁功能，將檔案開啟並切換至中央 Editor
      let nextTabs = state.openTabs
      let nextActive = state.activeFilePath
      let nextViewMode = state.viewMode

      if (autoOpen) {
        if (!nextTabs.includes(path)) {
          nextTabs = [...nextTabs, path]
        }
        nextActive = path
        nextViewMode = 'edit'
      }

      set({
        agentModifiedFiles: nextModified,
        fileReloadTick: nextTicks,
        gitTick: nextGitTick,
        openTabs: nextTabs,
        activeFilePath: nextActive,
        viewMode: nextViewMode,
        centerMaximized: false
      })
    } catch (err) {
      console.error('[Store] onExternalChange handler error:', err)
    }
  })
}

const listeners = new Set<() => void>()

function set(patch: Partial<WorkbenchState>): void {
  state = { ...state, ...patch }
  listeners.forEach((l) => l())
}

export function markAgentModified(path: string): void {
  const next = new Set(state.agentModifiedFiles)
  next.add(path)
  set({ agentModifiedFiles: next })
}

export function clearAgentModified(path: string): void {
  if (!state.agentModifiedFiles.has(path)) return
  const next = new Set(state.agentModifiedFiles)
  next.delete(path)
  set({ agentModifiedFiles: next })
}

export function clearAllAgentModified(): void {
  if (state.agentModifiedFiles.size === 0) return
  set({ agentModifiedFiles: new Set<string>() })
}

export function setUiMode(mode: UiMode): void {
  if (state.uiMode === mode) return
  applyUiMode(mode)
  try {
    localStorage.setItem('wb-ui-mode', mode)
  } catch {
    /* 忽略 */
  }
  set({ uiMode: mode, centerMaximized: false })
}

export function setUsageRange(range: UsageRange): void {
  try {
    localStorage.setItem('wb-usage-range', range)
  } catch {
    /* 忽略 */
  }
  set({ usageRange: range })
}

export function openInBrowser(url: string): void {
  set({ browserRequest: { url, nonce: (state.browserRequest?.nonce ?? 0) + 1 } })
}

/** 終端偵測到開發伺服器網址；同網址不重複觸發。Vibe 模式直接在內建 Browser 開啟。 */
export function reportDevUrl(url: string): void {
  if (state.detectedDevUrl?.url === url) return
  set({ detectedDevUrl: { url, nonce: (state.detectedDevUrl?.nonce ?? 0) + 1 } })
  if (state.uiMode === 'vibe') openInBrowser(url)
}

let agentBusyTimer: ReturnType<typeof setTimeout> | undefined
/** Agent 終端每有輸出就呼叫；只在 idle↔busy 轉換時 set，避免每個 chunk 都重繪。 */
export function pulseAgentActivity(): void {
  if (!state.agentBusy) set({ agentBusy: true })
  clearTimeout(agentBusyTimer)
  agentBusyTimer = setTimeout(() => set({ agentBusy: false }), 1500)
}

export function setEditorDirtyPaths(paths: string[]): void {
  const prev = state.editorDirtyPaths
  if (paths.length === prev.length && paths.every((p, i) => p === prev[i])) return
  set({ editorDirtyPaths: paths })
}

/** 使用者是否已主動設定過工作區；啟動時從後端還原的那一次不算（終端面板據此決定要不要接續最後的 CLI） */
export let workspaceChangedByUser = false

export function setWorkspaceRoot(path: string): void {
  workspaceChangedByUser = true
  set({ workspaceRoot: path })
}

export function setSidebarTab(tab: SidebarTab): void {
  set({ sidebarTab: tab })
}

export async function switchWorkspace(path: string): Promise<boolean> {
  if (path && window.api?.files?.setWorkspaceRoot) {
    const ok = await window.api.files.setWorkspaceRoot(path).catch(() => false)
    if (ok) {
      workspaceChangedByUser = true
      // 不切換側邊欄：從儀表板切資料夾後留在儀表板
      set({
        workspaceRoot: path,
        fileTreeTick: state.fileTreeTick + 1,
        gitTick: state.gitTick + 1
      })
      return true
    }
  }
  return false
}

export function toggleCenterMaximized(): void {
  set({ centerMaximized: !state.centerMaximized })
}

export function setCenterMaximized(val: boolean): void {
  set({ centerMaximized: val })
}

export function setTerminalDetached(val: boolean): void {
  set({ isTerminalDetached: val })
}

export function openFile(path: string): void {
  const tabs = state.openTabs.includes(path) ? state.openTabs : [...state.openTabs, path]
  set({ activeFilePath: path, viewMode: 'edit', openTabs: tabs, activeCommitDiff: null })
}

export function openDiff(path: string): void {
  const tabs = state.openTabs.includes(path) ? state.openTabs : [...state.openTabs, path]
  set({ activeFilePath: path, viewMode: 'diff', openTabs: tabs, activeCommitDiff: null })
}

export function openCommitDiff(target: GitCommitDiffTarget): void {
  const tabId = target.filePath
    ? `commit:${target.commitHash.slice(0, 7)}:${target.filePath}`
    : `commit:${target.commitHash.slice(0, 7)}`
  const tabs = state.openTabs.includes(tabId) ? state.openTabs : [...state.openTabs, tabId]
  set({
    activeFilePath: tabId,
    viewMode: 'diff',
    activeCommitDiff: target,
    openTabs: tabs,
    centerMaximized: false
  })
}

export function selectTab(tabId: string): void {
  if (tabId.startsWith('commit:')) {
    const parts = tabId.split(':')
    const hash = parts[1]
    const filePath = parts.slice(2).join(':')
    const target: GitCommitDiffTarget =
      state.activeCommitDiff && state.activeCommitDiff.commitHash.startsWith(hash)
        ? { ...state.activeCommitDiff, filePath }
        : { commitHash: hash, filePath }
    set({
      activeFilePath: tabId,
      viewMode: 'diff',
      activeCommitDiff: target
    })
  } else {
    openFile(tabId)
  }
}

/** 關閉分頁；關掉的若是當前檔，改選旁邊那個 */
export function closeTab(path: string): void {
  const idx = state.openTabs.indexOf(path)
  if (idx === -1) return
  const tabs = state.openTabs.filter((p) => p !== path)
  const nextActive =
    state.activeFilePath === path ? (tabs[Math.max(0, idx - 1)] ?? null) : state.activeFilePath
  const isCommit = nextActive ? nextActive.startsWith('commit:') : false
  set({
    openTabs: tabs,
    activeFilePath: nextActive,
    viewMode: isCommit ? 'diff' : (nextActive ? state.viewMode : 'edit'),
    activeCommitDiff: isCommit ? state.activeCommitDiff : null
  })
}
export function bumpGit(): void {
  set({ gitTick: state.gitTick + 1 })
}
export function bumpFileTree(): void {
  set({ fileTreeTick: state.fileTreeTick + 1 })
}
export function bumpSettings(): void {
  set({ settingsTick: state.settingsTick + 1 })
}
export function setEditorDraft(path: string, text: string): void {
  const current = state.editorDraft
  if (current && current.path === path && current.text === text) return
  set({
    editorDraft: {
      path,
      text,
      version: (current?.version || 0) + 1
    }
  })
}
export function clearEditorDraft(path?: string): void {
  if (!path || state.editorDraft?.path === path) {
    set({ editorDraft: null })
  }
}
export function openSettings(tab: SettingsTab = 'appearance'): void {
  set({ settingsModal: { isOpen: true, tab } })
}
export function closeSettings(): void {
  set({ settingsModal: { isOpen: false, tab: state.settingsModal.tab } })
}
export function setSettingsTab(tab: SettingsTab): void {
  set({ settingsModal: { ...state.settingsModal, tab } })
}
/**
 * 把文字送進終端。刻意「只貼上、不自動送出」——指令要不要執行由使用者按 Enter 決定。
 */
export function sendToTerminal(text: string, target: 'shell' | 'active' = 'shell'): void {
  set({
    terminalDispatch: {
      text,
      target,
      nonce: (state.terminalDispatch?.nonce ?? 0) + 1
    }
  })
}

/**
 * 新開一個指定的 shell 分頁，等它啟動完成後貼上 text。同樣不自動送出，由使用者確認後按 Enter。
 */
export function pasteInNewShell(text: string, shell: string): void {
  if (state.centerMaximized) set({ centerMaximized: false })
  set({
    terminalDispatch: {
      text,
      target: 'shell',
      newShell: shell,
      nonce: (state.terminalDispatch?.nonce ?? 0) + 1
    }
  })
}

/**
  * 開啟或切換至特定 Agent CLI 終端會話（如點選 Dashboard session）。
  * 自動解除中央最大化，並派發給 TerminalPanel 與 App 版面。
  */
export function openTerminalSession(req: {
  id?: string
  agent: AgentId
  title?: string
  status?: string
  workspacePath?: string
  ensureRightDock?: boolean
}): void {
  if (state.centerMaximized) {
    set({ centerMaximized: false })
  }
  set({
    terminalOpenSession: {
      ...req,
      ensureRightDock: req.ensureRightDock ?? true,
      nonce: (state.terminalOpenSession?.nonce ?? 0) + 1
    }
  })
}

/** TerminalPanel 回報目前活著（未 exit）的 Agent 會話 id，供 Dashboard 標記 active。 */
export function setLiveAgentSessionIds(ids: string[]): void {
  const prev = state.liveAgentSessionIds
  if (prev.length === ids.length && ids.every((x, i) => prev[i] === x)) return
  // 從清單消失＝分頁被關掉，記下時間；重新開起來就把標記清掉。
  const now = Date.now()
  const closed = { ...state.closedAgentSessions }
  for (const id of prev) {
    if (!ids.includes(id)) closed[id] = now
  }
  for (const id of ids) delete closed[id]
  set({ liveAgentSessionIds: ids, closedAgentSessions: closed })
}

export function setTheme(theme: Theme): void {
  if (state.theme === theme) return
  applyTheme(theme)
  try {
    localStorage.setItem('wb-theme', theme)
  } catch {
    /* 忽略 */
  }
  set({ theme })
}

export function setLanguage(language: Language): void {
  if (state.language === language) return
  try {
    localStorage.setItem('wb-language', language)
  } catch {
    /* 忽略 */
  }
  set({ language })
  window.api?.settings?.set?.({ language }).catch(() => {})
}

export function toggleTheme(): void {
  let next: Theme = 'dark'
  if (state.theme === 'dark') next = 'light'
  else if (state.theme === 'light') next = 'light-morandi'
  else if (state.theme === 'light-morandi') next = 'dark-morandi'
  else if (state.theme === 'dark-morandi') next = 'dark'
  setTheme(next)
}

export function useWorkbench(): WorkbenchState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb)
      return () => listeners.delete(cb)
    },
    () => state
  )
}

/** 跨面板拖曳之 Agent Session 酬載 */
export interface DraggedSessionPayload {
  type: 'agent-session'
  id: string
  agent: AgentId
  title: string
  status: string
  workspace?: string
  workspacePath?: string
  model?: string
  totalTokens?: number
}

let activeDraggedSession: DraggedSessionPayload | null = null

export function setDraggedSession(session: DraggedSessionPayload | null): void {
  activeDraggedSession = session
}

export function getDraggedSession(): DraggedSessionPayload | null {
  return activeDraggedSession
}
