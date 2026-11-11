// One way to write a date and time on the Logs pages (Access Audit,
// Heartbeats): "Nov 10, 5:39 PM" in the viewer's own time zone. Each page
// keeps the exact stamp in the opened record.

function toDate(value: string | number | Date | null | undefined): Date | null {
  if (value === null || value === undefined || value === '') return null
  const d = value instanceof Date ? value : new Date(value)
  return Number.isNaN(d.getTime()) ? null : d
}

/** "Nov 10, 5:39 PM" (local). Falls back to the raw text, or an em dash. */
export function localWhen(value: string | number | Date | null | undefined): string {
  const d = toDate(value)
  if (!d) return typeof value === 'string' && value ? value : '—'
  return d.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

/** "Nov 10, 2026, 5:39:26 PM" (local): the same format with the year and
 *  seconds, for an opened record. */
export function localWhenFull(value: string | number | Date | null | undefined): string {
  const d = toDate(value)
  if (!d) return typeof value === 'string' && value ? value : '—'
  return d.toLocaleString([], { year: 'numeric', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit', second: '2-digit' })
}
