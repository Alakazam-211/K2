// The notice bar: one full-width bar above the hero, one at a time, with
// what happened and one next step (lib/live-view/shared/notices.ts has the
// words). The same anatomy for the outside saves, a refusal and every
// connection state: the tone's icon (or a spinner while it tries), the
// title in bold, the line after it, the actions at the end.

import { Button, cx, Spinner } from '@/components/ui'
import { noticeView, type LiveNotice, type NoticeActionId, type NoticeContext } from '@/lib/live-view/shared/notices'
import { CopyGlyph, HistoryGlyph, InfoGlyph, OfflineGlyph, OkGlyph, RetryGlyph, WarnGlyph } from './glyphs'

const ACTION_GLYPH: Partial<Record<NoticeActionId, React.ReactNode>> = {
  retry: <RetryGlyph size={12} />,
  'open-copy': <CopyGlyph size={12} />,
  history: <HistoryGlyph size={12} />,
}

function icon(n: LiveNotice, tone: 'info' | 'warn' | 'bad'): React.ReactNode {
  if (n.kind === 'connection' && (n.state === 'cant' || n.state === 'offline')) return <OfflineGlyph size={16} />
  if (n.kind === 'connection' && n.state === 'back') return <OkGlyph size={16} />
  return tone === 'info' ? <InfoGlyph size={16} /> : <WarnGlyph size={16} />
}

export function NoticeBar({
  notice,
  ctx,
  onAction,
}: {
  notice: LiveNotice | null
  ctx: NoticeContext
  onAction: (id: NoticeActionId, notice: LiveNotice) => void
}): React.JSX.Element | null {
  if (!notice) return null
  const v = noticeView(notice, ctx)
  return (
    <div
      role={v.role}
      data-testid="lv-notice"
      data-tone={v.tone}
      data-kind={notice.kind === 'connection' ? `connection:${notice.state}` : notice.kind === 'outside_save' ? `save:${notice.save.outcome}` : notice.kind === 'size' ? `size:${notice.size}` : notice.kind}
      data-k2-chamfer="surface"
      className="lv-notice mx-2.5 mt-2 flex flex-none flex-wrap items-center gap-x-2.5 gap-y-1.5 rounded-[var(--radius-box)] border px-3 py-2"
    >
      <span aria-hidden="true" className="lv-notice-icon inline-flex flex-none">
        {v.busy ? <Spinner /> : icon(notice, v.tone)}
      </span>
      <p className="type-row m-0 min-w-[240px] flex-1 text-[var(--color-text-primary)]">
        <b className="font-semibold">{v.title}</b>
        {v.line ? <span className="text-[var(--color-text-secondary)]"> {v.line}</span> : null}
      </p>
      <div className="ml-auto flex flex-none items-center gap-1.5">
        {v.actions.map((a) => (
          <Button
            key={a.id + a.label}
            variant={a.primary ? 'accent' : 'default'}
            disabled={a.disabled}
            aria-disabled={a.disabled || undefined}
            onClick={() => onAction(a.id, notice)}
            className={cx('inline-flex items-center gap-1.5', a.primary ? '' : 'text-[var(--color-text-primary)]')}
            data-action={a.id}
          >
            {a.disabled ? null : ACTION_GLYPH[a.id]}
            {a.label}
          </Button>
        ))}
      </div>
    </div>
  )
}
