import { SettingsIcon } from '@/components/icons/K2Icons'

/** Small settings cog used by workspace-drawer section headers. */
export function SectionManageCog({
  title,
  onClick,
}: {
  title: string
  onClick: () => void
}): React.JSX.Element {
  return (
    <span
      role="button"
      onClick={(e) => {
        e.stopPropagation()
        onClick()
      }}
      className="flex items-center justify-center p-0.5 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
      title={title}
    >
      <SettingsIcon size={12} />
    </span>
  )
}
