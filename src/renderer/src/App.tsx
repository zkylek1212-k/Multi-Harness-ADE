import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import FileTreePanel from '@panels/filetree/FileTreePanel'
import GitPanel from '@panels/git/GitPanel'
import EditorPanel from '@panels/editor/EditorPanel'
import PreviewPanel from '@panels/preview/PreviewPanel'
import MemoryPanel from '@panels/memory/MemoryPanel'
import TerminalPanel from '@panels/terminal/TerminalPanel'
import DashboardPanel from '@panels/dashboard/DashboardPanel'
import TestBrowserPanel from '@panels/browser/TestBrowserPanel'
import SettingsModal from '@/components/SettingsModal'
import Splitter from '@/components/Splitter'
import VibeRail from '@/components/VibeRail'
import VibeUsageBar from '@/components/VibeUsageBar'
import RemoteIndicator from '@/components/RemoteIndicator'
import { ProxyStatusPill } from '@/components/ProxySettings'
// React Flow 只在進 Cowork 模式時載入，不拖慢 Developer／Vibe 啟動
const CanvasView = lazy(() => import('@panels/canvas/CanvasView'))
import {
  IconSidebarCollapse,
  IconSidebarExpand,
  IconDockRight,
  IconDockBottom,
  IconSettings,
  IconMaximize,
  IconMinimize,
  IconClose,
  IconTerminalBox
} from '@/components/Icons'
import {
  toggleCenterMaximized,
  useWorkbench,
  openSettings,
  closeSettings,
  setSidebarTab,
  setWorkspaceRoot,
  openInBrowser,
  bumpSettings
} from '@/store'
import { useTranslation } from '@/i18n'
import {
  clamp,
  DEFAULT_LAYOUT,
  LIMITS,
  loadLayout,
  saveLayout,
  type Dock,
  type LayoutState
} from '@/layout'

type CenterTab = 'editor' | 'preview' | 'browser' | 'memory'

/** 已通知過「記憶同步需要確認」的專案（每次啟動每個專案只通知一次） */
const memsyncNotified = new Set<string>()

export default function App(): JSX.Element {
  const isDetachedWindow =
    typeof window !== 'undefined' && window.location.search.includes('mode=terminal-detached')

  // 若是以獨立視窗啟動的終端模式，整面直接呈現全螢幕 TerminalPanel
  if (isDetachedWindow) {
    return (
      <div
        className="app detached-term-app"
        style={{ height: '100vh', width: '100vw', overflow: 'hidden' }}
      >
        <TerminalPanel />
      </div>
    )
  }

  const {
    centerMaximized,
    terminalOpenSession,
    settingsModal,
    sidebarTab,
    uiMode,
    browserRequest,
    detectedDevUrl,
    agentBusy,
    workspaceRoot,
    liveAgentSessionIds,
    agentModifiedFiles
  } = useWorkbench()
  const isVibe = uiMode === 'vibe'
  const isCowork = uiMode === 'cowork'
  // Cowork 畫布蓋在原本的版面上；底下仍是 Developer 版面且保持掛載，終端不會因切換模式而結束（§2.4）
  const layoutMode: 'developer' | 'vibe' = isVibe ? 'vibe' : 'developer'
  // Vibe 模式左欄只當任務看板（Dashboard）；Files / Git 收起，程式碼改由中欄「程式碼」分頁看 diff
  const activeSidebarTab = isVibe ? 'dashboard' : sidebarTab
  const [center, setCenter] = useState<CenterTab>(isVibe ? 'browser' : 'editor')
  const [browserOpened, setBrowserOpened] = useState(false)
  const [proxySettingsOpen, setProxySettingsOpen] = useState(false)
  const { t } = useTranslation()

  useEffect(() => {
    window.api.remote.reportVibeStatus({
      workspace: workspaceRoot,
      agentBusy,
      agentCount: liveAgentSessionIds.length,
      devUrl: detectedDevUrl?.url || null,
      changedFiles: [...agentModifiedFiles]
    })
  }, [workspaceRoot, agentBusy, liveAgentSessionIds, detectedDevUrl, agentModifiedFiles])

  // 記憶同步：開啟／切換專案時自動接上記憶並拉取（main 只在使用者開啟記憶同步時才動作，否則不碰網路）。
  // 需要使用者確認、所以沒有自動接上的專案，每次啟動通知一次
  useEffect(() => {
    if (!workspaceRoot) return
    window.api.memsync
      .onProjectOpen()
      .then((r) => {
        const reason = r.ok ? r.data?.needsReview : null
        if (!reason || memsyncNotified.has(workspaceRoot)) return
        memsyncNotified.add(workspaceRoot)
        const name = workspaceRoot.split(/[\\/]/).filter(Boolean).pop() || workspaceRoot
        window.api.notify.show(t('memsync.reviewTitle'), t(`memsync.review_${reason}`, { name }))
      })
      .catch(() => {})
  }, [workspaceRoot])

  // 若以 ?workspace= 參數開啟獨立專案視窗，初始化工作區與側邊欄；若未指定工作區，預設停留在 Dashboard
  // 設定被別處改了（手機遠端切換 Bypass 等）→ 讓各面板重新讀取
  useEffect(() => window.api.settings.onChanged(() => bumpSettings()), [])

  useEffect(() => {
    const urlParams = new URLSearchParams(window.location.search)
    const wsParam = urlParams.get('workspace')
    if (wsParam) {
      setWorkspaceRoot(wsParam)
      setSidebarTab('files')
    } else {
      setSidebarTab('dashboard')
    }
  }, [])

  // 全域快捷鍵：Ctrl+Shift+N (Cmd+Shift+N) 開啟新專案視窗（如同 VS Code）
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'N' || e.key === 'n')) {
        e.preventDefault()
        if (window.api?.window?.openProjectWindow) {
          window.api.window.openProjectWindow()
        }
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [])

  // 記錄已造訪過的面板，實現「按需掛載（Mount-On-Demand）+ 狀態保留（Keep-Alive）」
  // 啟動時不掛載未造訪的次要面板（如 Git、Preview、Memory），大幅縮短首屏載入時間與消除開機子行程搶佔
  const [visitedSidebarTabs, setVisitedSidebarTabs] = useState<Set<string>>(
    () => new Set([sidebarTab || 'dashboard'])
  )
  const [visitedCenterTabs, setVisitedCenterTabs] = useState<Set<string>>(
    () => new Set([center || 'editor'])
  )

  useEffect(() => {
    if (sidebarTab) {
      setVisitedSidebarTabs((prev) => (prev.has(sidebarTab) ? prev : new Set(prev).add(sidebarTab)))
    }
  }, [sidebarTab])

  useEffect(() => {
    if (center) {
      setVisitedCenterTabs((prev) => (prev.has(center) ? prev : new Set(prev).add(center)))
    }
  }, [center])

  useEffect(() => {
    if (center === 'browser') {
      setBrowserOpened(true)
    }
  }, [center])

  // 兩種模式各存一份版面；layoutModeRef 記住目前 layout 屬於哪個模式，切換時不會寫錯 key
  const layoutModeRef = useRef(layoutMode)
  const [layout, setLayout] = useState<LayoutState>(() => loadLayout(layoutMode))
  useEffect(() => saveLayout(layout, layoutModeRef.current), [layout])
  useEffect(() => {
    if (layoutModeRef.current === layoutMode) return
    layoutModeRef.current = layoutMode
    setLayout(loadLayout(layoutMode))
    setCenter(layoutMode === 'vibe' ? 'browser' : 'editor')
  }, [layoutMode])

  // 有人請求開網址（終端偵測到 dev server / 狀態列）→ 切到 Browser
  useEffect(() => {
    if (!browserRequest) return
    setCenter('browser')
    setLayout((l) => ({ ...l, centerCollapsed: false }))
  }, [browserRequest?.nonce])

  // 監聽跨面板請求開啟/聚焦終端：若指定 ensureRightDock 且目前非右側停靠，自動切換至右側停靠
  useEffect(() => {
    if (!terminalOpenSession || isVibe) return
    // 終端欄被關掉時，有人要開／聚焦終端（例如從儀表板點會話）就把它打開
    if (layout.termHidden) setLayout((l) => ({ ...l, termHidden: false }))
    if (terminalOpenSession.ensureRightDock && layout.dock !== 'right') {
      setLayout((l) => ({
        ...l,
        dock: 'right',
        rightW: Math.max(l.rightW, LIMITS.rightMin)
      }))
    }
  }, [terminalOpenSession, layout.dock])

  // 切換側邊欄標籤時，若側邊欄原本為摺疊狀態，自動展開以利立即檢視內容
  useEffect(() => {
    if (isVibe) return
    if (layout.leftCollapsed && sidebarTab) {
      setLayout((l) => ({
        ...l,
        leftCollapsed: false,
        leftW: Math.max(l.leftW, LIMITS.leftMin)
      }))
    }
  }, [sidebarTab])

  const isBottom = !isVibe && layout.dock === 'bottom'
  const isCenterMaximized = Boolean(centerMaximized)
  // Vibe 模式沒有左欄：Dashboard 的 session 清單改由最左圖示列懸浮開啟
  const isLeftCollapsed = isVibe || isCenterMaximized || Boolean(layout.leftCollapsed || layout.leftW === 0)
  // 開發模式預設收起中央欄（編輯器／Preview），畫面只剩側邊欄＋終端；聚焦模式下中央欄必定展開
  const isCenterCollapsed = !isVibe && !isCenterMaximized && Boolean(layout.centerCollapsed)
  const expandCenter = (): void => setLayout((l) => ({ ...l, centerCollapsed: false }))
  // 頂端列兩個開關：>_ 開關終端欄，編輯器分頁開關編輯器欄；關掉其中一個時另一個一定開著
  const isTermHidden = !isVibe && !isCenterMaximized && !isCenterCollapsed && Boolean(layout.termHidden)
  const toggleTerminal = (): void =>
    setLayout((l) => (l.termHidden ? { ...l, termHidden: false } : { ...l, termHidden: true, centerCollapsed: false }))
  const onEditorTab = (tab: CenterTab): void => {
    if (!isCenterCollapsed && center === tab) {
      // 再點一次亮著的分頁 → 關掉編輯器欄（終端跟著打開）
      setLayout((l) => ({ ...l, centerCollapsed: true, termHidden: false }))
      return
    }
    setCenter(tab)
    expandCenter()
  }
  // 頂端列拖曳排序：終端圖示排在編輯器組前面 → 終端欄在左（右側停靠）或在上（底部停靠）
  const termFirst = !isVibe && Boolean(layout.termFirst)
  const editorGroupRef = useRef<HTMLDivElement>(null)
  const [dragging, setDragging] = useState<'editor' | 'terminal' | null>(null)
  const onHeaderDragOver = (e: React.DragEvent): void => {
    if (!dragging) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'move'
    const box = editorGroupRef.current?.getBoundingClientRect()
    if (!box) return
    // 拖曳中即時套用：游標在編輯器組中線左邊 → 被拖的那個排到左邊
    const leftOfEditor = e.clientX < box.left + box.width / 2
    const nextTermFirst = dragging === 'terminal' ? leftOfEditor : !leftOfEditor
    if (nextTermFirst !== termFirst) setLayout((l) => ({ ...l, termFirst: nextTermFirst }))
  }

  // 上限依目前視窗算，避免把中央區擠沒了
  const maxLeft = (): number =>
    Math.min(LIMITS.leftMax, window.innerWidth - LIMITS.centerMin - (isBottom ? 0 : layout.rightW))
  // 開發模式：終端可以拖到只替編輯器留 editorMin；Vibe 模式維持原本上限
  const maxRight = (): number =>
    isVibe
      ? Math.min(LIMITS.rightMax, window.innerWidth - LIMITS.centerMin - (isLeftCollapsed ? 0 : layout.leftW))
      : Math.max(LIMITS.rightMin, window.innerWidth - LIMITS.editorMin - 1 - (isLeftCollapsed ? 0 : layout.leftW))
  // 38px 是頂端列
  const maxTerm = (): number => Math.max(LIMITS.termMin, window.innerHeight - 38 - LIMITS.editorMin - 1)

  const nudgeLeft = (dx: number): void =>
    setLayout((l) => {
      const target = (l.leftCollapsed ? 0 : l.leftW) + dx
      if (target < 90) {
        return { ...l, leftCollapsed: true, leftW: 0 }
      }
      return {
        ...l,
        leftCollapsed: false,
        leftW: clamp(target, LIMITS.leftMin, maxLeft())
      }
    })

  // 右欄在右側，往右拖代表把它縮小；終端在左（Vibe 模式或拖到前面）時方向相反
  const nudgeRight = (dx: number): void =>
    setLayout((l) => ({
      ...l,
      rightW: clamp(l.rightW + (isVibe || termFirst ? dx : -dx), LIMITS.rightMin, maxRight())
    }))
  // 終端在底部，往下拖代表把它縮小；終端在上方時方向相反
  const nudgeTerm = (dy: number): void =>
    setLayout((l) => ({ ...l, termH: clamp(l.termH + (termFirst ? dy : -dy), LIMITS.termMin, maxTerm()) }))

  const setDock = (dock: Dock): void => setLayout((l) => ({ ...l, dock }))

  const effectiveLeftW = isLeftCollapsed ? 0 : layout.leftW
  const sp1W = isLeftCollapsed ? '0px' : '1px'

  const gridStyle: React.CSSProperties = isCenterMaximized
    ? {
        gridTemplateColumns: '1fr',
        gridTemplateRows: '1fr',
        gridTemplateAreas: `"center"`
      }
    : isVibe
    ? {
        gridTemplateColumns: `56px 0px 0px ${layout.rightW}px 1px 1fr`,
        gridTemplateRows: '1fr',
        gridTemplateAreas: `"rail left sp1 term sp2 center"`
      }
    : isTermHidden
    ? isBottom
      ? {
          gridTemplateColumns: `${effectiveLeftW}px ${sp1W} 1fr`,
          gridTemplateRows: termFirst ? '0px 0px 1fr' : '1fr 0px 0px',
          gridTemplateAreas: termFirst
            ? `"left sp1 term" "left sp1 sp2" "left sp1 center"`
            : `"left sp1 center" "left sp1 sp2" "left sp1 term"`
        }
      : {
          gridTemplateColumns: termFirst
            ? `${effectiveLeftW}px ${sp1W} 0px 0px 1fr`
            : `${effectiveLeftW}px ${sp1W} 1fr 0px 0px`,
          gridTemplateRows: '1fr',
          gridTemplateAreas: termFirst ? `"left sp1 term sp2 center"` : `"left sp1 center sp2 term"`
        }
    : isBottom && termFirst
    ? {
        gridTemplateColumns: `${effectiveLeftW}px ${sp1W} 1fr`,
        gridTemplateRows: isCenterCollapsed ? '1fr 0px 0px' : `${layout.termH}px 1px 1fr`,
        gridTemplateAreas: `"left sp1 term" "left sp1 sp2" "left sp1 center"`
      }
    : isBottom
    ? {
        gridTemplateColumns: `${effectiveLeftW}px ${sp1W} 1fr`,
        gridTemplateRows: isCenterCollapsed ? '0px 0px 1fr' : `1fr 1px ${layout.termH}px`,
        gridTemplateAreas: `"left sp1 center" "left sp1 sp2" "left sp1 term"`
      }
    : termFirst
    ? {
        gridTemplateColumns: isCenterCollapsed
          ? `${effectiveLeftW}px ${sp1W} 1fr 0px 0px`
          : `${effectiveLeftW}px ${sp1W} ${layout.rightW}px 1px 1fr`,
        gridTemplateRows: '1fr',
        gridTemplateAreas: `"left sp1 term sp2 center"`
      }
    : {
        gridTemplateColumns: isCenterCollapsed
          ? `${effectiveLeftW}px ${sp1W} 0px 0px 1fr`
          : `${effectiveLeftW}px ${sp1W} 1fr 1px ${layout.rightW}px`,
        gridTemplateRows: '1fr',
        gridTemplateAreas: `"left sp1 center sp2 term"`
      }

  return (
    <div className="app">
      {/* 頂層統整式無縫視窗 Header（整合 Windows/macOS 標題列與全域控制項） */}
      <header className="app-unified-header">
        <div className="app-header-left">
          <span className="app-brand">
            Agent Workbench<span className="dot">●</span>
          </span>
          {!isLeftCollapsed && (
            <button
              className="btn-collapse-left"
              onClick={() => setLayout((l) => ({ ...l, leftCollapsed: true }))}
              title={t('header.collapseSidebar')}
            >
              <IconSidebarCollapse size={13} />
            </button>
          )}
          {isLeftCollapsed && !isCenterMaximized && !isVibe && (
            <button
              className="btn-expand-left"
              onClick={() =>
                setLayout((l) => ({
                  ...l,
                  leftCollapsed: false,
                  leftW: l.leftW >= LIMITS.leftMin ? l.leftW : DEFAULT_LAYOUT.leftW
                }))
              }
              title={t('header.expandSidebar')}
            >
              <IconSidebarExpand size={13} />
              <span>{t('header.sidebar')}</span>
            </button>
          )}
        </div>

        <div className="app-header-center" onDragOver={onHeaderDragOver} onDrop={(e) => e.preventDefault()}>
          {!isVibe && !isCowork && (
          <>
          {/* 編輯器組與終端圖示可互相拖曳換位；順序就是下方兩欄的左右（或上下）位置 */}
          <div
            ref={editorGroupRef}
            className={`header-drag-item ${dragging === 'editor' ? 'dragging' : ''}`}
            style={{ order: termFirst ? 2 : 1 }}
            draggable
            title={t('header.dragToReorder')}
            onDragStart={(e) => {
              e.dataTransfer.effectAllowed = 'move'
              e.dataTransfer.setData('text/plain', 'editor')
              setDragging('editor')
            }}
            onDragEnd={() => setDragging(null)}
          >
            <div className="segmented">
              {(['editor', 'preview', 'memory', 'browser'] as const).map((tab) => (
                <button
                  key={tab}
                  className={!isCenterCollapsed && center === tab ? 'on' : ''}
                  title={!isCenterCollapsed && center === tab ? t('header.hideEditor') : undefined}
                  onClick={() => onEditorTab(tab)}
                >
                  {t(`header.${tab}`)}
                </button>
              ))}
            </div>
          </div>
          <button
            className={`btn-icon header-drag-item header-term-btn ${isTermHidden ? '' : 'active'} ${dragging === 'terminal' ? 'dragging' : ''}`}
            style={{ order: termFirst ? 1 : 2 }}
            draggable
            aria-pressed={!isTermHidden}
            title={isTermHidden ? t('header.showTerminal') : t('header.hideTerminal')}
            onClick={toggleTerminal}
            onDragStart={(e) => {
              e.dataTransfer.effectAllowed = 'move'
              e.dataTransfer.setData('text/plain', 'terminal')
              setDragging('terminal')
            }}
            onDragEnd={() => setDragging(null)}
          >
            <IconTerminalBox size={14} />
          </button>
          </>
          )}
        </div>

        <div className="app-header-right">
          {!isVibe && detectedDevUrl && center !== 'browser' && (
            <button
              className="dev-url-chip"
              title={t('vibe.openDevUrl')}
              onClick={() => openInBrowser(detectedDevUrl.url)}
            >
              ▶ {detectedDevUrl.url.replace(/^http:\/\//, '')}
            </button>
          )}
          {/* Focus / Maximize Center View（Vibe 模式版面已精簡，不提供） */}
          {!isVibe && (
            <button
              className={`btn-icon ${isCenterMaximized ? 'active' : ''}`}
              title={isCenterMaximized ? t('header.exitFocus') : t('header.focusWorkspace')}
              onClick={toggleCenterMaximized}
            >
              {isCenterMaximized ? <IconMinimize size={14} /> : <IconMaximize size={14} />}
            </button>
          )}

          {!isVibe && (
          <div className="segmented dock-switch">
            <button
              className={!isBottom ? 'on' : ''}
              onClick={() => setDock('right')}
              title={t('header.dockRight')}
            >
              <IconDockRight size={13} />
            </button>
            <button
              className={isBottom ? 'on' : ''}
              onClick={() => setDock('bottom')}
              title={t('header.dockBottom')}
            >
              <IconDockBottom size={13} />
            </button>
          </div>
          )}

          <RemoteIndicator />
          <ProxyStatusPill onClick={() => { setProxySettingsOpen(true); openSettings() }} />

          {/* Vibe 模式的設定鈕在左側圖示列最下方 */}
          {!isVibe && (
            <button
              className="btn-icon"
              title={t('header.settingsTooltip')}
              onClick={() => openSettings()}
            >
              <IconSettings size={14} />
            </button>
          )}
        </div>
      </header>

      {/* 工作區（各欄起始 Y 座標完全齊平，底線絕對水平對齊） */}
      {isCowork && (
        <Suspense fallback={<div className="cv-root" />}>
          <CanvasView />
        </Suspense>
      )}
      {/* Cowork 模式只把版面藏起來，不卸載：終端與編輯器狀態都留著（§2.4） */}
      <div className={`workarea dock-${isVibe ? 'vibe' : layout.dock}`} style={isCowork ? { ...gridStyle, display: 'none' } : gridStyle}>
        <aside
          className="col col-left"
          style={{
            gridArea: 'left',
            display: isLeftCollapsed ? 'none' : 'flex',
            minWidth: isLeftCollapsed ? 0 : LIMITS.leftMin
          }}
        >
          <div className="tabbar" hidden={isVibe}>
            <div className="segmented left-segmented">
              <button className={sidebarTab === 'dashboard' ? 'on' : ''} onClick={() => setSidebarTab('dashboard')}>
                {t('sidebarTabs.dashboard')}
              </button>
              <button className={sidebarTab === 'files' ? 'on' : ''} onClick={() => setSidebarTab('files')}>
                {t('sidebarTabs.files')}
              </button>
              <button className={sidebarTab === 'git' ? 'on' : ''} onClick={() => setSidebarTab('git')}>
                {t('sidebarTabs.git')}
              </button>
            </div>
          </div>
          <div className="panel-body">
            {(isVibe || visitedSidebarTabs.has('dashboard')) && (
              <div hidden={activeSidebarTab !== 'dashboard'} className="fill">
                <DashboardPanel />
              </div>
            )}
            {visitedSidebarTabs.has('files') && (
              <div hidden={activeSidebarTab !== 'files'} className="fill">
                <FileTreePanel />
              </div>
            )}
            {visitedSidebarTabs.has('git') && (
              <div hidden={activeSidebarTab !== 'git'} className="fill">
                <GitPanel />
              </div>
            )}
          </div>
        </aside>

        {!isLeftCollapsed && (
          <Splitter
            axis="vertical"
            label="Sidebar width"
            area="sp1"
            onDelta={nudgeLeft}
            onReset={() =>
              setLayout((l) => ({
                ...l,
                leftCollapsed: false,
                leftW: DEFAULT_LAYOUT.leftW
              }))
            }
          />
        )}

        <main
          className="col col-center"
          style={{ gridArea: 'center', display: isCenterCollapsed ? 'none' : 'flex' }}
        >
          {/* Vibe：成品／Preview／程式碼分頁放在成果欄自己的頂端（靠左），不佔 header 正中 */}
          {isVibe && (
            <div className="tabbar vibe-center-tabs">
              <div className="segmented">
                <button className={center === 'browser' ? 'on' : ''} onClick={() => setCenter('browser')}>
                  {t('vibe.app')}
                </button>
                <button className={center === 'preview' ? 'on' : ''} onClick={() => setCenter('preview')}>
                  {t('header.preview')}
                </button>
                <button className={center === 'editor' ? 'on' : ''} onClick={() => setCenter('editor')}>
                  {t('vibe.code')}
                </button>
              </div>
            </div>
          )}
          <div className="panel-body">
            {visitedCenterTabs.has('editor') && (
              <div hidden={center !== 'editor'} className="fill">
                <EditorPanel />
              </div>
            )}
            {visitedCenterTabs.has('preview') && (
              <div hidden={center !== 'preview'} className="fill">
                <PreviewPanel />
              </div>
            )}
            {visitedCenterTabs.has('memory') && (
              <div hidden={center !== 'memory'} className="fill">
                <MemoryPanel />
              </div>
            )}
            {browserOpened && (
              <div
                className="fill"
                style={{
                  display: center === 'browser' ? 'flex' : 'none',
                  height: '100%',
                  width: '100%'
                }}
              >
                <TestBrowserPanel
                  onClose={isVibe ? undefined : () => setCenter('editor')}
                  idleUntilRequested={isVibe}
                />
              </div>
            )}
          </div>
        </main>

        {!isCenterMaximized && !isCenterCollapsed && !isTermHidden && (
          <Splitter
            axis={isBottom ? 'horizontal' : 'vertical'}
            label={isBottom ? 'Terminal height' : 'Terminal width'}
            area="sp2"
            onDelta={isBottom ? nudgeTerm : nudgeRight}
            onReset={() =>
              setLayout((l) =>
                isBottom
                  ? { ...l, termH: DEFAULT_LAYOUT.termH }
                  : { ...l, rightW: DEFAULT_LAYOUT.rightW }
              )
            }
          />
        )}

        <section
          className={`col col-term ${isVibe && agentBusy ? 'agent-busy' : ''}`}
          style={{
            gridArea: 'term',
            display: isCenterMaximized || isTermHidden ? 'none' : 'flex',
            minWidth: !isBottom ? LIMITS.rightMin : 0
          }}
        >
          {isVibe && <VibeUsageBar />}
          <div className="panel-body">
            <div className="fill">
              <TerminalPanel />
            </div>
          </div>
        </section>

        {isVibe && !isCenterMaximized && <VibeRail onOpenCode={() => setCenter('editor')} />}
      </div>

      <SettingsModal
        isOpen={settingsModal.isOpen}
        onClose={() => { setProxySettingsOpen(false); closeSettings() }}
        initialTab={proxySettingsOpen ? 'proxy' : settingsModal.tab}
      />
    </div>
  )
}
