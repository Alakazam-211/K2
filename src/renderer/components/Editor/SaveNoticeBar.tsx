// The editor's save notices on the shell's notice bar (NoticeBar's anatomy
// and tones, `.lv-notice`), in the stacked layout of the notes' v3: the tone's
// icon, a bold headline over one muted line, then the actions, and an × for a
// notice you can put away. At 800 px the actions wrap below the words.

import { CircleXIcon, Undo2Icon } from 'lucide-react'
import { Button, cx, Spinner } from '@/components/ui'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { CloseGlyph, HistoryGlyph, InfoGlyph, OfflineGlyph, OkGlyph, WarnGlyph } from '@/components/LiveView/shell/glyphs'
import type { SaveActionId, SaveNoticeView } from '@/lib/editor/save-model/notices'

const ICON: Record<SaveNoticeView['icon'], React.ReactNode> = {
  bad: <CircleXIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" focusable="false" />,
  history: <HistoryGlyph size={16} />,
  info: <InfoGlyph size={16} />,
  warn: <WarnGlyph size={16} />,
  ok: <OkGlyph size={16} />,
  offline: <OfflineGlyph size={16} />,
}

const UNDO: readonly string[] = ['undo-restore', 'undo-use-theirs', 'undo-theirs', 'undo', 'confirm-undo-theirs']

export function SaveNoticeBar<A extends string = SaveActionId>({ view, kind, onAction }: { view: SaveNoticeView<A>; kind: string; onAction: (id: NoInfer<A> | 'dismiss') => void }): React.JSX.Element {
  return (
    <div role={view.role} data-testid="ed-save-notice" data-kind={kind} data-tone={view.tone} data-k2-chamfer="surface" className="lv-notice ed-save-notice mx-2.5 mt-2 flex flex-none flex-wrap items-center gap-x-3 gap-y-2 rounded-[var(--radius-box)] border px-3 py-2">
      <span aria-hidden="true" className="lv-notice-icon inline-flex flex-none">
        {view.busy ? <Spinner /> : ICON[view.icon]}
      </span>
      <p className="type-row m-0 flex min-w-[240px] flex-1 flex-col text-[var(--color-text-primary)]">
        <b className="font-semibold">{view.title}</b>
        {view.line ? <span className="text-[var(--color-text-secondary)]">{view.line}</span> : null}
      </p>
      <div data-close-only={view.actions.length === 0 ? '' : undefined} className="ed-save-actions ml-auto flex flex-none flex-wrap items-center gap-1.5">
        {view.actions.map((a) => (
          <Button
            key={a.id}
            variant={a.primary ? 'accent' : 'default'}
            title={a.hint}
            aria-disabled={a.disabled || undefined}
            onClick={() => (a.disabled ? undefined : onAction(a.id))}
            className={cx('inline-flex items-center gap-1.5', a.primary ? '' : 'text-[var(--color-text-primary)]')}
            data-action={a.id}
          >
            {UNDO.includes(a.id) ? <Undo2Icon size={12} strokeWidth={K2_ICON_STROKE} aria-hidden="true" focusable="false" /> : null}
            {a.label}
          </Button>
        ))}
        {view.closable ? (
          <button
            type="button"
            data-k2-focus=""
            aria-label="Dismiss"
            title="Dismiss"
            onClick={() => onAction('dismiss')}
            className="lv-icon-btn no-drag inline-flex h-[26px] w-[26px] cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)]"
          >
            <CloseGlyph size={14} />
          </button>
        ) : null}
      </div>
    </div>
  )
}
