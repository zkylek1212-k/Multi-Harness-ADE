import { contextBridge, ipcRenderer } from '../desktop/ipc-renderer'
import type { RemoteVibeStatus } from '../shared/remoteProtocol'
import type { CustomAgentConfig, CustomResourceKind } from '../shared/customAgent'
export type { CustomAgentConfig, CustomResourceKind } from '../shared/customAgent'
import type {
  AgentModelChoice,
  CoworkAgent,
  CoworkBaselineInfo,
  CoworkCapability,
  CoworkModelCatalog,
  CoworkResult,
  CoworkRun,
  CoworkRunSummary,
  CoworkSettings,
  ExecPreview,
  MergeApproval
} from '../shared/cowork'

// ── Workbench IPC 契約（唯一整合縫合處）────────────────────────────────
// 所有 renderer panel 一律透過 window.api.* 呼叫；main 端各 handler 檔各自實作。
// 新增方法時，這裡加一條 + src/preload/index.d.ts 加型別 + 對應 main/ipc/*.ts 實作 handle。
const api = {
  // 檔案 / 檔案樹 —— main/ipc/files.ts
  files: {
    read: (path: string): Promise<string> => ipcRenderer.invoke('files:read', path),
    write: (path: string, content: string): Promise<void> =>
      ipcRenderer.invoke('files:write', path, content),
    list: (dir: string): Promise<FsEntry[]> => ipcRenderer.invoke('files:list', dir),
    exists: (path: string): Promise<boolean> => ipcRenderer.invoke('files:exists', path),
    workspaceRoot: (): Promise<string> => ipcRenderer.invoke('files:workspaceRoot'),
    setWorkspaceRoot: (path: string): Promise<boolean> =>
      ipcRenderer.invoke('files:setWorkspaceRoot', path),
    pickWorkspace: (): Promise<string | null> => ipcRenderer.invoke('files:pickWorkspace'),
    openExternal: (path: string, customToolPath?: string): Promise<{ ok: boolean; error?: string }> =>
      ipcRenderer.invoke('files:openExternal', path, customToolPath),
    showInFolder: (path: string): Promise<void> => ipcRenderer.invoke('files:showInFolder', path),
    /** 在檔案總管／Finder 開啟這個視窗目前的專案資料夾 */
    revealWorkspace: (): Promise<{ ok: boolean; error?: string }> => ipcRenderer.invoke('files:revealWorkspace'),
    stat: (path: string): Promise<FileStat> => ipcRenderer.invoke('files:stat', path),
    pickExecutable: (title?: string): Promise<string | null> =>
      ipcRenderer.invoke('files:pickExecutable', title),
    detectDocTools: (): Promise<DocToolPaths> => ipcRenderer.invoke('files:detectDocTools'),
    onExternalChange: (
      cb: (info: { path: string; relativePath: string; eventType: string }) => void
    ): (() => void) => {
      const listener = (
        _event: unknown,
        info: { path: string; relativePath: string; eventType: string }
      ): void => cb(info)
      ipcRenderer.on('files:externalChange', listener)
      return () => ipcRenderer.removeListener('files:externalChange', listener)
    },
    onTreeChange: (cb: () => void): (() => void) => {
      const listener = (): void => cb()
      ipcRenderer.on('files:treeChange', listener)
      return () => ipcRenderer.removeListener('files:treeChange', listener)
    }
  },
  // OS 原生通知 —— main/ipc/notify.ts
  notify: {
    show: (title: string, body: string): void => ipcRenderer.send('notify:show', title, body)
  },
  // 擴充管理（Skill / MCP / Plugin 跨 agent）—— main/ipc/ext.ts
  ext: {
    customList: (kind: CustomResourceKind): Promise<string[]> => ipcRenderer.invoke('ext:customList', kind),
    customRead: (kind: CustomResourceKind, file: string): Promise<string | null> => ipcRenderer.invoke('ext:customRead', kind, file),
    customWrite: (kind: CustomResourceKind, file: string, content: string, before: string | null): Promise<void> =>
      ipcRenderer.invoke('ext:customWrite', kind, file, content, before),
    agents: (): Promise<AgentStatus[]> => ipcRenderer.invoke('ext:agents'),
    inventory: (): Promise<ExtItem[]> => ipcRenderer.invoke('ext:inventory'),
    manifest: (): Promise<ExtManifest> => ipcRenderer.invoke('ext:manifest'),
    saveManifest: (m: ExtManifest): Promise<void> => ipcRenderer.invoke('ext:saveManifest', m),
    planSync: (): Promise<FileChange[]> => ipcRenderer.invoke('ext:planSync'),
    applySync: (): Promise<{ written: string[] }> => ipcRenderer.invoke('ext:applySync'),
    trustWorkspace: (): Promise<boolean> => ipcRenderer.invoke('ext:trustWorkspace'),
    getAgentInstallInfo: (agent: AgentId): Promise<AgentInstallInfo> =>
      ipcRenderer.invoke('ext:getAgentInstallInfo', agent),
    toggleItem: (kind: string, id: string, enabled: boolean, agent?: AgentId): Promise<boolean> =>
      ipcRenderer.invoke('ext:toggleItem', kind, id, enabled, agent)
  },
  // 連線憑證（值只進 OS 加密儲存，不回傳給 renderer）—— main/ipc/conn.ts
  conn: {
    list: (): Promise<ConnectionInfo[]> => ipcRenderer.invoke('conn:list'),
    set: (name: string, value: string): Promise<void> =>
      ipcRenderer.invoke('conn:set', name, value),
    remove: (name: string): Promise<void> => ipcRenderer.invoke('conn:remove', name)
  },
  // 設定管理（CLI 路徑等）—— main/ipc/settings.ts
  settings: {
    get: (): Promise<WorkbenchSettings> => ipcRenderer.invoke('settings:get'),
    set: (patch: Partial<WorkbenchSettings>): Promise<WorkbenchSettings> =>
      ipcRenderer.invoke('settings:set', patch),
    getRecentWorkspaces: (): Promise<string[]> =>
      ipcRenderer.invoke('settings:getRecentWorkspaces'),
    removeRecentWorkspace: (workspacePath: string): Promise<string[]> =>
      ipcRenderer.invoke('settings:removeRecentWorkspace', workspacePath),
    clearRecentWorkspaces: (): Promise<void> =>
      ipcRenderer.invoke('settings:clearRecentWorkspaces'),
    testCliPath: (path: string): Promise<{ ok: boolean; version?: string; error?: string; resolvedPath?: string }> =>
      ipcRenderer.invoke('settings:testCliPath', path),
    testDocToolPath: (path: string): Promise<{ ok: boolean; version?: string; error?: string }> =>
      ipcRenderer.invoke('settings:testDocToolPath', path),
    // 設定被別處改了（例如手機遠端切換 Bypass 模式）
    onChanged: (cb: () => void): (() => void) => {
      const listener = (): void => cb()
      ipcRenderer.on('settings:changed', listener)
      return () => ipcRenderer.removeListener('settings:changed', listener)
    }
  },
  // 自動更新與版本管理 —— main/ipc/updater.ts
  updater: {
    getStatus: (): Promise<UpdaterStatus> => ipcRenderer.invoke('updater:getStatus'),
    check: (): Promise<UpdaterStatus> => ipcRenderer.invoke('updater:check'),
    download: (): Promise<boolean> => ipcRenderer.invoke('updater:download'),
    install: (): Promise<void> => ipcRenderer.invoke('updater:install'),
    openRelease: (url?: string): Promise<void> => ipcRenderer.invoke('updater:openRelease', url),
    onStatusChange: (cb: (status: UpdaterStatus) => void): (() => void) => {
      const handler = (_: unknown, status: UpdaterStatus): void => cb(status)
      ipcRenderer.on('updater:statusChange', handler)
      return (): void => {
        ipcRenderer.removeListener('updater:statusChange', handler)
      }
    }
  },
  // 儀表板與使用量統計 —— main/ipc/dashboard.ts
  dashboard: {
    data: (force?: boolean): Promise<DashboardData> => ipcRenderer.invoke('dashboard:data', force),
    usage: (): Promise<Record<AgentId, Record<UsageRange, UsageBucket>>> =>
      ipcRenderer.invoke('dashboard:usage'),
    archiveSession: (sessionId: string, archive: boolean): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:archiveSession', sessionId, archive),
    archiveSessions: (sessionIds: string[], archive: boolean): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:archiveSessions', sessionIds, archive),
    deleteSession: (sessionId: string): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:deleteSession', sessionId),
    deleteSessions: (sessionIds: string[]): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:deleteSessions', sessionIds),
    archiveWorkspace: (workspacePath: string, archive: boolean, sessionIds?: string[]): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:archiveWorkspace', workspacePath, archive, sessionIds),
    deleteWorkspace: (workspacePath: string, sessionIds?: string[]): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:deleteWorkspace', workspacePath, sessionIds),
    unmarkWorkspace: (workspacePath: string): Promise<boolean> =>
      ipcRenderer.invoke('dashboard:unmarkWorkspace', workspacePath),
    bgTasks: (workspacePath: string): Promise<BgTask[]> => ipcRenderer.invoke('dashboard:bgTasks', workspacePath)
  },
  // Git —— main/ipc/git.ts
  git: {
    status: (): Promise<GitStatus> => ipcRenderer.invoke('git:status'),
    log: (limit?: number): Promise<GitCommit[]> => ipcRenderer.invoke('git:log', limit),
    graph: (limit?: number): Promise<GitGraphNode[]> => ipcRenderer.invoke('git:graph', limit),
    diff: (path: string): Promise<{ head: string; work: string }> =>
      ipcRenderer.invoke('git:diff', path),
    stage: (path: string): Promise<void> => ipcRenderer.invoke('git:stage', path),
    unstage: (path: string): Promise<void> => ipcRenderer.invoke('git:unstage', path),
    commit: (message: string): Promise<void> => ipcRenderer.invoke('git:commit', message),
    restore: (path: string): Promise<void> => ipcRenderer.invoke('git:restore', path),
    branches: (): Promise<{ current: string; all: string[] }> =>
      ipcRenderer.invoke('git:branches'),
    checkout: (branch: string): Promise<void> => ipcRenderer.invoke('git:checkout', branch),
    commitDetails: (hash: string): Promise<GitCommitDetail> =>
      ipcRenderer.invoke('git:commitDetails', hash),
    commitFileDiff: (
      hash: string,
      filePath: string,
      parentHash?: string
    ): Promise<{ original: string; modified: string }> =>
      ipcRenderer.invoke('git:commitFileDiff', hash, filePath, parentHash),
    clone: (
      repoUrl: string,
      targetDir: string
    ): Promise<{ success: boolean; targetDir?: string; error?: string }> =>
      ipcRenderer.invoke('git:clone', repoUrl, targetDir)
  },
  // CLI 終端殼（node-pty）—— main/ipc/pty.ts
  pty: {
    spawn: (opts: PtySpawnOptions): Promise<string> => ipcRenderer.invoke('pty:spawn', opts),
    write: (id: string, data: string): void => ipcRenderer.send('pty:write', id, data),
    pipe: (fromId: string, toId: string, text: string): Promise<boolean> =>
      ipcRenderer.invoke('pty:pipe', fromId, toId, text),
    resize: (id: string, cols: number, rows: number): void =>
      ipcRenderer.send('pty:resize', id, cols, rows),
    kill: (id: string): void => ipcRenderer.send('pty:kill', id),
    // 接上一個已在 main 跑著的 pty（手機遠端開的終端），回傳目前畫面
    attach: (id: string): Promise<{ snapshot: string; cols: number; rows: number } | null> =>
      ipcRenderer.invoke('pty:attach', id),
    // main 依輸出順序套用尺寸後才通知（pty.ts resizePty），renderer 照這個改 xterm 尺寸
    onResized: (id: string, cb: (cols: number, rows: number) => void): (() => void) => {
      const ch = `pty:resized:${id}`
      const listener = (_e: unknown, cols: number, rows: number): void => cb(cols, rows)
      ipcRenderer.on(ch, listener)
      return () => ipcRenderer.removeListener(ch, listener)
    },
    // 手機遠端在這個視窗的工作區開了新終端，renderer 收到後開分頁並 attach
    onRemoteSpawned: (cb: (info: RemoteSpawnedPty) => void): (() => void) => {
      const listener = (_e: unknown, info: RemoteSpawnedPty): void => cb(info)
      ipcRenderer.on('pty:remoteSpawned', listener)
      return () => ipcRenderer.removeListener('pty:remoteSpawned', listener)
    },
    onData: (id: string, cb: (data: string) => void): (() => void) => {
      const ch = `pty:data:${id}`
      const listener = (_e: unknown, data: string): void => cb(data)
      ipcRenderer.on(ch, listener)
      return () => ipcRenderer.removeListener(ch, listener)
    },
    onExit: (id: string, cb: (code: number) => void): (() => void) => {
      const ch = `pty:exit:${id}`
      const listener = (_e: unknown, code: number): void => cb(code)
      ipcRenderer.on(ch, listener)
      return () => ipcRenderer.removeListener(ch, listener)
    },
    // 讀取 agents/*.yaml launcher 定義
    launchers: (): Promise<CliLauncher[]> => ipcRenderer.invoke('pty:launchers')
  },
  // 手機遠端控制（區網 Remote Bridge）—— main/remote/index.ts
  remote: {
    reportVibeStatus: (status: RemoteVibeStatus): void => ipcRenderer.send('remote:vibeStatus', status),
    status: (): Promise<RemoteStatus> => ipcRenderer.invoke('remote:status'),
    setEnabled: (enabled: boolean): Promise<RemoteStatus> => ipcRenderer.invoke('remote:setEnabled', enabled),
    setPort: (port: number): Promise<RemoteStatus> => ipcRenderer.invoke('remote:setPort', port),
    setAllowMesh: (allow: boolean): Promise<RemoteStatus> => ipcRenderer.invoke('remote:setAllowMesh', allow),
    qr: (text: string): Promise<string> => ipcRenderer.invoke('remote:qr', text),
    createPairing: (address?: string): Promise<RemotePairingInfo | null> => ipcRenderer.invoke('remote:createPairing', address),
    devices: (): Promise<RemoteDeviceInfo[]> => ipcRenderer.invoke('remote:devices'),
    revokeDevice: (id: string): Promise<boolean> => ipcRenderer.invoke('remote:revokeDevice', id),
    resetTrust: (): Promise<RemoteStatus> => ipcRenderer.invoke('remote:resetTrust'),
    onStatusChange: (cb: (status: RemoteStatus) => void): (() => void) => {
      const listener = (_e: unknown, status: RemoteStatus): void => cb(status)
      ipcRenderer.on('remote:status', listener)
      return () => ipcRenderer.removeListener('remote:status', listener)
    }
  },
  // 記憶同步：專案記憶放進私人 GitHub repo 或 OneDrive／Google Drive 資料夾 —— main/ipc/memsync.ts、main/memsync/core.ts
  memsync: {
    state: (): Promise<MemSyncResult<MemSyncState>> => ipcRenderer.invoke('memsync:state'),
    setConfig: (
      patch: Partial<{ consent: boolean; backend: MemSyncBackend; repo: string; autoSync: boolean; autoLink: boolean }>
    ): Promise<MemSyncResult<MemSyncState>> => ipcRenderer.invoke('memsync:setConfig', patch),
    pickFolder: (): Promise<MemSyncResult<{ picked: boolean; state: MemSyncState }>> => ipcRenderer.invoke('memsync:pickFolder'),
    launchCloudApp: (provider: 'onedrive' | 'gdrive'): Promise<MemSyncResult<boolean>> => ipcRenderer.invoke('memsync:launchCloudApp', provider),
    setupRepo: (): Promise<MemSyncResult<{ result: MemSyncRepoResult; state: MemSyncState }>> => ipcRenderer.invoke('memsync:setupRepo'),
    /** 免 gh 一鍵建立：用 Git 的 GitHub 登入建立（或沿用）私人 repo */
    createRepoViaGit: (name: string): Promise<MemSyncResult<{ result: MemSyncRepoResult; state: MemSyncState }>> =>
      ipcRenderer.invoke('memsync:createRepoViaGit', name),
    /** 免 gh：連結使用者自己建好的私人 repo（網址或 owner/repo） */
    connectRepo: (input: string): Promise<MemSyncResult<{ result: MemSyncRepoResult; state: MemSyncState }>> =>
      ipcRenderer.invoke('memsync:connectRepo', input),
    /** 設定頁按下才呼叫；本機與共用的記憶內容不同時，prefer 指定用哪一份 */
    linkProject: (prefer?: 'shared' | 'local'): Promise<MemSyncResult<{ result: MemSyncLinkResult; state: MemSyncState }>> =>
      ipcRenderer.invoke('memsync:linkProject', prefer),
    sync: (pullOnly?: boolean): Promise<MemSyncResult<{ result: MemSyncSyncResult; state: MemSyncState }>> => ipcRenderer.invoke('memsync:sync', pullOnly),
    /** 開啟／切換專案時呼叫；記憶同步沒開啟時立刻回 null，不碰網路 */
    onProjectOpen: (): Promise<
      MemSyncResult<{ pulled: MemSyncSyncResult | null; linked: MemSyncLinkResult | null; needsReview: 'tracked' | 'differs' | null } | null>
    > => ipcRenderer.invoke('memsync:onProjectOpen'),
    openClone: (): Promise<void> => ipcRenderer.invoke('memsync:openClone'),
    /** Workbench 自帶的 shared-project-memory plugin：每家 agent 的安裝狀態 */
    pluginState: (): Promise<MemSyncResult<MemoryPluginState>> => ipcRenderer.invoke('memsync:pluginState'),
    /** 使用者同意後：裝上自帶版，並移除其他來源的版本 */
    applyPlugin: (agents: MemoryPluginAgent[]): Promise<MemSyncResult<{ results: MemoryPluginApplyResult[]; state: MemoryPluginState }>> =>
      ipcRenderer.invoke('memsync:applyPlugin', agents),
    /** 使用者拒絕：這個自帶版本不再自動詢問 */
    declinePlugin: (): Promise<MemSyncResult<MemoryPluginState>> => ipcRenderer.invoke('memsync:declinePlugin')
  },
  // Cowork 多 agent 規劃會議 —— main/ipc/cowork.ts（設計見 cowork.md）
  cowork: {
    capabilities: (force?: boolean): Promise<CoworkResult<{ agents: CoworkCapability[]; baseline: CoworkBaselineInfo }>> =>
      ipcRenderer.invoke('cowork:capabilities', force),
    // 三家的可選模型（設定頁與開會表單用）；第一次會跑 CLI 讀清單，之後快取
    models: (force?: boolean): Promise<CoworkResult<CoworkModelCatalog[]>> => ipcRenderer.invoke('cowork:models', force),
    list: (): Promise<CoworkResult<CoworkRunSummary[]>> => ipcRenderer.invoke('cowork:list'),
    get: (runId: string): Promise<CoworkResult<CoworkRun | null>> => ipcRenderer.invoke('cowork:get', runId),
    start: (req: {
      mode?: 'discussion' | 'project'
      autoEffort?: boolean
      summarizer?: CoworkAgent
      summarizeEachRound?: boolean
      prompt: string
      chair: CoworkAgent
      participants: CoworkAgent[]
      language: 'en' | 'zh-TW'
      models?: Partial<Record<CoworkAgent, AgentModelChoice>>
    }): Promise<CoworkResult<CoworkRun>> => ipcRenderer.invoke('cowork:start', req),
    cancel: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:cancel', runId),
    discuss: (runId: string, text: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:discuss', runId, text),
    setSummarizer: (runId: string, agent: CoworkAgent): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:summarizer', runId, agent),
    summarize: (runId: string, conclude?: boolean): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:summarize', runId, conclude),
    retry: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:retry', runId),
    dropFailedReviewers: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:drop', runId),
    addNote: (runId: string, text: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:note', runId, text),
    feedback: (runId: string, text: string): Promise<CoworkResult<void>> =>
      ipcRenderer.invoke('cowork:feedback', runId, text),
    editBoard: (
      runId: string,
      basePlanRevision: number,
      tasks: unknown
    ): Promise<CoworkResult<{ ok: true } | { ok: false; errors: string[] }>> =>
      ipcRenderer.invoke('cowork:editBoard', runId, basePlanRevision, tasks),
    dismissUnresolved: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:dismiss', runId),
    approve: (runId: string, planRevision: number): Promise<CoworkResult<void>> =>
      ipcRenderer.invoke('cowork:approve', runId, planRevision),
    raiseLimits: (
      runId: string,
      limits: { maxPlanningCalls?: number; maxPlanningMinutes?: number }
    ): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:raiseLimits', runId, limits),
    logDispatch: (runId: string, taskId: string, target: string): Promise<CoworkResult<void>> =>
      ipcRenderer.invoke('cowork:logDispatch', runId, taskId, target),
    delete: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:delete', runId),
    // 背景執行：在 repo 內的 .cowork/<runId>/ worktree 裡跑，進度與回覆經 onUpdate 回來
    execPreview: (runId: string, opts: { mode: 'sequential' | 'parallel'; linkDeps: boolean }): Promise<CoworkResult<ExecPreview>> =>
      ipcRenderer.invoke('cowork:execPreview', runId, opts),
    execStart: (runId: string, opts: { mode: 'sequential' | 'parallel'; linkDeps: boolean; approval: string }): Promise<CoworkResult<void>> =>
      ipcRenderer.invoke('cowork:execStart', runId, opts),
    execWaive: (runId: string, taskId: string, reason: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execWaive', runId, taskId, reason),
    execWaiveGitConfig: (runId: string, taskId: string, reason: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execWaiveGitConfig', runId, taskId, reason),
    execMessage: (runId: string, taskId: string, text: string): Promise<CoworkResult<void>> =>
      ipcRenderer.invoke('cowork:execMessage', runId, taskId, text),
    execRetry: (runId: string, taskId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execRetry', runId, taskId),
    // 越界改動：接受（照原樣 commit）或還原越界的檔
    execAcceptScope: (runId: string, taskId: string, hash?: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execAcceptScope', runId, taskId, hash),
    execRevertScope: (runId: string, taskId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execRevertScope', runId, taskId),
    execPause: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execPause', runId),
    execResume: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execResume', runId),
    execMerge: (runId: string, approval: MergeApproval): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execMerge', runId, approval),
    execCleanup: (runId: string): Promise<CoworkResult<void>> => ipcRenderer.invoke('cowork:execCleanup', runId),
    bypass: (): Promise<CoworkResult<boolean>> => ipcRenderer.invoke('cowork:bypass'),
    onUpdate: (cb: (run: CoworkRun) => void): (() => void) => {
      const listener = (_e: unknown, run: CoworkRun): void => cb(run)
      ipcRenderer.on('cowork:update', listener)
      return () => ipcRenderer.removeListener('cowork:update', listener)
    }
  },
  // 視窗管理（獨立彈出終端等）
  window: {
    detachTerminal: (): Promise<boolean> => ipcRenderer.invoke('window:openTerminalWindow'),
    attachTerminal: (): Promise<boolean> => ipcRenderer.invoke('window:closeTerminalWindow'),
    openProjectWindow: (workspacePath?: string): Promise<boolean> =>
      ipcRenderer.invoke('window:openProjectWindow', workspacePath),
    setTitleBarTheme: (theme: 'light' | 'dark'): Promise<boolean> =>
      ipcRenderer.invoke('window:setTitleBarTheme', theme),
    onTerminalAttached: (cb: () => void): (() => void) => {
      const listener = (): void => cb()
      ipcRenderer.on('terminal:attached', listener)
      return () => ipcRenderer.removeListener('terminal:attached', listener)
    }
  }
}

contextBridge.exposeInMainWorld('api', api)

export type WorkbenchApi = typeof api

// —— 契約用到的資料型別（renderer 與 main 共用）——
export interface FsEntry {
  name: string
  path: string
  isDir: boolean
}
export interface GitFileChange {
  path: string
  index: string // staged 狀態碼
  working_dir: string // 工作區狀態碼
}
export interface GitStatus {
  isRepo: boolean
  current: string
  staged: GitFileChange[]
  unstaged: GitFileChange[]
  untracked: string[]
}
export interface GitCommit {
  hash: string
  date: string
  message: string
  author: string
}

export interface GitGraphNode {
  hash: string
  parents: string[]
  author: string
  date: string
  refs: string[]
  message: string
}

export interface GitCommitFileChange {
  path: string
  status: string
}

export interface GitCommitDetail {
  hash: string
  fullHash: string
  parents: string[]
  author: string
  date: string
  message: string
  files: GitCommitFileChange[]
}

export interface DocToolPaths {
  word?: string
  excel?: string
  powerpoint?: string
  pdf?: string
  [key: string]: string | undefined
}

export interface FileStat {
  size: number
  mtime: string
  isFile: boolean
}

export interface CliTestRecord {
  ok: boolean
  version?: string
  error?: string
  testedAt?: number
  testedPath?: string
}

export interface WorkbenchSettings {
  customAgent?: CustomAgentConfig
  cliPaths: {
    claude?: string
    antigravity?: string
    codex?: string
    powershell?: string
    cmd?: string
    [key: string]: string | undefined
  }
  cliEnabled?: {
    claude?: boolean
    antigravity?: boolean
    codex?: boolean
    powershell?: boolean
    cmd?: boolean
    [key: string]: boolean | undefined
  }
  cliTestResults?: Record<string, CliTestRecord>
  cliBypassPermissions?: boolean
  docToolPaths?: DocToolPaths
  docToolTestResults?: Record<string, CliTestRecord>
  autoOpenAgentModifiedFiles?: boolean
  language?: 'en' | 'zh-TW'
  lastWorkspace?: string
  recentWorkspaces?: string[]
  autoCheckUpdates?: boolean
  autoDownloadUpdates?: boolean
  /** Cowork 主席、與會者與預算上限；只認全域設定，repo 不能放寬（cowork.md §8） */
  cowork?: CoworkSettings
}

export interface UpdateInfo {
  version: string
  releaseDate?: string
  releaseNotes?: string
  releaseName?: string
  downloadUrl?: string
}

export interface UpdaterStatus {
  currentVersion: string
  isPackaged: boolean
  isInstalled: boolean
  checking: boolean
  updateAvailable: boolean
  updateDownloaded: boolean
  isDownloading: boolean
  downloadProgress?: {
    percent: number
    bytesPerSecond: number
    transferred: number
    total: number
  }
  updateInfo?: UpdateInfo
  error?: string
  /** 這個版本自己的更新來源（owner/repo）；官方版就是原作者的 repo */
  channelRepo?: string
  /** 只有 fork 版才有：原作者那邊的版本狀態，只提示、不自動安裝 */
  upstream?: UpstreamStatus
}

export interface UpstreamStatus {
  repo: string
  /** 這個 fork 版基於原作者的哪一版 */
  baseVersion: string
  latestVersion?: string
  latestReleaseDate?: string
  latestUrl?: string
  /** 原作者有比 baseVersion 新的版本 */
  newer: boolean
  checkedAt?: number
  error?: string
}

export interface SessionTokenBreakdown {
  promptTokens: number
  toolReadTokens: number
  completionTokens: number
  details: {
    category: string
    tokens: number
    percentage: number
  }[]
}

export interface AgentSessionInfo {
  id: string
  agent: AgentId
  title: string
  status: 'active' | 'waiting_approval' | 'idle' | 'completed' | 'error'
  startTime: string
  lastActiveTime: string
  totalTokens: number
  tokenBreakdown: SessionTokenBreakdown
  model?: string
  isArchived?: boolean
  workspace?: string
  workspacePath?: string
}

export interface WindowUsage {
  usedPct: number
  resetsAt?: string | null
  resetsInSeconds?: number | null
  tokens?: number
  label?: string
}

/** Dashboard 用量區間：全部歷史／近 30 天／近 7 天／今天（本地時區） */
export type UsageRange = 'all' | '30d' | '7d' | '1d'

/** input 含 cache 寫入；cacheRead 另計（Claude 計費約原價一成）；output 為模型輸出 */
/** Agent 背景任務（背景指令／subagent）；status 為 running、completed、failed 或 Agent 回報的其他狀態 */
export interface BgTask {
  agent: AgentId
  id: string
  desc: string
  startedAt: string
  status: string
  summary?: string
  endedAt?: string
}

export interface UsageBucket {
  input: number
  cacheRead: number
  output: number
}

export interface AgentUsageSummary {
  agent: AgentId
  label: string
  totalSessions: number
  activeSessions: number
  totalTokens: number
  promptTokens: number
  toolTokens: number
  completionTokens: number
  /** true＝紀錄檔無真實 token 欄位，數字為字數估算（Antigravity） */
  estimated?: boolean
  usedPct?: number
  quotaLimit?: number
  fiveHour?: WindowUsage
  weekly?: WindowUsage
}

export interface DashboardWorkspaceInfo {
  path: string
  name: string
  isArchived: boolean
}

export interface DashboardData {
  agents: Record<AgentId, AgentUsageSummary>
  /** 各 Agent 在各區間的用量（掃全部歷史紀錄，非僅最近 N 筆 session） */
  usageByRange?: Record<AgentId, Record<UsageRange, UsageBucket>>
  sessions: AgentSessionInfo[]
  userWorkspaces?: DashboardWorkspaceInfo[]
  archivedWorkspaces?: string[]
  deletedWorkspaces?: string[]
}

export interface CliLauncher {
  id: string
  name: string
  /** 已驗證的內建 CLI 名稱，或全域設定已啟用的 custom-agent */
  cli: string
  command: string
  args: string[]
  env?: Record<string, string>
  /** yaml 有自帶 args／env＝真的自訂；否則只是內建 CLI 的別名 */
  hasExtras?: boolean
}
// ── 擴充管理型別 ──────────────────────────────────────────────────
export type AgentId = 'claude' | 'antigravity' | 'codex'
export type ExtKind = 'skill' | 'mcp' | 'plugin'

export interface AgentInstallInfo {
  id: AgentId
  name: string
  command: string
  targetPath: string
  /** 在哪一種終端執行 command（安裝指令會貼進看得見的終端，由使用者按 Enter） */
  shell: 'powershell' | 'bash'
}

/** 某個擴充在某個 agent 上的狀態 */
export type SupportState =
  | 'installed' // 已裝且可用
  | 'missing' // 支援但尚未裝
  | 'unsupported' // 該 agent 沒有這個概念
  | 'pending' // 尚未支援（Codex）
  | 'error' // 裝了但讀取/設定有問題

export interface AgentSupport {
  agent: AgentId
  state: SupportState
  detail?: string
  enabled?: boolean
}

export interface ExtItem {
  id: string
  kind: ExtKind
  name: string
  description?: string
  version?: string
  /** 由 workbench manifest 管理（可同步），或只是掃到的既有安裝 */
  managed: boolean
  agents: AgentSupport[]
  /** 需要的連線憑證名稱（來自 env 的 ${conn:x} 佔位） */
  needsConnection?: string[]
  enabled?: boolean
}

export interface AgentStatus {
  agent: AgentId
  label: string
  /** CLI 執行檔是否存在 */
  cliFound: boolean
  cliPath?: string
  configHome?: string
  /** workbench 是否支援管理這個 agent */
  supported: boolean
  pending: boolean
  counts: { skill: number; mcp: number; plugin: number }
  notes: string[]
}

export interface ManifestMcp {
  id: string
  command: string
  args?: string[]
  env?: Record<string, string>
  targets: AgentId[]
}
export interface ManifestSkill {
  id: string
  /** 相對工作區的來源資料夾，內含 SKILL.md */
  path: string
  targets: AgentId[]
}
export interface ManifestPlugin {
  id: string
  /** Claude: <plugin>@<marketplace>；Antigravity: plugin 目錄名 */
  targets: AgentId[]
}
export interface ExtManifest {
  version: number
  skills: ManifestSkill[]
  mcp: ManifestMcp[]
  plugins: ManifestPlugin[]
}

/** 同步前的變更預覽（before/after 餵給 Monaco DiffEditor） */
export interface FileChange {
  path: string
  agent: AgentId
  before: string
  after: string
  note?: string
}

export interface ConnectionInfo {
  name: string
  /** 只回報有沒有設定，永遠不回傳值 */
  isSet: boolean
  usedBy: string[]
}

// ── 記憶同步（main/memsync/core.ts 的結果型別） ────────────────────────
/** 記憶放哪裡：GitHub 私人 repo，或由 OneDrive／Google Drive 同步的資料夾 */
export type MemSyncBackend = 'github' | 'onedrive' | 'gdrive'
export interface MemSyncConfig {
  /** 使用者同意開啟的時間；null = 未同意 */
  consentAt: number | null
  backend: MemSyncBackend | null
  /** 私人記憶 repo 名稱（repo 或 owner/repo） */
  repo: string
  /** 這台電腦上記憶 repo 的 clone 位置（GitHub 模式） */
  clonePath: string
  repoFull: string | null
  login: string | null
  /** 雲端硬碟模式選的資料夾；記憶只存在這裡 */
  folderPath: string | null
  /** GitHub 模式：開專案時拉取＋每 10 分鐘背景同步 */
  autoSync: boolean
  /** 開專案時把有記憶的專案自動接上 */
  autoLink: boolean
  /** 使用者拒絕安裝的自帶 plugin 版本 */
  pluginDeclined: string | null
}
export interface MemSyncState {
  config: MemSyncConfig
  /** 偵測到的雲端硬碟根目錄 */
  cloudRoots: { onedrive: string[]; gdrive: string[] }
  /** 雲端硬碟程式是否已安裝（已安裝但沒登入時 cloudRoots 會是空的） */
  cloudApps: { onedrive: { installed: boolean; exe?: string }; gdrive: { installed: boolean; exe?: string } }
  /** 目前的記憶根目錄（clone 或資料夾），尚未選擇時為 null */
  root: string | null
  rootReady: boolean
  gitMissing: boolean
  gh: { installed: boolean; loggedIn: boolean; login?: string }
  clone: { exists: boolean; origin?: string; ahead?: number; behind?: number; dirty?: boolean }
  project: {
    key: string
    target: string
    linked: boolean
    linkedElsewhere: boolean
    linkedTo?: string
    linkedToOldStore?: boolean
    hasLocalMemory: boolean
    hasRemoteMemory: boolean
    tracked: string[]
    trackedEntries: string[]
    memoryDiffers: boolean
    branch: string
  } | null
  commands: { shell: 'powershell' | 'bash'; installGh: string; loginGh: string; installOneDrive: string; installGDrive: string }
}
export type MemoryPluginAgent = 'claude' | 'codex' | 'antigravity'
export interface MemoryPluginAgentState {
  agent: MemoryPluginAgent
  /** 設定裡有啟用這家 agent */
  enabled: boolean
  /** 找得到 CLI */
  cli: boolean
  /** 已安裝的 shared-project-memory（任何來源） */
  installed: { key: string; version: string | null; scope: string }[]
  /** ok：已是自帶版；install：沒裝；update：自帶版較舊；migrate：有 GitHub 或其他來源的版本 */
  action: 'ok' | 'install' | 'update' | 'migrate'
}
export interface MemoryPluginState {
  bundledVersion: string
  /** 使用者拒絕過的自帶版本；和 bundledVersion 相同就不自動詢問 */
  declined: string | null
  agents: MemoryPluginAgentState[]
}
export interface MemoryPluginApplyResult {
  agent: MemoryPluginAgent
  ok: boolean
  log: string[]
  error?: string
}
export interface MemSyncRepoResult {
  repo: string
  created: boolean
  cloned: boolean
  clonePath: string
}
export interface MemSyncLinkResult {
  key: string
  target: string
  source: 'local' | 'repo' | 'already-linked'
  backup?: string
  projectCommit?: string
}
export interface MemSyncSyncResult {
  committed: boolean
  pulled: boolean
  pushed: boolean
  error?: string
}
export type MemSyncResult<T> = { ok: true; data: T } | { ok: false; code: string; message: string }

export interface RemoteStatus {
  enabled: boolean
  running: boolean
  port: number
  addresses: string[]
  allowMesh: boolean
  meshDetected: string[]
  meshNeedsNewCa: boolean
  /** 手機 PWA 網址（https） */
  appUrl: string | null
  /** 安裝 CA 憑證的設定頁（http） */
  setupUrl: string | null
  caFingerprint: string | null
  connectedDevices: string[]
  error: string | null
}
export interface RemotePairingInfo {
  code: string
  expiresAt: number
  pairUrl: string | null
  pairQr: string | null
}
export interface RemoteDeviceInfo {
  id: string
  name: string
  createdAt: number
  lastSeenAt: number
  pushEnabled: boolean
  online: boolean
}
export interface RemoteSpawnedPty {
  ptyId: string
  /** 與 renderer 的 launcherKey 相同：claude / codex / antigravity / powershell / cmd 或自訂 launcher id */
  launcherKey: string
  title: string
}
export interface PtySpawnOptions {
  launcherId?: string
  command?: string
  args?: string[]
  cwd?: string
  cols?: number
  rows?: number
  sessionId?: string
  /** 分頁顯示名稱；遠端控制的 session 清單也用這個名字 */
  title?: string
}
