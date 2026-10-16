// The name + agent count inside the two nav pickers (the Agents sidebar's
// focus-group dropdown and the Home picker), so both read the same: the
// count sits right after the name ("WORK 12"), not at the far edge. A long
// name truncates with an ellipsis; the count never shrinks, so it stays
// visible. The wrapper takes the free width, so the chevron / check stays
// at the right.

/** The picker button's label: name, then the count chip. */
export function PickerTriggerLabel({ name, count }: { name: string; count?: number }): React.JSX.Element {
  return (
    <span className="flex-1 min-w-0 flex items-center gap-1.5" data-picker-label="">
      <span
        className="min-w-0 truncate text-[11px] font-semibold text-[var(--color-text-primary)] uppercase tracking-wide"
        data-picker-name=""
      >
        {name}
      </span>
      {count !== undefined && (
        <span
          className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono"
          data-picker-count=""
        >
          {count}
        </span>
      )}
    </span>
  )
}

/** A menu row's label: name, then its count. */
export function PickerOptionLabel({ name, count }: { name: string; count?: number }): React.JSX.Element {
  return (
    <span className="flex-1 min-w-0 flex items-center gap-1.5" data-picker-label="">
      <span className="min-w-0 truncate" data-picker-name="">{name}</span>
      {count !== undefined && (
        <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)] tabular-nums" data-picker-count="">
          {count}
        </span>
      )}
    </span>
  )
}
