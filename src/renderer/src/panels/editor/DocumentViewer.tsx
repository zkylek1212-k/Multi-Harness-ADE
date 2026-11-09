/// <reference path="../../../../preload/index.d.ts" />
import { useState, useEffect, useCallback } from 'react'
import { openSettings, useWorkbench } from '@/store'
import { Webview } from '@/components/Webview'
import {
  IconExternalLink,
  IconFolderOpen,
  IconSettings,
  IconCheck,
  IconFileText
} from '@/components/Icons'
import './documentViewer.css'
import type { FileStat, WorkbenchSettings } from '../../../../preload/index'

export type DocType = 'word' | 'excel' | 'powerpoint' | 'pdf'

interface DocMeta {
  type: DocType
  label: string
  extLabel: string
  badgeColor: string
  iconLetter: string
  appDefaultName: string
}

export function getDocumentType(filePath: string): DocType | null {
  const ext = filePath.split('.').pop()?.toLowerCase() ?? ''
  if (ext === 'docx' || ext === 'doc') return 'word'
  if (ext === 'xlsx' || ext === 'xls') return 'excel'
  if (ext === 'pptx' || ext === 'ppt') return 'powerpoint'
  if (ext === 'pdf') return 'pdf'
  return null
}

export function isDocumentFile(filePath: string): boolean {
  return getDocumentType(filePath) !== null
}

const DOC_META: Record<DocType, DocMeta> = {
  word: {
    type: 'word',
    label: 'Microsoft Word Document',
    extLabel: 'DOCX / DOC',
    badgeColor: '#185ABD',
    iconLetter: 'W',
    appDefaultName: 'Microsoft Word'
  },
  excel: {
    type: 'excel',
    label: 'Microsoft Excel Spreadsheet',
    extLabel: 'XLSX / XLS',
    badgeColor: '#107C41',
    iconLetter: 'X',
    appDefaultName: 'Microsoft Excel'
  },
  powerpoint: {
    type: 'powerpoint',
    label: 'Microsoft PowerPoint Presentation',
    extLabel: 'PPTX / PPT',
    badgeColor: '#D83B01',
    iconLetter: 'P',
    appDefaultName: 'Microsoft PowerPoint'
  },
  pdf: {
    type: 'pdf',
    label: 'Portable Document Format (PDF)',
    extLabel: 'PDF',
    badgeColor: '#E5252A',
    iconLetter: 'PDF',
    appDefaultName: 'Default PDF Reader'
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`
}

function formatDate(iso: string): string {
  try {
    const d = new Date(iso)
    return d.toLocaleString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    })
  } catch {
    return iso
  }
}

interface DocumentViewerProps {
  filePath: string
}

export default function DocumentViewer({ filePath }: DocumentViewerProps): JSX.Element {
  const workbench = useWorkbench()
  const docType = getDocumentType(filePath) || 'pdf'
  const meta = DOC_META[docType]

  const [stat, setStat] = useState<FileStat | null>(null)
  const [settings, setSettings] = useState<WorkbenchSettings | null>(null)
  const [isLaunching, setIsLaunching] = useState(false)
  const [toast, setToast] = useState<{ type: 'ok' | 'err'; message: string } | null>(null)
  const [pdfEmbedded, setPdfEmbedded] = useState(docType === 'pdf')

  // Load stats and settings
  useEffect(() => {
    let active = true

    window.api.files
      .stat(filePath)
      .then((s) => {
        if (active) setStat(s)
      })
      .catch(() => {})

    window.api.settings
      .get()
      .then((cfg) => {
        if (active) setSettings(cfg)
      })
      .catch(() => {})

    setToast(null)
    setPdfEmbedded(docType === 'pdf')

    return () => {
      active = false
    }
  }, [filePath, docType, workbench.settingsTick])

  const fileName = filePath.split(/[/\\]/).pop() ?? filePath
  const configuredTool = settings?.docToolPaths?.[docType]?.trim() || ''
  const hasCustomTool = Boolean(configuredTool)

  const toolDisplayName = hasCustomTool
    ? configuredTool.split(/[/\\]/).pop() || configuredTool
    : meta.appDefaultName

  const showToast = useCallback((type: 'ok' | 'err', message: string): void => {
    setToast({ type, message })
    setTimeout(() => {
      setToast(null)
    }, 4000)
  }, [])

  // Launch handlers
  const handleLaunch = async (useCustom: boolean): Promise<void> => {
    setIsLaunching(true)
    try {
      const tool = useCustom && hasCustomTool ? configuredTool : undefined
      const res = await window.api.files.openExternal(filePath, tool)
      if (res.ok) {
        showToast(
          'ok',
          useCustom && hasCustomTool
            ? `Opened in ${toolDisplayName} ✓`
            : `Opened in System Default Application ✓`
        )
      } else {
        showToast('err', res.error || 'Failed to open document')
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err)
      showToast('err', `Launch failed: ${msg}`)
    } finally {
      setIsLaunching(false)
    }
  }

  const handleReveal = async (): Promise<void> => {
    try {
      await window.api.files.showInFolder(filePath)
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err)
      showToast('err', `Could not reveal in folder: ${msg}`)
    }
  }

  const handleOpenSettings = (): void => {
    openSettings('doctools')
  }

  return (
    <div className="doc-viewer-container">
      {/* Top Action Toolbar */}
      <div className="doc-viewer-toolbar">
        <div className="doc-viewer-toolbar-left">
          <span
            className="doc-viewer-badge"
            style={{ backgroundColor: meta.badgeColor }}
          >
            {meta.iconLetter}
          </span>
          <span className="doc-viewer-filename" title={filePath}>
            {fileName}
          </span>
          {stat && (
            <span className="doc-viewer-meta-text">
              ({formatBytes(stat.size)})
            </span>
          )}
        </div>

        <div className="doc-viewer-toolbar-actions">
          {docType === 'pdf' && (
            <button
              type="button"
              className="doc-viewer-btn secondary"
              onClick={() => setPdfEmbedded((p) => !p)}
              title={pdfEmbedded ? 'Switch to Information Card' : 'Switch to Embedded Preview'}
            >
              <IconFileText size={13} />
              <span>{pdfEmbedded ? 'Info Card' : 'Preview'}</span>
            </button>
          )}

          <button
            type="button"
            className="doc-viewer-btn primary"
            onClick={() => handleLaunch(true)}
            disabled={isLaunching}
            title={hasCustomTool ? `Open with ${configuredTool}` : `Open with System Default Application`}
          >
            <IconExternalLink size={13} />
            <span>Open in {toolDisplayName}</span>
          </button>

          {hasCustomTool && (
            <button
              type="button"
              className="doc-viewer-btn secondary"
              onClick={() => handleLaunch(false)}
              disabled={isLaunching}
              title="Open with OS Default Application"
            >
              <span>System Default</span>
            </button>
          )}

          <button
            type="button"
            className="doc-viewer-btn secondary"
            onClick={handleReveal}
            title="Reveal file in Windows Explorer"
          >
            <IconFolderOpen size={13} />
            <span>Explorer</span>
          </button>

          <button
            type="button"
            className="doc-viewer-btn secondary"
            onClick={handleOpenSettings}
            title="Configure tool paths in Settings"
          >
            <IconSettings size={13} />
            <span>Settings ⚙</span>
          </button>
        </div>
      </div>

      {/* Main View Area */}
      {docType === 'pdf' && pdfEmbedded ? (
        <div className="doc-pdf-view-wrapper">
          <Webview
            src={`file://${encodeURI(filePath.replace(/\\/g, '/'))}`}
            className="doc-pdf-frame"
          />
        </div>
      ) : (
        <div className="doc-viewer-body">
          <div className="doc-card-wrapper">
            <div
              className="doc-card-icon-container"
              style={{ backgroundColor: meta.badgeColor }}
            >
              {meta.iconLetter}
            </div>

            <div className="doc-card-title">{fileName}</div>
            <div className="doc-card-subtitle">{meta.label}</div>

            <div className="doc-card-details">
              <div className="doc-card-detail-row">
                <span className="doc-detail-label">File Type:</span>
                <span className="doc-detail-value">{meta.extLabel}</span>
              </div>
              <div className="doc-card-detail-row">
                <span className="doc-detail-label">File Size:</span>
                <span className="doc-detail-value">
                  {stat ? `${formatBytes(stat.size)} (${stat.size.toLocaleString()} bytes)` : 'Calculating…'}
                </span>
              </div>
              <div className="doc-card-detail-row">
                <span className="doc-detail-label">Last Modified:</span>
                <span className="doc-detail-value">
                  {stat ? formatDate(stat.mtime) : '…'}
                </span>
              </div>
              <div className="doc-card-detail-row">
                <span className="doc-detail-label">Active Opener:</span>
                <span
                  className={`doc-detail-value ${hasCustomTool ? 'tool-active' : ''}`}
                  title={hasCustomTool ? configuredTool : 'System Default Application'}
                >
                  {hasCustomTool ? `Custom: ${toolDisplayName}` : 'System Default Application'}
                </span>
              </div>
              <div className="doc-card-detail-row">
                <span className="doc-detail-label">File Path:</span>
                <span className="doc-detail-value" title={filePath}>
                  {filePath}
                </span>
              </div>
            </div>

            <div className="doc-card-actions">
              <button
                type="button"
                className="doc-btn-launch-primary"
                onClick={() => handleLaunch(true)}
                disabled={isLaunching}
              >
                <IconExternalLink size={15} />
                <span>
                  {isLaunching ? 'Opening…' : `Open with ${toolDisplayName}`}
                </span>
              </button>

              <div className="doc-card-subactions">
                {hasCustomTool && (
                  <button
                    type="button"
                    className="doc-btn-sub"
                    onClick={() => handleLaunch(false)}
                    disabled={isLaunching}
                  >
                    System Default
                  </button>
                )}

                <button
                  type="button"
                  className="doc-btn-sub"
                  onClick={handleReveal}
                >
                  <IconFolderOpen size={13} />
                  <span>Reveal in Explorer</span>
                </button>

                <button
                  type="button"
                  className="doc-btn-sub"
                  onClick={handleOpenSettings}
                >
                  <IconSettings size={13} />
                  <span>Configure Opener…</span>
                </button>
              </div>
            </div>

            {toast && (
              <div className={`doc-viewer-toast ${toast.type}`}>
                {toast.type === 'ok' && <IconCheck size={13} />}
                <span>{toast.message}</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
