// 0.46.15: the zone shown beside a heartbeat schedule's time.

/** The zone fields a heartbeat row carries (older daemons omit both). */
export interface HeartbeatZoneFields {
  /** The IANA zone the schedule's times are read in; null = the server's clock. */
  tz?: string | null
  /** The daemon's label: the zone, or `server clock (<the server's zone>)`. */
  zoneLabel?: string | null
}

/** The zone shown beside a schedule's time: the daemon's label (the row's
 *  zone, or `server clock (<the server's zone>)`). The time itself is the
 *  row's own wall-clock time, never converted to this device's zone. */
export function scheduleZoneLabel(row: HeartbeatZoneFields): string {
  return row.zoneLabel || row.tz || 'server clock'
}
