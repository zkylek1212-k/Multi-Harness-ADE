import { app, ipcMain, shell, BrowserWindow } from 'electron'
import { loadSettings } from './settings'
import type { UpdaterStatus, UpdateInfo } from '../../preload/index'

const RELEASES = 'https://github.com/zkylek1212-k/Multi-Harness-ADE/releases'
// ponytail: scans 100 recent releases; paginate if Windows history hides the Linux release.
const API = 'https://api.github.com/repos/zkylek1212-k/Multi-Harness-ADE/releases?per_page=100'

// Linux releases stay prereleases so Windows /releases/latest never selects them.
export function selectLinuxRelease(releases: unknown, currentVersion: string): UpdateInfo | undefined {
  if (!Array.isArray(releases)) throw new Error('Invalid GitHub releases response')
  const compare = (a: string, b: string): number => {
    const left = a.split('.').map(Number), right = b.split('.').map(Number)
    for (let i = 0; i < 3; i++) {
      if (left[i] !== right[i]) return left[i] - right[i]
    }
    return 0
  }
  let newest: UpdateInfo | undefined
  for (const release of releases) {
    const match = typeof release?.tag_name === 'string' && /^linux-v(\d+\.\d+\.\d+)$/.exec(release.tag_name)
    if (!match || release.draft || !release.prerelease) continue
    if (!Array.isArray(release.assets) || !release.assets.some((asset: { name?: string }) =>
      typeof asset?.name === 'string' && /^Agent-Workbench-Linux-.+-x64\.(AppImage|deb|tar\.gz)$/.test(asset.name))) continue
    const version = match[1]
    if (compare(version, currentVersion) <= 0 || (newest && compare(version, newest.version) <= 0)) continue
    newest = {
      version,
      releaseName: typeof release.name === 'string' ? release.name : `Linux ${version}`,
      releaseNotes: typeof release.body === 'string' ? release.body : undefined,
      releaseDate: typeof release.published_at === 'string' ? release.published_at : undefined,
      downloadUrl: `${RELEASES}/tag/${release.tag_name}`
    }
  }
  return newest
}

export function registerUpdaterHandlers(): void {
  const status: UpdaterStatus = {
    currentVersion: app.getVersion(), isPackaged: app.isPackaged, isInstalled: false,
    checking: false, updateAvailable: false, updateDownloaded: false, isDownloading: false
  }
  const broadcast = (): void => {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed() && !win.webContents.isDestroyed()) win.webContents.send('updater:statusChange', status)
    }
  }
  const check = async (): Promise<UpdaterStatus> => {
    if (status.checking) return status
    status.checking = true
    status.error = undefined
    broadcast()
    try {
      const response = await fetch(API, {
        headers: { 'User-Agent': 'Agent-Workbench-Linux' }, signal: AbortSignal.timeout(8000)
      })
      if (!response.ok) throw new Error(`GitHub update check failed (${response.status})`)
      status.updateInfo = selectLinuxRelease(await response.json(), status.currentVersion)
      status.updateAvailable = !!status.updateInfo
    } catch (err) {
      status.error = err instanceof Error ? err.message : String(err)
    } finally {
      status.checking = false
      broadcast()
    }
    return status
  }
  const openRelease = async (customUrl?: string): Promise<void> => {
    const fallback = status.updateInfo?.downloadUrl || RELEASES
    const url = customUrl || fallback
    await shell.openExternal(/^https?:\/\//i.test(url) ? url : fallback)
  }
  ipcMain.handle('updater:getStatus', () => status)
  ipcMain.handle('updater:check', check)
  ipcMain.handle('updater:download', async () => { await openRelease(); return true })
  ipcMain.handle('updater:install', () => {})
  ipcMain.handle('updater:openRelease', (_event, url?: string) => openRelease(url))
  setTimeout(() => {
    if (app.isPackaged && loadSettings().autoCheckUpdates !== false) void check()
  }, 5000)
}
