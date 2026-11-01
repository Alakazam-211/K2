// A stand-in `k2` for the built-in Board widget (k2:board@1), shared by the JSDOM suite
// (src/renderer/lib/contract/builtin-board.test.ts) and the Playwright specs (tests/board/*.spec.ts).
// It follows K2's Garden data contract: revs, ifRev conflicts, all-or-nothing batch and tx, subscribe =
// the whole state under a prefix first, then each change; blobs are content-addressed (sha256 of the bytes).
//   window.makeFakeK2(opts) → { k2, space, log, saves, blobs, agentWrite, resync, putBytes, drop, ... }
// opts: seed [[key, value]], saves [[key, value]], viewer (name), agents (rows), grantees (items | 'refuse'
// | undefined = no verb), caps (verbs k2.can answers true for), denyStore (code), picks ([{files}]),
// echoDelay (ms), quiet (no live echoes), urlOf(ref, px, bytes, mime) (an img src for a stored file).
;(function (g) {
  'use strict'
  g.makeFakeK2 = function makeFakeK2(opts) {
    opts = opts || {}
    const clone = (v) => (v == null ? v : JSON.parse(JSON.stringify(v)))
    const K2E = (code) => Object.assign(new Error(code), { name: 'K2Error', code })
    const space = new Map(); const feed = new Map(); let head = 1; const subs = []
    const saves = new Map(opts.saves || [])
    const blobs = new Map()   // ref → {bytes, mime, name}
    const log = { batches: [], txs: [], shown: [], contexts: [], drafts: [], opened: [], toggles: [], layoutSets: [], downloads: [], blobPuts: [], blobGets: [], urls: [], hits: [], links: [], ready: 0, grantees: 0 }
    const ME = 'you via k2:board'
    for (const [k, v] of opts.seed || []) space.set(k, { key: k, rev: 1, value: clone(v), by: 'agent:helper', updatedAt: 0 })
    const deny = () => { if (opts.denyStore) throw K2E(opts.denyStore) }
    const seedQueue = []

    function deliver(items) {
      if (opts.quiet) return
      for (const s of subs) {
        const mine = items.filter((i) => i.key.indexOf(s.prefix) === 0)
        if (mine.length) setTimeout(() => s.cb(clone(mine)), opts.echoDelay || 0)
      }
    }
    function apply(ops, by) {
      for (const o of ops) { const e = space.get(o.key); if (o.ifRev != null && (e ? e.rev : 0) !== o.ifRev) throw K2E('conflict') }
      head++
      const items = []
      for (const o of ops) {
        if (o.op === 'delete') {
          if (space.delete(o.key)) { items.push({ key: o.key, rev: head, value: null, by }); feed.set(o.key, { key: o.key, rev: head, at: Date.now() / 1000, by, deleted: true }) }
        } else {
          space.set(o.key, { key: o.key, rev: head, value: clone(o.value), by, updatedAt: Date.now() / 1000 })
          items.push({ key: o.key, rev: head, value: clone(o.value), by })
          feed.set(o.key, { key: o.key, rev: head, at: Date.now() / 1000, by, deleted: false })
        }
      }
      deliver(items)
      return { rev: head, ops: ops.length }
    }

    async function bytesOf(data) {
      if (data instanceof Uint8Array) return data
      if (data instanceof ArrayBuffer) return new Uint8Array(data)
      if (typeof data === 'string') return new TextEncoder().encode(data)
      if (data && typeof data.arrayBuffer === 'function') return new Uint8Array(await data.arrayBuffer())
      return new Promise((resolve, reject) => { const r = new g.FileReader(); r.onload = () => resolve(new Uint8Array(r.result)); r.onerror = () => reject(r.error); r.readAsArrayBuffer(data) })
    }
    async function sha256(bytes) {
      const d = new Uint8Array(await g.crypto.subtle.digest('SHA-256', bytes))
      return Array.from(d, (b) => b.toString(16).padStart(2, '0')).join('')
    }
    const sniff = (b) => (b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47 ? 'image/png' : b[0] === 0xff && b[1] === 0xd8 ? 'image/jpeg' : b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46 ? 'image/gif' : 'application/octet-stream')
    async function putBytes(data, name) {
      const bytes = await bytesOf(data)
      if (bytes.length > 16 * 1024 * 1024) throw K2E('too_large')
      const ref = 'k2blob:sha256-' + await sha256(bytes)
      const isNew = !blobs.has(ref)
      blobs.set(ref, { bytes, mime: sniff(bytes), name: name == null ? null : String(name) })
      return { ref, mime: sniff(bytes), name: name == null ? null : String(name), size: bytes.length, new: isNew }
    }

    const store = {
      async get(key) { deny(); return clone(space.get(key) || null) },
      async list(o) {
        deny()
        const keys = [...space.keys()].filter((k) => k.indexOf((o && o.prefix) || '') === 0).sort().filter((k) => !(o && o.after) || k > o.after)
        const lim = (o && o.limit) || 3   // small pages: the widget must follow `next`
        return { items: keys.slice(0, lim).map((k) => clone(space.get(k))), next: keys.length > lim ? keys[lim - 1] : null }
      },
      async changes(o) {
        deny()
        const all = [...feed.values()].filter((e) => e.rev > ((o && o.since) || 0) && e.key.indexOf((o && o.prefix) || '') === 0).sort((x, y) => x.rev - y.rev)
        const items = all.slice(0, 2).map(clone)
        return { head, items, next: all.length > 2 ? { since: items[items.length - 1].rev } : null }
      },
      async batch(ops) {
        deny(); log.batches.push(clone(ops)); if (ops.length > 64) throw K2E('too_large')
        // raceSeed: two windows' first-board batches arrive before either lands (a real race, made certain).
        if (opts.raceSeed && ops.some((o) => o.key === 'seeded')) {
          await new Promise((resolve) => { seedQueue.push(resolve); if (seedQueue.length === 2) seedQueue.forEach((f) => f()) })
        }
        return apply(ops, ME)
      },
      tx() {
        const staged = []
        return {
          async add(ops) { deny(); if (ops.length > 64) throw K2E('too_large'); staged.push(...ops); return { ops: staged.length } },
          async commit() { log.txs.push(clone(staged)); return apply(staged, ME) },
          async abort() { return { aborted: true } },
        }
      },
      subscribe(prefix, cb, onErr) {
        if (opts.denyStore) { setTimeout(() => onErr && onErr(K2E(opts.denyStore))); return () => {} }
        const s = { prefix, cb }; subs.push(s)
        setTimeout(() => cb([...space.values()].filter((e) => e.key.indexOf(prefix) === 0).map(clone)))
        return () => subs.splice(subs.indexOf(s), 1)
      },
    }
    if (opts.grantees !== undefined) {
      store.grantees = async () => { log.grantees++; if (opts.grantees === 'refuse') throw K2E('not_granted'); return { items: clone(opts.grantees) } }
    }
    const caps = opts.caps || ['store.get', 'store.list', 'store.batch', 'store.changes', 'blobs.put', 'blobs.get', 'downloads.save', 'uploads.pick', 'uploads.dropped', 'agents.list', 'agents.show', 'conversation.setContext', 'links.open'].concat(opts.grantees !== undefined ? ['store.grantees'] : [])
    let dropCb = null
    const urlOf = opts.urlOf || ((ref, px) => 'blob:fake/' + ref.slice(-12) + (px ? '?px=' + px : ''))
    const agents = opts.agents || [
      { address: 'helper::local', label: 'Helper', server: null },
      { address: 'writer::local', label: 'Writer', server: null },
      { address: 'far::elsewhere', label: 'Far away', server: 'elsewhere' },
    ]
    let columns = opts.columns || [{ index: 0, label: 'Board', collapsible: false, collapsed: false }, { index: 1, label: 'Chat', collapsible: true, collapsed: true }]
    const layoutSubs = []
    const k2 = {
      connected: Promise.resolve(), config: {}, widget: { id: 'k2:board@1', name: 'Board', garden: 'g-test0001' }, motion: { reduced: true },
      can: (verb) => caps.includes(verb),
      ready() { log.ready++ },
      viewer: async () => ({ kind: 'owner', name: opts.viewer === undefined ? 'Ada' : opts.viewer }),
      theme: { get: async () => ({ theme: { scheme: 'light' } }), changed() { return () => {} } },
      canvas: { setHitRegions: async (r) => { log.hits.push(r) } },
      save: { async get(k) { return clone(saves.has(k) ? saves.get(k) : null) }, async set(k, v) { saves.set(k, clone(v)) } },
      agents: { list: async () => clone(agents), async show(a) { log.shown.push(clone(a)) } },
      conversation: { async open(a) { log.opened.push(a) }, async setContext(t) { log.contexts.push(t) } },
      compose: { async draft(a, t) { log.drafts.push([a, t]) } },
      layout: {
        async get() { return { columns: clone(columns) } },
        async set(o) { log.layoutSets.push(clone(o)); columns = columns.map((c) => (c.index === o.column ? Object.assign({}, c, { collapsed: !!o.collapsed }) : c)) },
        async toggle(i) { log.toggles.push(i); columns = columns.map((c) => (c.index === i ? Object.assign({}, c, { collapsed: !c.collapsed }) : c)); for (const f of layoutSubs) f({ columns: clone(columns) }) },
        subscribe(cb) { layoutSubs.push(cb); return () => {} },
      },
      store,
      blobs: {
        async put(data, o) { const r = await putBytes(data, o && o.name); log.blobPuts.push(r); return r },
        async get(ref, o) { log.blobGets.push([ref, o || null]); const b = blobs.get(ref); if (!b) throw K2E('not_found'); return { ref, mime: b.mime, name: b.name, size: b.bytes.length, blob: new g.Blob([b.bytes], { type: b.mime }) } },
        async url(ref, o) { const b = blobs.get(ref); if (!b) throw K2E('not_found'); const u = urlOf(ref, o && o.px, b.bytes, b.mime); log.urls.push([ref, (o && o.px) || 0]); return u },
      },
      async open(url) { log.links.push(url); return { ok: true } },
      async download(name, data, o) {
        const bytes = typeof data === 'function' ? await data() : data
        log.downloads.push({ name, data: bytes, mime: o && o.mime })
        return { saved: true, name }
      },
      async pick() { return (opts.picks && opts.picks.shift()) || { files: [] } },
      onDrop(cb) { dropCb = cb; return () => { dropCb = null } },
    }
    return {
      k2, space, log, saves, blobs,
      head: () => head,
      // An agent (or another window) writes: the widget hears it live.
      agentWrite(ops, handle) { return apply(ops.map((o) => Object.assign({ op: 'put' }, o)), 'agent:' + (handle || 'helper')) },
      otherWindow(ops) { return apply(ops.map((o) => Object.assign({ op: 'put' }, o)), ME) },
      // A write the widget doesn't hear about (no echo).
      sneak(key, value, by) { head++; space.set(key, { key, rev: head, value: clone(value), by: by || 'agent:helper', updatedAt: 0 }); return head },
      resync(prefix) { for (const s of subs) if (s.prefix === prefix) s.cb({ resync: true, items: [...space.values()].filter((e) => e.key.indexOf(prefix) === 0).map(clone) }) },
      putBytes,
      drop(files, x, y) { if (!dropCb) throw new Error('the widget never asked for drops'); dropCb({ files, x, y }) },
    }
  }
})(typeof window !== 'undefined' ? window : globalThis)
