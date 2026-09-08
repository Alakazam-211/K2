/** Prefix bare hostnames with https:// so "example.com" just works.
 *  Loopback hosts (`localhost`, `127.0.0.1`, `::1`) get `http://`. */
export function normalizeUrl(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed) return ''
  if (/^https?:\/\//i.test(trimmed)) return trimmed
  const hostPort = trimmed.split(/[/?#]/, 1)[0]?.toLowerCase() ?? ''
  const isLoopback =
    hostPort === 'localhost' ||
    hostPort.startsWith('localhost:') ||
    hostPort === '127.0.0.1' ||
    hostPort.startsWith('127.0.0.1:') ||
    hostPort === '::1' ||
    hostPort === '[::1]' ||
    hostPort.startsWith('[::1]:')
  if (isLoopback) {
    if (hostPort === '::1') return `http://[::1]${trimmed.slice(3)}`
    return `http://${trimmed}`
  }
  return `https://${trimmed}`
}
