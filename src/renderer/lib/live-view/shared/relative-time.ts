// How long ago something happened, as the live views say it (Activity,
// Changes, Messages, the timeline's rows). It steps up as time goes: "now",
// "N s ago", "N min ago" (under an hour), "N h ago" (under a day),
// "yesterday", "N days ago" (under a week), then the date, short ("9 Nov",
// with the year when it isn't this one). A time ahead of the clock reads as
// now. (The app's compact "2m ago" is for lists elsewhere; the live views
// keep their words.)

const SHORT = (timeZone?: string, year = false) => new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', ...(year ? { year: 'numeric' } : {}), timeZone })

/** `at` and `now` in ms; `timeZone` for the date (default: the machine's). */
export function liveAgo(at: number, now: number, timeZone?: string): string {
  const s = Math.floor(Math.max(0, now - at) / 1000)
  if (s < 2) return 'now'
  if (s < 60) return `${s} s ago`
  const min = Math.floor(s / 60)
  if (min < 60) return `${min} min ago`
  const h = Math.floor(s / 3600)
  if (h < 24) return `${h} h ago`
  const days = Math.floor(s / 86400)
  if (days === 1) return 'yesterday'
  if (days < 7) return `${days} days ago`
  const yearOf = (t: number): string => new Intl.DateTimeFormat('en-GB', { year: 'numeric', timeZone }).format(new Date(t))
  return SHORT(timeZone, yearOf(at) !== yearOf(now)).format(new Date(at))
}
