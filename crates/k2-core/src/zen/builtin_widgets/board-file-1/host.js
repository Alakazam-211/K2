// BoardHost for a .k5board in a K2 tab: everything Board needs from the tab it runs in, other than the file
// (that is store.js). The Garden's own host is board-1/host.js; the two fold together in @2.
//   host.features   what a tab leaves out: the switcher, For agents, Chat, Timer (a running timer isn't in the
//                   format), History and the activity pill (a file holds no updatedBy or log), and JSON export
//                   (that envelope stays the Garden's). Others as in board-1/host.js.
//   Keys: a sealed frame keeps every key it sees, so Cmd+S flushes here and K2's own tab keys are handed back.
(function (g) {
  'use strict'
  const k2 = g.k2
  const schemeOf = (v) => { const t = v && (v.theme || v); return (t && (t.scheme || (t.vars && t.vars.scheme))) || null }
  const may = (verb) => !!k2 && typeof k2.can === 'function' && k2.can(verb)
  // Keys K2's window handles: Cmd/Ctrl with a digit, T, W or N, Cmd+Shift+[ and ], and Ctrl+Tab.
  const ownKey = (e) => {
    const cmd = e.metaKey || e.ctrlKey
    if (e.ctrlKey && e.key === 'Tab') return true
    if (!cmd || e.altKey) return false
    if (/^Digit[1-9]$/.test(e.code)) return true
    if (e.shiftKey) return e.code === 'BracketLeft' || e.code === 'BracketRight'
    return e.code === 'KeyT' || e.code === 'KeyW' || e.code === 'KeyN'
  }
  g.BoardHost = {
    features: { boards: false, forAgents: false, chat: false, timer: false, history: false, activity: false, exportJson: false, fileTab: true },
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
    setHitRegions() {},   // a tab has no window-drag strip to let clicks through
    canDownload: () => !!k2 && typeof k2.download === 'function' && may('downloads.save'),
    download: (name, data, opts) => k2.download(name, data, opts),
    canPick: () => !!k2 && typeof k2.pick === 'function' && may('uploads.pick'),
    pick: (opts) => k2.pick(opts),
    onDrop() { return () => {} },
  }
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'KeyS') {
      e.preventDefault()
      if (g.BoardApp && g.BoardApp.endEdit) g.BoardApp.endEdit(true)
      if (g.BoardApp && g.BoardApp.store) g.BoardApp.store.flushNow()
      return
    }
    if (!ownKey(e) || !k2 || !k2.boardFile) return
    e.preventDefault(); e.stopPropagation()
    k2.boardFile.key({ key: e.key, code: e.code, meta: e.metaKey, ctrl: e.ctrlKey, shift: e.shiftKey }).catch(() => {})
  }, true)
})(window)
