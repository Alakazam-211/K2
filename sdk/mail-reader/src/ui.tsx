// The reader's controls, supplied by the host (K2's own; not from OCmail).
//
// The package imports nothing but react and dompurify, so it can't reach the
// app's shared controls or its icon set. Instead the host hands in a small kit
// (`MailReader ui={…}`): a button, a tag, a search field, a text field, a
// card, an icon for each name in `MailIconName`, and `shape()`, the attributes
// the host's CSS keys its looks to (cut corners in Terminal, tiles and frost
// in Glass). K2 passes its `ui/` primitives and `K2Icons`
// (src/renderer/components/Mail/mail-ui.tsx).
//
// Without a kit the package still works: plain token-styled controls, and an
// icon button shows its label in words.

import { createContext, useContext, useMemo, type ComponentType } from 'react'

/** Every icon the reader draws. */
export type MailIconName =
  | 'inbox'
  | 'sent'
  | 'drafts'
  | 'archive'
  | 'trash'
  | 'spam'
  | 'folder'
  | 'move'
  | 'reply'
  | 'reply-all'
  | 'forward'
  | 'delete'
  | 'star'
  | 'starred'
  | 'flag'
  | 'attach'
  | 'file'
  | 'more'
  | 'back'
  | 'refresh'
  | 'compose'
  | 'mark-read'
  | 'mark-unread'
  | 'download'
  | 'close'
  | 'agent'

export const MAIL_ICON_NAMES: readonly MailIconName[] = [
  'inbox', 'sent', 'drafts', 'archive', 'trash', 'spam', 'folder', 'move', 'reply', 'reply-all', 'forward', 'delete',
  'star', 'starred', 'flag', 'attach', 'file', 'more', 'back', 'refresh', 'compose', 'mark-read', 'mark-unread', 'download',
  'close', 'agent',
]

export type MailButtonVariant = 'primary' | 'default' | 'quiet' | 'danger'

export interface MailButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: MailButtonVariant
  size?: 'sm' | 'md'
}

/** ok: passed; warn: neither passed nor failed (softfail, none, neutral);
 *  danger: failed. accent, muted and neutral are plain labels. */
export type MailTagTone = 'ok' | 'warn' | 'danger' | 'accent' | 'muted' | 'neutral'

export interface MailTagProps extends React.HTMLAttributes<HTMLSpanElement> {
  tone?: MailTagTone
}

export interface MailCardProps extends React.HTMLAttributes<HTMLDivElement> {
  /** `raised` sits a step above a tile (a card inside a pane). */
  raised?: boolean
}

export interface MailIconProps {
  name: MailIconName
  size?: number
  className?: string
}

/** Where a shape hook goes: a control (button, field), a chip (tag), a card,
 *  or a pane (one of the reader's columns). */
export type MailShape = 'control' | 'chip' | 'card' | 'pane'

export interface MailReaderUi {
  Button: ComponentType<MailButtonProps>
  Tag: ComponentType<MailTagProps>
  /** A search field; `className` sizes its box. */
  SearchField: ComponentType<React.InputHTMLAttributes<HTMLInputElement>>
  TextField: ComponentType<React.InputHTMLAttributes<HTMLInputElement>>
  Card: ComponentType<MailCardProps>
  /** null: no icons (icon buttons show their label in words). */
  Icon: ComponentType<MailIconProps> | null
  /** Attributes the host's CSS styles by (spread onto the element). */
  shape: (kind: MailShape) => Record<string, string>
}

const PLAIN_BUTTON: Record<MailButtonVariant, string> = {
  primary: 'bg-[var(--color-accent-fill)] border-[var(--color-accent-fill)] text-[var(--color-on-accent)]',
  default: 'bg-[var(--color-bg-elevated)] border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]',
  quiet: 'bg-transparent border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]',
  danger: 'bg-transparent border-[var(--color-border)] text-[var(--color-status-error-text)] hover:bg-[var(--color-bg-hover)]',
}

function PlainButton({ variant = 'default', size = 'sm', className = '', type = 'button', ...rest }: MailButtonProps): React.JSX.Element {
  return (
    <button
      type={type}
      className={`border rounded-[var(--radius-field)] whitespace-nowrap disabled:opacity-50 ${size === 'md' ? 'px-3.5 py-1.5' : 'px-3 py-1'} type-row ${PLAIN_BUTTON[variant]} ${className}`}
      {...rest}
    />
  )
}

const PLAIN_TAG: Record<MailTagTone, string> = {
  ok: 'text-[var(--color-status-ok)] border-[var(--color-status-ok)]',
  warn: 'text-[var(--color-status-warn-text)] border-[var(--color-status-warn-text)]',
  danger: 'text-[var(--color-status-error-text)] border-[var(--color-status-error-text)]',
  accent: 'text-[var(--color-accent)] border-[var(--color-accent)]',
  muted: 'text-[var(--color-text-muted)] border-[var(--color-border)]',
  neutral: 'text-[var(--color-text-secondary)] border-[var(--color-border)]',
}

function PlainTag({ tone = 'neutral', className = '', ...rest }: MailTagProps): React.JSX.Element {
  return <span data-tone={tone} className={`type-hint px-1.5 border rounded-[var(--radius-badge)] ${PLAIN_TAG[tone]} ${className}`} {...rest} />
}

const PLAIN_FIELD =
  'type-row px-2 py-1 border border-[var(--color-border)] rounded-[var(--radius-field)] bg-[var(--color-bg)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]'

function PlainSearch({ className = '', ...rest }: React.InputHTMLAttributes<HTMLInputElement>): React.JSX.Element {
  return <input type="search" className={`${PLAIN_FIELD} ${className}`} {...rest} />
}

function PlainText({ className = '', ...rest }: React.InputHTMLAttributes<HTMLInputElement>): React.JSX.Element {
  return <input className={`${PLAIN_FIELD} ${className}`} {...rest} />
}

function PlainCard({ raised, className = '', ...rest }: MailCardProps): React.JSX.Element {
  return (
    <div
      className={`border border-[var(--color-border)] rounded-[var(--radius-box)] ${raised ? 'bg-[var(--color-bg-elevated)]' : 'bg-[var(--color-bg-surface)]'} ${className}`}
      {...rest}
    />
  )
}

export const DEFAULT_MAIL_UI: MailReaderUi = {
  Button: PlainButton,
  Tag: PlainTag,
  SearchField: PlainSearch,
  TextField: PlainText,
  Card: PlainCard,
  Icon: null,
  shape: () => ({}),
}

const MailUiContext = createContext<MailReaderUi>(DEFAULT_MAIL_UI)

/** The host's kit for every view under it (MailReader sets this itself; use
 *  it directly to show a view such as DraftCard on its own). */
export function MailUiProvider({ ui, children }: { ui?: Partial<MailReaderUi>; children: React.ReactNode }): React.JSX.Element {
  const value = useMemo(() => ({ ...DEFAULT_MAIL_UI, ...ui }), [ui])
  return <MailUiContext.Provider value={value}>{children}</MailUiContext.Provider>
}

export function useMailUi(): MailReaderUi {
  return useContext(MailUiContext)
}

/** An icon, or nothing when the host gave none. */
export function MailIcon({ name, size, className }: MailIconProps): React.JSX.Element | null {
  const { Icon } = useMailUi()
  return Icon ? <Icon name={name} size={size} className={className} /> : null
}

/** A button that is an icon with a tooltip and an accessible name; with no
 *  icons it shows the label instead. `showLabel` keeps the word beside the
 *  icon (the main actions). */
export function IconButton({
  icon,
  label,
  showLabel = false,
  variant = 'quiet',
  className = '',
  pressed,
  ...rest
}: Omit<MailButtonProps, 'children'> & {
  icon: MailIconName
  label: string
  showLabel?: boolean
  pressed?: boolean
}): React.JSX.Element {
  const ui = useMailUi()
  const words = showLabel || !ui.Icon
  return (
    <ui.Button
      variant={variant}
      aria-label={label}
      aria-pressed={pressed}
      title={label}
      className={`inline-flex items-center justify-center gap-1.5 ${words ? '' : '!px-1.5'} ${className}`}
      {...rest}
    >
      {ui.Icon && <ui.Icon name={icon} size={14} />}
      {words && <span>{label}</span>}
    </ui.Button>
  )
}
