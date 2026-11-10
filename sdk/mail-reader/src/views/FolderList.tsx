// The folder list. Adapted from the folder part of OCmail's
// components/layout/sidebar.tsx (MIT, Copyright (c) 2025 Matthieu MALVACHE,
// Copyright (c) 2026 Alakazam Labs; see ../../NOTICE): role folders first,
// then named ones. No folder create/rename/delete, drag and drop, counts or
// quota here.

import { folderLabel, folderOrder, folderParam } from '../format'
import { t } from '../i18n'
import type { ReaderFolder } from '../types'

export function FolderList({
  folders,
  selected,
  onSelect,
}: {
  folders: ReaderFolder[]
  selected: string
  onSelect: (folder: string) => void
}): React.JSX.Element {
  return (
    <nav data-mr="folder-list" aria-label={t('folders')} className="flex flex-col py-1">
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
            className={`text-left px-3 py-1 type-row truncate hover:bg-[var(--color-bg-hover)] ${
              isSel ? 'bg-[var(--color-accent-soft)] text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'
            }`}
          >
            {folderLabel(f)}
          </button>
        )
      })}
    </nav>
  )
}
