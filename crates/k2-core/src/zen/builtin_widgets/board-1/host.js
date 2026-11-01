// BoardHost: everything Board needs from the page it runs in, other than storage (that is store.js).
// Here the host is a K2 Garden widget. To run Board somewhere else (a K2 tab, a plain web page),
// replace this file:
//   host.features              {boards, forAgents, chat}: the Garden-only parts of the UI (the board
//                              switcher and projects, the "For agents" panel, the Chat column steering)
//   host.start()               → Promise; resolves once the host is ready
//   host.ready()               tell the host the first frame is drawn
//   host.onScheme(cb)          cb('light' | 'dark' | null): null = guess from CSS
//   host.reducedMotion()       → boolean
//   host.setHitRegions(rects)  screen rects of floating controls (Garden: lets clicks through K2's
//                              window-drag strip)
//   host.openLink(url)         → Promise<'opened' | 'declined' | 'busy' | 'unavailable'>: an http(s)
//                              address in the person's browser, right after their click
//   host.canDownload() / host.download(name, data, {mime})   save a file to the person's Downloads
//                              (data: a string, bytes, a Blob, or an async function that makes them)
//   host.canPick() / host.pick({accept})   K2's file picker → {files: [{ref, name, mime, size}]}
//   host.onDrop(cb)            cb({files, x, y}) for files dropped on the board (already stored)
(function (g) {
  'use strict'
  const k2 = g.k2
  const schemeOf = (v) => { const t = v && (v.theme || v); return (t && (t.scheme || (t.vars && t.vars.scheme))) || null }
  const may = (verb) => !!k2 && typeof k2.can === 'function' && k2.can(verb)
  g.BoardHost = {
    features: { boards: true, forAgents: true, chat: true },
    start: () => (k2 ? k2.connected : Promise.resolve()),
    ready() { if (k2) k2.ready() },
    onScheme(cb) {
      if (!k2) return
      k2.theme.get().then((v) => cb(schemeOf(v)), () => cb(null))
      k2.theme.changed((v) => cb(schemeOf(v)))
    },
    reducedMotion: () => !!(k2 && k2.motion && k2.motion.reduced),
    async openLink(url) {
      if (!/^https?:\/\/[^\s]+$/i.test(String(url)) || !k2 || typeof k2.open !== 'function') return 'unavailable'
      try { const r = await k2.open(String(url)); return r && r.ok === false ? 'declined' : 'opened' } catch (e) { return e && e.code === 'rate_limited' ? 'busy' : 'unavailable' }
    },
    setHitRegions(rects) {
      if (!k2) return
      k2.canvas.setHitRegions(rects.map((r) => ({ x: r.x, y: r.y, width: r.width, height: r.height }))).catch(() => {})
    },
    canDownload: () => !!k2 && typeof k2.download === 'function' && may('downloads.save'),
    download: (name, data, opts) => k2.download(name, data, opts),
    canPick: () => !!k2 && typeof k2.pick === 'function' && may('uploads.pick'),
    pick: (opts) => k2.pick(opts),
    onDrop(cb) { if (k2 && typeof k2.onDrop === 'function' && may('uploads.dropped')) return k2.onDrop(cb); return () => {} },
  }
})(window)
