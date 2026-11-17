// 浮在畫布上的審批卡（cowork-demo.jpg 的「Inline-Diff Approval Card」）：一個獨立節點，放在它所屬終端的下方。
// 結構化審批（hook）有改動前後的全文，畫成行內 diff；畫面辨識的審批只有螢幕尾段。
// 審批內容來自 Agent，一律當純文字顯示。
import { memo, useMemo, useState } from 'react'
import type { Node, NodeProps } from '@xyflow/react'
import { useTranslation } from '@/i18n'
import type { ApprovalFile, ApprovalRequest } from '../../../../shared/canvas'
import { lineDiff, withContext, type Row } from '../../../../shared/lineDiff'
import type { CanvasCall } from './CanvasView'

export type ApprovalNodeData = {
  nodeId: string
  channelId: string
  name: string
  approval: ApprovalRequest
  stopped: boolean
  call: CanvasCall
  onFocusTerminal: (nodeId: string) => void
}

const MAX_ROWS = 160

function FileDiff({ file }: { file: ApprovalFile }): JSX.Element {
  const rows = useMemo<Row[]>(() => {
    // Codex sends a patch instead of both texts: its own +/- lines are the diff
    if (file.patch) return file.patch.split('\n').map((l) => ({ kind: l.startsWith('+') ? '+' : l.startsWith('-') ? '-' : ' ', text: l.replace(/^[+\- ]/, '') }) as Row)
    return withContext(lineDiff(file.before ?? '', file.after ?? ''))
  }, [file])
  const shown = rows.slice(0, MAX_ROWS)
  return (
    <div className="cv-diffbox">
      <div className="cv-diff-file">
        <span className="cv-diff-path" title={file.path}>{file.path.split(/[\\/]/).pop()}</span>
        <span className="cv-add">+{file.added}</span>
        <span className="cv-del">-{file.removed}</span>
      </div>
      <div className="cv-diff-rows">
        {shown.map((r, i) =>
          r.kind === '…' ? (
            <div key={i} className="cv-diff-row gap">⋯</div>
          ) : (
            <div key={i} className={`cv-diff-row ${r.kind === '+' ? 'add' : r.kind === '-' ? 'del' : ''}`}>
              <span className="cv-diff-no">{r.a ?? ''}</span>
              <span className="cv-diff-no">{r.b ?? ''}</span>
              <span className="cv-diff-sign">{r.kind}</span>
              <span className="cv-diff-text">{r.text}</span>
            </div>
          )
        )}
        {rows.length > MAX_ROWS && <div className="cv-diff-row gap">⋯ {rows.length - MAX_ROWS}</div>}
      </div>
    </div>
  )
}

function ApprovalNode({ data }: NodeProps<Node<ApprovalNodeData>>): JSX.Element {
  const { t } = useTranslation()
  const { approval: a, call } = data
  const [busy, setBusy] = useState(false)
  const ids = { channelId: data.channelId, nodeId: data.nodeId, requestId: a.requestId, sessionGeneration: a.sessionGeneration }
  const act = async (op: 'node.approve' | 'node.reject'): Promise<void> => {
    setBusy(true)
    await call(op, ids)
    setBusy(false)
  }
  return (
    <div className="cv-appr nowheel">
      <div className="cv-appr-head">
        <span className="cv-appr-kind">{t(a.hook?.files?.length ? 'canvas.approvalCard' : 'canvas.approvalCardPlain')}</span>
        <span className="cv-appr-who">{data.name}</span>
      </div>
      <div className="cv-appr-title">{a.title}</div>
      <div className="cv-appr-body nodrag">
        {a.hook?.files?.map((f) => <FileDiff key={f.path} file={f} />)}
        {a.hook?.command && !a.hook.files?.length && <pre className="cv-appr-pre">{a.hook.command}</pre>}
        {!a.hook && <pre className="cv-appr-pre">{a.tail.trimEnd().split('\n').slice(-10).join('\n')}</pre>}
      </div>
      <div className="cv-appr-actions nodrag">
        {a.canApprove && !data.stopped && (
          <button className="cv-appr-btn ok" disabled={busy} onClick={() => void act('node.approve')}>
            {t('canvas.approve')}
          </button>
        )}
        {a.canReject && (
          <button className="cv-appr-btn no" disabled={busy} onClick={() => void act('node.reject')}>
            {t('canvas.reject')}
          </button>
        )}
        {/* 認不出問題與選項時不猜按鍵，只能回終端自己答（cowork2.0 §3.4） */}
        {!a.canApprove && !a.canReject && (
          <button className="cv-appr-btn" onClick={() => data.onFocusTerminal(data.nodeId)}>
            {t('canvas.answerInTerminal')}
          </button>
        )}
      </div>
    </div>
  )
}

export default memo(ApprovalNode)
