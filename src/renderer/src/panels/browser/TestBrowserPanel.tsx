import { useEffect, useRef, useState } from 'react'
import { IconClose } from '@/components/Icons'
import { Webview } from '@/components/Webview'
import { useTranslation } from '@/i18n'
import { useWorkbench } from '@/store'
import './browser.css'

type ViewportMode = 'full' | 'tablet' | 'mobile'

const QUICK_PORTS = [
  { label: ':5173 (Vite)', port: '5173' },
  { label: ':3000 (React/Next)', port: '3000' },
  { label: ':8080 (Dev)', port: '8080' },
  { label: ':8000 (Python)', port: '8000' }
]

interface TestBrowserPanelProps {
  onClose?: () => void
  /** Vibe 模式：還沒偵測到 dev server 前不去開 5173，顯示「尚無執行中的 App」空狀態 */
  idleUntilRequested?: boolean
}

export default function TestBrowserPanel({ onClose, idleUntilRequested = false }: TestBrowserPanelProps): JSX.Element {
  const { t } = useTranslation()
  const [url, setUrl] = useState(idleUntilRequested ? '' : 'http://localhost:5173')
  const [inputVal, setInputVal] = useState(idleUntilRequested ? '' : 'http://localhost:5173')
  const [isLoading, setIsLoading] = useState(false)
  const [loadError, setLoadError] = useState<{ code: number; desc: string; url: string } | null>(null)
  const [viewport, setViewport] = useState<ViewportMode>('full')
  const [isLandscape, setIsLandscape] = useState(false)
  const webviewRef = useRef<any>(null)

  const normalizeUrl = (raw: string): string => {
    let trimmed = raw.trim()
    if (!trimmed) return 'http://localhost:5173'
    if (/^\d{2,5}$/.test(trimmed)) return `http://localhost:${trimmed}`
    if (!/^https?:\/\//i.test(trimmed)) trimmed = `http://${trimmed}`
    return trimmed
  }

  const navigate = (newUrl: string): void => {
    setLoadError(null)
    const target = normalizeUrl(newUrl)
    setUrl(target)
    setInputVal(target)
  }

  // 其他面板（終端偵測到 dev server、Vibe 狀態列）請求開啟網址
  const { browserRequest } = useWorkbench()
  useEffect(() => {
    if (browserRequest) navigate(browserRequest.url)
  }, [browserRequest?.nonce])

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'Enter') {
      navigate(inputVal)
    }
  }

  const goBack = (): void => {
    try {
      setLoadError(null)
      if (webviewRef.current && typeof webviewRef.current.goBack === 'function') {
        webviewRef.current.goBack()
      }
    } catch {
      /* ignore */
    }
  }

  const goForward = (): void => {
    try {
      setLoadError(null)
      if (webviewRef.current && typeof webviewRef.current.goForward === 'function') {
        webviewRef.current.goForward()
      }
    } catch {
      /* ignore */
    }
  }

  const reload = (): void => {
    setLoadError(null)
    try {
      if (webviewRef.current && typeof webviewRef.current.reload === 'function') {
        webviewRef.current.reload()
      } else {
        // Fallback: re-trigger url
        const current = url
        setUrl('')
        setTimeout(() => setUrl(current), 50)
      }
    } catch {
      /* ignore */
    }
  }

  const openExternal = (): void => {
    try {
      window.open(url, '_blank')
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    const el = webviewRef.current
    if (!el) return

    const onStart = (): void => {
      setIsLoading(true)
      setLoadError(null)
    }
    const onStop = (): void => {
      setIsLoading(false)
      try {
        if (typeof el.getURL === 'function') {
          const u = el.getURL()
          if (u && u !== 'about:blank') {
            setInputVal(u)
          }
        }
      } catch {
        /* ignore */
      }
    }
    const onFail = (e: any): void => {
      setIsLoading(false)
      if (e.errorCode && e.errorCode !== -3) {
        setLoadError({
          code: e.errorCode,
          desc: e.errorDescription || 'Connection refused',
          url: e.validatedURL || url
        })
      }
    }

    el.addEventListener('did-start-loading', onStart)
    el.addEventListener('did-stop-loading', onStop)
    el.addEventListener('did-navigate', onStop)
    el.addEventListener('did-fail-load', onFail)

    return () => {
      el.removeEventListener('did-start-loading', onStart)
      el.removeEventListener('did-stop-loading', onStop)
      el.removeEventListener('did-navigate', onStop)
      el.removeEventListener('did-fail-load', onFail)
    }
  }, [url])

  // Viewport dimensions
  let frameWidth = '100%'
  let frameHeight = '100%'
  if (viewport === 'tablet') {
    frameWidth = isLandscape ? '1024px' : '768px'
    frameHeight = isLandscape ? '768px' : '1024px'
  } else if (viewport === 'mobile') {
    frameWidth = isLandscape ? '844px' : '390px'
    frameHeight = isLandscape ? '390px' : '844px'
  }

  return (
    <div className="browser-root">
      {/* Browser Navigation Toolbar */}
      <div className="browser-toolbar">
        <div className="browser-nav-group">
          <button className="browser-btn" onClick={goBack} title="Back">
            ←
          </button>
          <button className="browser-btn" onClick={goForward} title="Forward">
            →
          </button>
          <button
            className={`browser-btn ${isLoading ? 'spinning' : ''}`}
            onClick={reload}
            title="Reload"
          >
            ↻
          </button>
          <button
            className="browser-btn"
            onClick={() => navigate('http://localhost:5173')}
            title="Home"
          >
            ⌂
          </button>
        </div>

        {/* Address Input Bar */}
        <div className="browser-address-container">
          <input
            className="browser-address-input"
            type="text"
            value={inputVal}
            onChange={(e) => setInputVal(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Enter URL or port (e.g. 5173 or http://localhost:3000)..."
          />
          {isLoading && <span className="browser-loading-pill">Loading...</span>}
        </div>

        {/* Quick Localhost Ports Selector */}
        <div className="browser-port-select-wrap">
          <select
            className="browser-port-select"
            value={QUICK_PORTS.find((p) => url.includes(`:${p.port}`))?.port || ''}
            onChange={(e) => {
              if (e.target.value) {
                navigate(`http://localhost:${e.target.value}`)
              }
            }}
            title="Jump to standard local dev port"
          >
            <option value="" disabled>
              Ports ▾
            </option>
            {QUICK_PORTS.map((p) => (
              <option key={p.port} value={p.port}>
                {p.label}
              </option>
            ))}
          </select>
        </div>

        {/* Viewport Presets & External */}
        <div className="browser-viewports">
          <select
            className="browser-port-select browser-viewport-select"
            value={viewport}
            onChange={(e) => setViewport(e.target.value as ViewportMode)}
            title="Viewport"
          >
            <option value="full">Desktop</option>
            <option value="tablet">Tablet</option>
            <option value="mobile">Mobile</option>
          </select>
          <div className="segmented browser-viewport-segmented">
            <button
              className={viewport === 'full' ? 'on' : ''}
              onClick={() => setViewport('full')}
              title="Responsive 100% full viewport"
            >
              Desktop
            </button>
            <button
              className={viewport === 'tablet' ? 'on' : ''}
              onClick={() => setViewport('tablet')}
              title="Tablet viewport (768px)"
            >
              Tablet
            </button>
            <button
              className={viewport === 'mobile' ? 'on' : ''}
              onClick={() => setViewport('mobile')}
              title="Mobile viewport (390px)"
            >
              Mobile
            </button>
          </div>

          {viewport !== 'full' && (
            <button
              className="browser-btn"
              onClick={() => setIsLandscape(!isLandscape)}
              title="Toggle landscape / portrait orientation"
            >
              ↻
            </button>
          )}

          <button
            className="browser-btn"
            onClick={openExternal}
            title="Open in external system browser"
          >
            ↗
          </button>

          {onClose && (
            <button
              className="browser-btn browser-close-btn"
              onClick={onClose}
              title="Close Browser view (Return to Editor)"
            >
              <IconClose size={11} />
              <span className="browser-close-text">Exit</span>
            </button>
          )}
        </div>
      </div>

      {/* Browser Viewport Stage */}
      <div className={`browser-stage ${viewport !== 'full' ? 'device-mode' : ''}`}>
        <div
          className={`browser-frame-wrapper ${viewport !== 'full' ? 'device-frame' : ''}`}
          style={{ width: frameWidth, height: frameHeight }}
        >
          {url ? (
            <>
              <Webview
                ref={webviewRef}
                src={url}
                className="browser-webview"
                allowpopups={true}
                webpreferences="contextIsolation=true"
              />
              {loadError && (
                <div className="browser-error-overlay">
                  <div className="browser-error-card">
                    <div className="browser-error-icon">
                      <svg
                        width="36"
                        height="36"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.8"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                      >
                        <circle cx="12" cy="12" r="10" />
                        <line x1="2" y1="12" x2="22" y2="12" />
                        <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
                      </svg>
                    </div>

                    <h3 className="browser-error-title">
                      {loadError.url.includes('localhost') || loadError.url.includes('127.0.0.1')
                        ? t('browser.serverNotRunning')
                        : t('browser.cannotConnect')}
                    </h3>

                    <p className="browser-error-desc">
                      {loadError.url.includes('localhost') || loadError.url.includes('127.0.0.1')
                        ? t('browser.serverNotRunningDesc', { url: loadError.url })
                        : t('browser.cannotConnectDesc', { url: loadError.url })}
                    </p>

                    <div className="browser-error-actions">
                      <button
                        type="button"
                        className="browser-action-pill primary"
                        onClick={reload}
                      >
                        ↻ {t('browser.retry')}
                      </button>
                      <button
                        type="button"
                        className="browser-action-pill"
                        onClick={openExternal}
                      >
                        ↗ {t('browser.openExternal')}
                      </button>
                    </div>

                    {(loadError.url.includes('localhost') || loadError.url.includes('127.0.0.1')) && (
                      <div className="browser-quick-ports-section">
                        <span className="browser-quick-ports-label">
                          {t('browser.tryPorts')}
                        </span>
                        <div className="browser-quick-ports-list">
                          {QUICK_PORTS.map((p) => (
                            <button
                              key={p.port}
                              type="button"
                              className="browser-port-chip"
                              onClick={() => navigate(`http://localhost:${p.port}`)}
                            >
                              {p.label}
                            </button>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="browser-empty-state">
              <h3>{idleUntilRequested ? t('vibe.noAppTitle') : 'No URL loaded'}</h3>
              <p>
                {idleUntilRequested
                  ? t('vibe.noAppDesc')
                  : 'Type a URL or select a port above to start testing.'}
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
