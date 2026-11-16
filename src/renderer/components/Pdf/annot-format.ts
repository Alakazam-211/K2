// How an annotation's date reads: "12 Nov 2026, 09:30" in this computer's
// time zone, 24-hour, as the live panels' clocks are.

const FMT = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false })

export const formatAnnotDate = (d: Date): string => FMT.format(d)
