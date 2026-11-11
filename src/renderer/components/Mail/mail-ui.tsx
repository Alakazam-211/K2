// K2's kit for @k2/mail-reader (OA6): the reader draws K2's own shared
// controls and icon set. The package can't import the app (it imports only
// react and dompurify), so the host hands these in through `ui`.

import type { MailIconName, MailIconProps, MailReaderUi, MailShape } from '@k2/mail-reader'
import { Button, Input, Surface, Tag, type ButtonProps } from '@/components/ui'
import {
  AgentsIcon,
  ArchiveIcon,
  ArrowLeftIcon,
  CloseIcon,
  ComposeIcon,
  DownloadIcon,
  DraftsIcon,
  FlagIcon,
  FolderIcon,
  FolderMoveIcon,
  ForwardIcon,
  InboxIcon,
  MailIcon,
  MailOpenIcon,
  MoreIcon,
  PaperclipIcon,
  RefreshIcon,
  ReplyAllIcon,
  ReplyIcon,
  SendIcon,
  SpamIcon,
  StarIcon,
  TrashIcon,
  type K2IconProps,
} from '@/components/icons/K2Icons'

/** Every name the reader draws, to its K2 icon. */
export const MAIL_ICONS: Record<MailIconName, (p: K2IconProps) => React.JSX.Element> = {
  inbox: InboxIcon,
  sent: SendIcon,
  drafts: DraftsIcon,
  archive: ArchiveIcon,
  trash: TrashIcon,
  spam: SpamIcon,
  folder: FolderIcon,
  move: FolderMoveIcon,
  reply: ReplyIcon,
  'reply-all': ReplyAllIcon,
  forward: ForwardIcon,
  delete: TrashIcon,
  star: (p) => <StarIcon {...p} />,
  starred: (p) => <StarIcon {...p} filled />,
  flag: FlagIcon,
  attach: PaperclipIcon,
  file: DraftsIcon,
  more: MoreIcon,
  back: ArrowLeftIcon,
  refresh: RefreshIcon,
  compose: ComposeIcon,
  'mark-read': MailOpenIcon,
  'mark-unread': MailIcon,
  download: DownloadIcon,
  close: CloseIcon,
  agent: AgentsIcon,
}

function MailGlyph({ name, size = 14, className }: MailIconProps): React.JSX.Element {
  const Icon = MAIL_ICONS[name]
  return <Icon size={size} className={className} />
}

const VARIANT: Record<string, ButtonProps['variant']> = {
  primary: 'accent',
  default: 'default',
  quiet: 'quiet',
  danger: 'danger-quiet',
}

const SHAPE: Record<MailShape, Record<string, string>> = {
  // Cut corners in Terminal (and nothing elsewhere).
  control: { 'data-k2-chamfer': 'control' },
  chip: { 'data-k2-chamfer': 'chip' },
  card: { 'data-k2-chamfer': 'surface' },
  // A column: a tile in Glass (its own rim and radius), square in Terminal.
  pane: { 'data-material': 'tile', 'data-clip': '' },
}

export const K2_MAIL_UI: MailReaderUi = {
  Button: ({ variant = 'default', ...rest }) => <Button variant={VARIANT[variant] ?? 'default'} {...rest} />,
  Tag: (p) => <Tag {...p} />,
  SearchField: ({ className, ...rest }) => <Input variant="search" className={className} {...rest} />,
  TextField: ({ className, ...rest }) => <Input className={className} {...rest} />,
  Card: ({ raised, ...rest }) => <Surface chamfer material={raised ? 'raised' : 'tile'} role2={raised ? 'elevated' : 'surface'} {...rest} />,
  Icon: MailGlyph,
  shape: (kind) => SHAPE[kind],
}
