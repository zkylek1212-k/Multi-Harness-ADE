// 一個既有 PTY session 的畫面（終端模式的對話、接手終端）。行程屬於後端：卸下元件絕不 kill。
import { useEffect, useRef } from 'react'
import { useWorkbench } from '@/store'
import { createTerminalSession, xtermTheme, type TerminalSession } from '../terminal/terminalSession'

export default function TerminalPane({ ptyId, focus }: { ptyId: string; focus?: number }): JSX.Element {
  const { theme } = useWorkbench()
  const host = useRef<HTMLDivElement>(null)
  const session = useRef<TerminalSession | null>(null)

  useEffect(() => {
    if (!host.current) return
    const s = createTerminalSession({
      theme: xtermTheme(theme),
      minimumContrastRatio: theme.startsWith('light') ? 4.5 : 1,
      unsubscribe: (id) => window.api.pty.detach(id)
    })
    session.current = s
    s.attach(host.current)
    s.start({ ptyId })
    s.focus()
    return () => {
      session.current = null
      // ponytail: no detach on unmount (a late detach could undo a quick remount's attach); dispose stops drawing
      s.dispose()
    }
    // theme 另外套用，不重建 session
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ptyId])

  useEffect(() => {
    if (session.current) session.current.term.options.theme = xtermTheme(theme)
  }, [theme])

  useEffect(() => {
    if (focus) session.current?.focus()
  }, [focus])

  return <div ref={host} className="cw-term" />
}
