// The Workbench's shared helpers (k2:work-inbox@1, k2:review@1; 0.46.7).
// Pure functions (no k2 calls) so they can be tested outside a frame:
// window.WB = {el, errText, ago, taskState, section, snoozePresets,
// itemKey, isSidecar, remote}. Every text goes in with textContent.
(function (root) {
  'use strict'

  function el(tag, cls, text) {
    var e = document.createElement(tag)
    if (cls) e.className = cls
    if (text !== undefined && text !== null) e.textContent = String(text)
    return e
  }

  function errText(e) {
    if (!e) return 'error'
    if (e.code === 'cap_not_granted') return 'this widget can’t do that here (' + (e.cap || e.code) + ')'
    if (e.code === 'verb_unavailable' || e.code === 'verb_local') return e.message || 'not available here yet'
    return e.message || e.code || String(e)
  }

  function ago(ms, now) {
    var s = Math.max(0, Math.round(((now || Date.now()) - ms) / 1000))
    if (s < 60) return 'now'
    if (s < 3600) return Math.floor(s / 60) + 'm'
    if (s < 86400) return Math.floor(s / 3600) + 'h'
    return Math.floor(s / 86400) + 'd'
  }

  function until(ms, now) {
    var s = Math.max(0, Math.round((ms - (now || Date.now())) / 1000))
    if (s < 3600) return Math.max(1, Math.ceil(s / 60)) + 'm'
    if (s < 86400) return Math.round(s / 3600) + 'h'
    return Math.round(s / 86400) + 'd'
  }

  // A row address `ws/name::host` is a sidecar; `ws::host` a workspace.
  function isSidecar(address) {
    var h = String(address || '').split('::')[0]
    return h.indexOf('/') > 0
  }

  function remote(address) {
    var i = String(address || '').indexOf('::')
    return i > 0 && address.slice(i + 2) !== 'local'
  }

  // Store keys: `item/<thread address>` (ws/name stays as two segments).
  function itemKey(thread) {
    return 'item/' + String(thread || '').toLowerCase().replace(/[^a-z0-9/_.-]/g, '-')
  }

  // What a task needs now, from K2's facts: the daemon's own taskState when
  // it has one (Lane W1), else cards waiting, the Thread turn, and git.
  //   needs-you · working · ready · idle · landed · retired
  function taskState(task, facts) {
    var t = task || {}
    var f = facts || {}
    if (t.retiredAt || t.taskState === 'retired') return 'retired'
    if (t.landedAt || t.taskState === 'landed') return 'landed'
    if (t.taskState === 'needs-you' || (f.waiting || 0) > 0 || f.turn === 'needs-you') return 'needs-you'
    if (t.taskState === 'working' || f.turn === 'working' || f.turn === 'monitoring') return 'working'
    if (t.taskState === 'ready') return 'ready'
    if (t.worktree && ((t.ahead || 0) > 0 || (t.dirty || 0) > 0)) return 'ready'
    return 'idle'
  }

  var LABEL = {
    'needs-you': 'Needs you',
    working: 'Working',
    ready: 'Ready to land',
    idle: 'Idle',
    landed: 'Landed',
    retired: 'Retired',
  }

  // Which inbox section a task sits in, with the person's own marks
  // (pin, snooze, hide, settle) from the widget's data.
  function section(state, mark, now) {
    var m = mark || {}
    if (m.hidden) return 'hidden'
    if (m.snoozeUntil && m.snoozeUntil > (now || Date.now())) return 'snoozed'
    if (state === 'landed' || state === 'retired' || m.settled) return 'settled'
    if (m.pinned) return 'pinned'
    if (state === 'needs-you') return 'needs-you'
    if (state === 'working') return 'working'
    if (state === 'ready') return 'ready'
    return 'idle'
  }

  // Snooze presets: label → the time it ends (ms).
  function snoozePresets(now) {
    var n = now || Date.now()
    var d = new Date(n)
    var tomorrow = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1, 9, 0, 0, 0).getTime()
    var days = (8 - d.getDay()) % 7 || 7
    var monday = new Date(d.getFullYear(), d.getMonth(), d.getDate() + days, 9, 0, 0, 0).getTime()
    return [
      { label: 'For 1 hour', at: n + 3600e3 },
      { label: 'For 4 hours', at: n + 4 * 3600e3 },
      { label: 'Until tomorrow 9:00', at: tomorrow },
      { label: 'Until Monday 9:00', at: monday },
    ]
  }

  root.WB = Object.freeze({
    el: el,
    errText: errText,
    ago: ago,
    until: until,
    isSidecar: isSidecar,
    remote: remote,
    itemKey: itemKey,
    taskState: taskState,
    LABEL: LABEL,
    section: section,
    snoozePresets: snoozePresets,
  })
})(typeof window !== 'undefined' ? window : globalThis)
