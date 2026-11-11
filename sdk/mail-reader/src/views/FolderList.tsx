// The folder list. Adapted from the folder part of OCmail's
// components/layout/sidebar.tsx (MIT, Copyright (c) 2025 Matthieu MALVACHE,
// Copyright (c) 2026 Alakazam Labs; see ../../NOTICE): role folders first,
// then named ones. No folder create/rename/delete, drag and drop, counts or
// quota here. Each folder has its icon (role folders their own, named ones
// a plain folder); the rows are the Settings nav's: inset, the selector
// radius, the lit one raised.

import { folderLabel, folderOrder, folderParam, folderRole } from '../format'
import { t } from '../i18n'
import type { ReaderFolder } from '../types'
import { MailIcon, useMailUi, type MailIconName } from '../ui'

const ROLE_ICON: Record<string, MailIconName> = {
  inbox: 'inbox',
  sent: 'sent',
  drafts: 'drafts',
  archive: 'archive',
  trash: 'trash',
  junk: 'spam',
  spam: 'spam',
}

/** A folder's icon: its role's, else a plain folder. */
export function folderIcon(f: ReaderFolder): MailIconName {
  const r = folderRole(f)
  return (r && ROLE_ICON[r]) || 'folder'
}

/** A row in the sidebar (an inbox or a folder): inset, selector radius, the
 *  lit one on the raised fill. */
export const NAV_ROW =
  'mx-1.5 w-[calc(100%-12px)] border-transparent text-left rounded-[var(--radius-selector)] transition-colors no-drag cursor-pointer focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]'
export function navRowTone(selected: boolean): string {
  return selected
    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
    : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
}

export function FolderList({
  folders,
  selected,
  onSelect,
}: {
  folders: ReaderFolder[]
  selected: string
  onSelect: (folder: string) => void
}): React.JSX.Element {
  const ui = useMailUi()
  return (
    <nav data-mr="folder-list" aria-label={t('folders')} className="flex flex-col gap-px py-1.5">
      {folderOrder(folders).map((f) => {
        const value = folderParam(f)
        const isSel = selected.toLowerCase() === value.toLowerCase()
        return (
          <button
            key={f.name}
            type="button"
            data-mr="folder-row"
            data-testid={`mr-folder-${value}`}
            aria-current={isSel ? 'true' : undefined}
            onClick={() => onSelect(value)}
            {...ui.shape('control')}
            className={`${NAV_ROW} ${navRowTone(isSel)} flex items-center gap-2 px-2.5 py-1.5 type-control`}
          >
            <MailIcon name={folderIcon(f)} size={14} className={isSel ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-muted)]'} />
            <span className="truncate">{folderLabel(f)}</span>
          </button>
        )
      })}
    </nav>
  )
}
