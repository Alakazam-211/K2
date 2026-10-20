/*! k2-thread 1.0.0 (FSL-1.1-Apache-2.0), K2's widget kit: a Thread panel. Copyright 2025-2026 Alakazam Labs. */
// K2Thread: talk to one of your agents inside a custom widget, in K2's
// Thread look, with one call.
//
//   manifest.json: "caps": ["agents:read", "thread:read", "thread:post"],
//                  "requires": { "libs": ["k2-thread@1"] }
//   k2.connected.then(() => {
//     const panel = K2Thread.mount(document.getElementById('thread'), {
//       look: k2.config.look || 'chamfer',   // 'chamfer' | 'glass'
//       address: k2.config.agent,            // 'handle::host'; or panel.open(address, label) later
//     })
//   })
//
// Two looks over one behaviour core:
//   - chamfer: K2's own Thread (the desktop Thread in K2's square Style,
//     components/Thread/ThreadZenKit + chamfer.ts): small 45° corner cuts
//     (bubbles 4px with the sender's corner sharp, the field 4px, Send 5px at
//     32px, the working tab 5px on top), in the Garden theme's colours
//     (--zen-*), with the frosted fade behind the message box.
//   - glass: K2 Companion's look (k2:kanban@2/@3's Thread panel): round
//     18px bubbles, a 20px field, a round Send, iOS system colours (the
//     phone's palette in dark), and the Gardens' mobile glass behind the
//     box: two blur layers instead of the desktop's four (it looks the same
//     on a strip this short and costs half).
// Both follow the Garden's light/dark (`data-zen-scheme` on the frame's
// root, which K2 keeps in step), never the computer's.
//
// The core (from k2:kanban@3's dock): bubbles, yours right and the agent's
// left, "author · time" under each; choice cards answered with a tap;
// Enter sends, Shift+Enter is a new line, one post at a time and the text
// stays until K2 took it; the working tab on the field's top edge (state,
// counters, clock; "Sent" until the agent picks it up); K2's stick-to-bottom
// scrolling: at the bottom a new message glides in, scrolled up nothing
// moves and a "N new messages ↓" pill counts them; a narrow panel (under
// 480px) lets a message use the whole width but a 12px inset.
// Attachments: a message's `doc.attachments` (when the Thread view carries
// them) draw as file chips, images inline from data: URLs;
// `renderAttachment(file, item)` and `onOpenFile(file, item)` take over.
// Markdown: with the `marked` and `dompurify` libraries loaded too, bodies
// render as sanitized markdown; `renderBody(node, text, item)` takes over.
// Agent text only ever goes in with textContent or DOMPurify.
;(function () {
  'use strict'
  if (window.K2Thread) return
  var d = document
  var LOOKS = ['chamfer', 'glass']

  // The cut polygon from --ch-tl/tr/br/bl, each less `less` (chamfer.ts).
  function poly(less) {
    function c(k) {
      return 'max(0px, calc(var(--ch-' + k + ') - ' + less + '))'
    }
    return (
      'polygon(' + c('tl') + ' 0, calc(100% - ' + c('tr') + ') 0, 100% ' + c('tr') + ', 100% calc(100% - ' + c('br') + '), ' +
      'calc(100% - ' + c('br') + ') 100%, ' + c('bl') + ' 100%, 0 calc(100% - ' + c('bl') + '), 0 ' + c('tl') + ')'
    )
  }
  function cuts(tl, tr, br, bl) {
    return '--ch-tl:' + tl + 'px;--ch-tr:' + tr + 'px;--ch-br:' + br + 'px;--ch-bl:' + bl + 'px;'
  }

  var C = '.k2t-look-chamfer'
  var G = '.k2t-look-glass'
  var DARK = ':root[data-zen-scheme="dark"] '

  var CSS = [
    // ── Tokens: chamfer = the Garden theme (K2's basic theme as fallback) ──
    C + '{--k2t-bg:var(--zen-surface,#ffffff);--k2t-canvas:var(--zen-canvas,#f7f8fa);--k2t-raised:var(--zen-surface-raised,#ffffff);' +
      '--k2t-field:var(--zen-surface-raised,#ffffff);--k2t-ear:var(--zen-surface,#ffffff);--k2t-soft:var(--zen-surface,#ffffff);' +
      '--k2t-border:var(--zen-border,#e3e6ec);--k2t-text:var(--zen-text,#1a1d23);--k2t-muted:var(--zen-text-muted,#5f6672);' +
      '--k2t-accent:var(--zen-accent,#2563eb);--k2t-accent-text:var(--zen-accent-text,#ffffff);--k2t-me:var(--zen-bubble-me,#2563eb);' +
      '--k2t-me-text:var(--zen-bubble-me-text,#ffffff);--k2t-agent:var(--zen-bubble-agent,#eef1f5);--k2t-agent-text:var(--zen-bubble-agent-text,#1a1d23);' +
      '--k2t-working:var(--zen-working,#2563eb);--k2t-needs-you:var(--zen-needs-you,#c27c0e);--k2t-ok:var(--zen-ok,#2f855a);--k2t-danger:var(--zen-danger,#c53030);' +
      '--k2t-font:var(--zen-font-family,-apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif);--k2t-size:var(--zen-font-size,14px);' +
      '--k2t-lh:var(--zen-line-height,1.45);--k2t-gap:var(--zen-gap,12px);--k2t-msg-max:min(78%,46rem);--k2t-wash:86%}',
    DARK + C + '{--k2t-bg:var(--zen-surface,#1a1b1f);--k2t-canvas:var(--zen-canvas,#121316);--k2t-raised:var(--zen-surface-raised,#222328);' +
      '--k2t-field:var(--zen-surface-raised,#222328);--k2t-ear:var(--zen-surface,#1a1b1f);--k2t-soft:var(--zen-surface,#1a1b1f);' +
      '--k2t-border:var(--zen-border,#2e3036);--k2t-text:var(--zen-text,#e6e8ec);--k2t-muted:var(--zen-text-muted,#9aa0ab);' +
      '--k2t-accent:var(--zen-accent,#60a5fa);--k2t-accent-text:var(--zen-accent-text,#0b1220);--k2t-me:var(--zen-bubble-me,#1d4ed8);' +
      '--k2t-me-text:var(--zen-bubble-me-text,#eff6ff);--k2t-agent:var(--zen-bubble-agent,#222328);--k2t-agent-text:var(--zen-bubble-agent-text,#e6e8ec);' +
      '--k2t-working:var(--zen-working,#60a5fa);--k2t-needs-you:var(--zen-needs-you,#e0a43a);--k2t-ok:var(--zen-ok,#5cc08a);--k2t-danger:var(--zen-danger,#f07167);--k2t-wash:72%}',
    // ── Tokens: glass = K2 Companion (kanban@3's kanban.css palettes) ──
    G + '{--k2t-bg:#ffffff;--k2t-canvas:#f2f2f7;--k2t-raised:#ffffff;--k2t-field:#ffffff;--k2t-ear:#f2f2f7;--k2t-soft:#e5e5ea;' +
      '--k2t-border:rgba(60,60,67,.16);--k2t-text:#1c1c1e;--k2t-muted:#6e6e73;--k2t-accent:#007aff;--k2t-accent-text:#ffffff;' +
      '--k2t-me:#007aff;--k2t-me-text:#ffffff;--k2t-agent:#e9e9eb;--k2t-agent-text:#000000;--k2t-working:#007aff;--k2t-needs-you:#ff9500;' +
      '--k2t-ok:#34c759;--k2t-danger:#ff3b30;--k2t-font:-apple-system,BlinkMacSystemFont,"SF Pro Text","SF Pro Display",system-ui,sans-serif;' +
      '--k2t-size:14px;--k2t-lh:1.45;--k2t-gap:12px;--k2t-msg-max:min(82%,46rem);--k2t-sheen:0 1px 2px rgba(0,0,0,.04);--k2t-wash:86%}',
    DARK + G + '{--k2t-bg:#0a0a0a;--k2t-canvas:#0a0a0a;--k2t-raised:#1c1c1e;--k2t-field:#1c1c1e;--k2t-ear:#111111;--k2t-soft:#2c2c2e;' +
      '--k2t-border:rgba(60,60,62,.75);--k2t-text:#e4e4e7;--k2t-muted:#8e8e93;--k2t-accent:#0a84ff;--k2t-me:#0a84ff;--k2t-me-text:#ffffff;' +
      '--k2t-agent:#3a3a3c;--k2t-agent-text:#ffffff;--k2t-working:#0a84ff;--k2t-needs-you:#ff9f0a;--k2t-ok:#30d158;--k2t-danger:#ff453a;' +
      '--k2t-sheen:inset 0 1px 0 rgba(255,255,255,.12);--k2t-wash:72%}',
    // ── The panel ──
    '.k2t{position:relative;display:flex;flex-direction:column;min-height:0;height:100%;box-sizing:border-box;font-family:var(--k2t-font);' +
      'font-size:var(--k2t-size);line-height:var(--k2t-lh);color:var(--k2t-text);color-scheme:inherit}',
    '.k2t *,.k2t *::before,.k2t *::after{box-sizing:border-box}',
    '.k2t[data-k2t-frame]{background:var(--k2t-bg);border:1px solid var(--k2t-border);overflow:hidden}',
    G + '[data-k2t-frame]{border-radius:14px}',
    '.k2t-body{position:relative;z-index:0;flex:1 1 auto;min-height:0;overflow:auto;overscroll-behavior:contain;padding:14px var(--k2t-gap) 8px}',
    '.k2t-msgs{display:flex;flex-direction:column;gap:10px;container-type:inline-size}',
    '.k2t-msg{display:flex;flex-direction:column;gap:3px;min-width:0;max-width:var(--k2t-msg-max)}',
    '@container (max-width: 479.98px){.k2t-msg{max-width:calc(100% - 12px)}}',
    '.k2t-mine{align-self:flex-end;align-items:flex-end}.k2t-theirs{align-self:flex-start;align-items:flex-start}',
    '.k2t-bubble{min-width:0;max-width:100%;overflow-wrap:anywhere;-webkit-user-select:text;user-select:text}',
    '.k2t-bubble-me{background:var(--k2t-me);color:var(--k2t-me-text)}.k2t-bubble-agent{background:var(--k2t-agent);color:var(--k2t-agent-text)}',
    C + ' .k2t-bubble{padding:9px 14px}',
    G + ' .k2t-bubble{padding:8px 13px;line-height:1.4;border-radius:18px}',
    G + ' .k2t-bubble-me{border-bottom-right-radius:6px}' + G + ' .k2t-bubble-agent{border-bottom-left-radius:6px}',
    '.k2t-text{white-space:pre-wrap}.k2t-md{white-space:normal}.k2t-md>:first-child{margin-top:0}.k2t-md>:last-child{margin-bottom:0}',
    '.k2t-md code,.k2t-md pre{background:color-mix(in srgb,currentColor 14%,transparent)}.k2t-md pre{padding:6px 8px;overflow:auto}',
    '.k2t-bubble-me a{color:inherit;text-decoration:underline}',
    '.k2t-status{margin-top:4px;font-size:.85em;opacity:.8}',
    '.k2t-meta{padding:0 6px;font-size:.72em;color:var(--k2t-muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:100%}',
    '.k2t-opts,.k2t-custom{display:flex;flex-wrap:wrap;gap:6px;margin-top:8px}.k2t-custom{flex-wrap:nowrap}',
    '.k2t-chip{padding:5px 12px;border:1px solid color-mix(in srgb,currentColor 40%,transparent);border-radius:999px;background:transparent;color:inherit;font:inherit;line-height:1.2;cursor:pointer}',
    '.k2t-chip:hover{background:color-mix(in srgb,currentColor 12%,transparent)}',
    '.k2t-custom input{flex:1;min-width:0;padding:5px 12px;border:1px solid color-mix(in srgb,currentColor 40%,transparent);border-radius:999px;background:transparent;color:inherit;font:inherit}',
    '.k2t-files{display:flex;flex-wrap:wrap;gap:6px;max-width:100%}',
    '.k2t-file{display:inline-flex;align-items:center;gap:6px;max-width:16rem;padding:3px 10px;font:inherit;font-size:.8em;line-height:1.4;color:var(--k2t-text);' +
      'background:var(--k2t-raised);border:1px solid var(--k2t-border);border-radius:999px}',
    'button.k2t-file{cursor:pointer}.k2t-file span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
    '.k2t-img{display:block;max-width:min(260px,100%);max-height:220px;object-fit:contain;border:1px solid var(--k2t-border);border-radius:12px}',
    '.k2t-note{color:var(--k2t-muted);font-size:.85em}.k2t-empty{padding:24px 12px;text-align:center}',
    '.k2t-err{color:var(--k2t-danger);font-size:.85em;margin:0 0 6px}',
    // ── The foot: the box floats over the list's end, glass behind it ──
    '.k2t-foot{position:absolute;left:0;right:0;bottom:0;z-index:2;padding:4px var(--k2t-gap) 12px}',
    '.k2t-glass{position:absolute;left:0;right:0;bottom:0;top:-6px;pointer-events:none;z-index:0}',
    '.k2t-glass>span{position:absolute;inset:0;pointer-events:none}',
    '.k2t-blur-1{-webkit-backdrop-filter:blur(3px);backdrop-filter:blur(3px);-webkit-mask-image:linear-gradient(to bottom,transparent 0%,#000 30%);mask-image:linear-gradient(to bottom,transparent 0%,#000 30%)}',
    '.k2t-blur-2{-webkit-backdrop-filter:blur(14px);backdrop-filter:blur(14px);-webkit-mask-image:linear-gradient(to bottom,transparent 30%,#000 75%);mask-image:linear-gradient(to bottom,transparent 30%,#000 75%)}',
    '.k2t-tint{background:linear-gradient(to bottom,color-mix(in srgb,var(--k2t-bg) 0%,transparent) 0%,color-mix(in srgb,var(--k2t-bg) 6%,transparent) 20%,' +
      'color-mix(in srgb,var(--k2t-bg) 18%,transparent) 45%,color-mix(in srgb,var(--k2t-bg) 32%,transparent) 70%,color-mix(in srgb,var(--k2t-bg) 46%,transparent) 100%)}',
    '@media (prefers-reduced-transparency: reduce){.k2t-blur-1,.k2t-blur-2{display:none}.k2t-tint{background:linear-gradient(to bottom,transparent 0,var(--k2t-bg) 6px)}' +
      '.k2t .k2t-field{--ch-fill:var(--k2t-field)!important;background:var(--k2t-field)!important}}',
    '@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))){.k2t-blur-1,.k2t-blur-2{display:none}' +
      '.k2t-tint{background:linear-gradient(to bottom,transparent 0,var(--k2t-bg) 6px)}}',
    '.k2t-newer{position:absolute;left:50%;bottom:calc(100% + 8px);z-index:3;transform:translateX(-50%);padding:4px 12px;border:1px solid var(--k2t-border);' +
      'border-radius:999px;background:var(--k2t-raised);color:var(--k2t-accent);font:inherit;font-size:.82em;line-height:1.3;white-space:nowrap;' +
      'box-shadow:0 4px 14px color-mix(in srgb,black 18%,transparent);cursor:pointer}',
    '.k2t-newer[hidden],.k2t-ear[hidden],.k2t-err[hidden],.k2t-compose[hidden]{display:none}',
    '.k2t-newer:focus-visible,.k2t-send:focus-visible,.k2t-chip:focus-visible{outline:2px solid var(--k2t-accent);outline-offset:2px}',
    '.k2t-compose{position:relative;z-index:1;margin:0}',
    // The field: a see-through wash of its fill so the blurred list shows.
    '.k2t-field{display:flex;align-items:flex-end;gap:6px;border:1px solid var(--k2t-border);' +
      'background:color-mix(in srgb,var(--k2t-field) var(--k2t-wash),transparent)}',
    C + ' .k2t-field{padding:6px;gap:8px}',
    G + ' .k2t-field{padding:4px 4px 4px 14px;border-radius:20px;box-shadow:var(--k2t-sheen)}',
    G + ' .k2t-field:focus-within{border-color:color-mix(in srgb,var(--k2t-accent) 55%,var(--k2t-border))}',
    '.k2t-field textarea{flex:1 1 auto;min-width:0;margin:0;border:0;background:transparent;color:inherit;font:inherit;resize:none;overflow:hidden}',
    C + ' .k2t-field textarea{min-height:32px;max-height:220px;padding:6px 4px 6px 8px;line-height:20px}',
    G + ' .k2t-field textarea{min-height:22px;max-height:220px;padding:4px 0;line-height:22px}',
    '.k2t-field textarea::placeholder{color:var(--k2t-muted);opacity:1}.k2t-field textarea:focus{outline:none}',
    '.k2t-send{flex:none;display:inline-flex;align-items:center;justify-content:center;padding:0;border:0;border-radius:50%;' +
      'background:var(--k2t-soft);color:var(--k2t-muted);transition:background-color 140ms ease,color 140ms ease,filter 90ms ease-out}',
    C + ' .k2t-send{width:32px;height:32px}' + G + ' .k2t-send{width:30px;height:30px}',
    '.k2t-send.k2t-ready{background:var(--k2t-accent);color:var(--k2t-accent-text);cursor:pointer}',
    '.k2t-send.k2t-ready:active{filter:brightness(1.5)}',
    // ── The working tab on the field's top edge ──
    '.k2t-ear{position:relative;z-index:0;display:flex;align-items:center;gap:8px;height:31px;padding:0 12px;background:var(--k2t-ear);' +
      'border:1px solid var(--k2t-border);border-bottom:0;font-size:13px;line-height:18px;white-space:nowrap;animation:k2t-ear-in 200ms cubic-bezier(.2,.9,.3,1.12) both}',
    C + ' .k2t-ear{margin:0 14px -1px}',
    G + ' .k2t-ear{margin:0 10px -1px;border-radius:15px 15px 0 0}',
    '.k2t-ear-dot{flex:none;width:8px;height:8px;border-radius:50%;background:var(--k2t-working)}',
    '.k2t-ear-label{flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;color:var(--k2t-text)}',
    '.k2t-ear-counts{flex:none;display:flex;align-items:center;gap:4px;font-size:12px;font-variant-numeric:tabular-nums}',
    '.k2t-tag{--tag:var(--k2t-muted);display:inline-flex;align-items:center;gap:3px;height:18px;padding:0 5px;border-radius:999px;' +
      'background:color-mix(in srgb,var(--tag) 16%,transparent);color:color-mix(in srgb,var(--tag) 70%,var(--k2t-text))}',
    '.k2t-tag svg{fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}',
    '.k2t-tag-subagents{--tag:var(--k2t-accent)}.k2t-tag-tools{--tag:#2dd4bf}.k2t-tag-commands{--tag:var(--k2t-ok)}',
    '.k2t-ear-clock{flex:none;font-size:12px;color:var(--k2t-muted);font-variant-numeric:tabular-nums}',
    '.k2t-ear[data-tone="working"] .k2t-ear-dot,.k2t-ear[data-tone="working"] .k2t-ear-label{animation:k2t-pulse 1.6s ease-in-out infinite}',
    '.k2t-ear[data-tone="monitoring"] .k2t-ear-dot{background:transparent;border:2px solid var(--k2t-accent)}',
    '.k2t-ear[data-tone="needs-you"] .k2t-ear-dot{background:var(--k2t-needs-you)}',
    '.k2t-ear[data-tone="needs-you"] .k2t-ear-label{color:color-mix(in srgb,var(--k2t-needs-you) 75%,var(--k2t-text))}',
    '.k2t-ear[data-tone="pending"] .k2t-ear-dot{background:transparent;border:2px solid var(--k2t-muted)}',
    '.k2t-ear[data-tone="pending"] .k2t-ear-label{color:var(--k2t-muted)}',
    '.k2t-reduced .k2t-ear,.k2t-reduced .k2t-ear-dot,.k2t-reduced .k2t-ear-label{animation:none!important}',
    '@keyframes k2t-ear-in{from{transform:translateY(calc(100% + 8px))}to{transform:none}}',
    '@keyframes k2t-pulse{0%,100%{opacity:1}50%{opacity:.45}}',
    '@media (prefers-reduced-motion: reduce){.k2t-ear,.k2t-ear-dot,.k2t-ear-label{animation:none!important}.k2t-send{transition:none}}',
    // ── Chamfers (chamfer.ts): fill and edge drawn by two clipped layers ──
    C + ' [data-k2t-ch]{position:relative;isolation:isolate;background:transparent!important;border-color:transparent!important;border-radius:0!important}',
    C + ' [data-k2t-ch]::before,' + C + ' [data-k2t-ch]::after{content:"";position:absolute;pointer-events:none;z-index:-1}',
    C + ' [data-k2t-ch]::before{inset:var(--ch-outset,calc(-1 * var(--ch-bw,0px)));background:var(--ch-edge,var(--ch-fill));clip-path:' + poly('0px') + '}',
    C + ' [data-k2t-ch]::after{inset:0;background:var(--ch-fill);clip-path:' + poly('calc(var(--ch-bw, 0px) * 0.586)') + '}',
    C + '[data-k2t-frame]{border-radius:0}',
    C + ' .k2t-bubble-me{' + cuts(4, 4, 0, 4) + '--ch-fill:var(--k2t-me)}',
    C + ' .k2t-bubble-agent{' + cuts(4, 4, 4, 0) + '--ch-fill:var(--k2t-agent)}',
    C + ' .k2t-field{' + cuts(4, 4, 4, 4) + '--ch-fill:color-mix(in srgb,var(--k2t-field) var(--k2t-wash),transparent);--ch-edge:var(--k2t-border);--ch-bw:1px}',
    C + ' .k2t-send{' + cuts(5, 5, 5, 5) + '--ch-fill:var(--k2t-soft)}',
    C + ' .k2t-send.k2t-ready{--ch-fill:var(--k2t-accent)}',
    C + ' .k2t-ear{' + cuts(5, 5, 0, 0) + '--ch-fill:var(--k2t-ear);--ch-edge:var(--k2t-border);--ch-bw:1px;--ch-outset:-1px -1px 0 -1px}',
    C + ' .k2t-newer{' + cuts(4, 4, 4, 4) + '--ch-fill:var(--k2t-raised);--ch-edge:var(--k2t-border);--ch-bw:1px}',
    // A chip's fill is its bubble's (the edge layer would show through a
    // transparent one).
    C + ' .k2t-chip{' + cuts(4, 4, 4, 4) + '--k2t-chip-bg:var(--k2t-agent);--ch-fill:var(--k2t-chip-bg);--ch-edge:color-mix(in srgb,currentColor 40%,var(--k2t-chip-bg));--ch-bw:1px}',
    C + ' .k2t-bubble-me .k2t-chip{--k2t-chip-bg:var(--k2t-me)}',
    C + ' .k2t-chip:hover{--ch-fill:color-mix(in srgb,currentColor 12%,var(--k2t-chip-bg))}',
    C + ' .k2t-custom input{border-radius:0}',
    C + ' .k2t-file{' + cuts(4, 4, 4, 4) + '--ch-fill:var(--k2t-raised);--ch-edge:var(--k2t-border);--ch-bw:1px}',
    C + ' .k2t-img{border-radius:0}',
  ].join('\n')

  var styled = false
  function addStyle() {
    if (styled) return
    styled = true
    var s = d.createElement('style')
    s.setAttribute('data-k2-lib', 'k2-thread')
    s.textContent = CSS
    ;(d.head || d.documentElement).appendChild(s)
  }
  addStyle()

  var STICK_PX = 32
  var SMOOTH_MS = 700
  var PENDING_MAX_MS = 3 * 60 * 1000
  var SVG = 'http://www.w3.org/2000/svg'
  // 24×24 strokes (K2's drawer icons): a robot, a wrench, a prompt.
  var ICONS = {
    subagents: ['M6 7h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2z', 'M12 3v4', 'M9 13h.01M15 13h.01', 'M2 12v3M22 12v3'],
    tools: ['M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.9 6.9a2.1 2.1 0 0 1-3-3l6.9-6.9a6 6 0 0 1 7.9-7.9z'],
    commands: ['M4 17l6-6-6-6', 'M12 19h8'],
  }
  var COUNTS = [
    ['subagents', 'subagent', 'subagents'],
    ['tools', 'tool call', 'tool calls'],
    ['commands', 'command', 'commands'],
  ]
  var IMAGE_EXT = /\.(png|jpe?g|gif|webp)$/i

  function el(tag, cls, text) {
    var n = d.createElement(tag)
    if (cls) n.className = cls
    if (text != null) n.textContent = text
    return n
  }

  function svgPaths(paths, size) {
    var svg = d.createElementNS(SVG, 'svg')
    svg.setAttribute('viewBox', '0 0 24 24')
    svg.setAttribute('width', String(size))
    svg.setAttribute('height', String(size))
    svg.setAttribute('aria-hidden', 'true')
    paths.forEach(function (p) {
      var path = d.createElementNS(SVG, 'path')
      path.setAttribute('d', p)
      svg.appendChild(path)
    })
    return svg
  }

  // K2's relative time (lib/format-relative-time): just now, 5m ago, 2h ago.
  function ago(sec, now) {
    var t = Math.max(0, now - sec)
    if (t < 45) return 'just now'
    if (t < 90) return '1m ago'
    var mins = Math.round(t / 60)
    if (mins < 60) return mins + 'm ago'
    var hours = Math.round(t / 3600)
    if (hours < 24) return hours + 'h ago'
    return Math.round(t / 86400) + 'd ago'
  }

  // K2's drawer clock: 0:05, 1:14, 1:02:09.
  function clock(ms) {
    var total = Math.max(0, Math.floor(ms / 1000))
    var h = Math.floor(total / 3600)
    var m = Math.floor((total % 3600) / 60)
    var s = String(total % 60).padStart(2, '0')
    return h ? h + ':' + String(m).padStart(2, '0') + ':' + s : m + ':' + s
  }

  function errText(e) {
    if (!e) return 'error'
    if (e.code === 'sending_off') return 'posting is paused; click Resume on the widget'
    return e.code || e.message || 'error'
  }

  function checkLook(look) {
    var l = look == null ? 'chamfer' : look
    if (LOOKS.indexOf(l) < 0) throw new TypeError('K2Thread: look is "chamfer" or "glass", not ' + JSON.stringify(look))
    return l
  }

  /**
   * Mount a Thread panel into `root` (its children are replaced).
   * opts: {look: 'chamfer'|'glass' (default 'chamfer'), address, label,
   *   frame (default true: the panel draws its own background and border),
   *   agents (default true: read the agent list for names and counters),
   *   placeholder, emptyText, markdown (default true when marked and
   *   DOMPurify are loaded), renderBody(node, text, item),
   *   renderAttachment(file, item) → Node | null, onOpenFile(file, item),
   *   onError(err)}
   */
  function mount(root, opts) {
    var k2 = window.k2
    if (!k2 || typeof k2.can !== 'function') throw new Error('K2Thread: K2’s frame runtime (window.k2) is missing')
    if (!root || root.nodeType !== 1) throw new TypeError('K2Thread.mount needs an element')
    var o = opts && typeof opts === 'object' ? opts : {}
    var look = checkLook(o.look)

    var panel = el('div', 'k2t k2t-look-' + look)
    panel.setAttribute('data-k2t-look', look)
    if (o.frame !== false) {
      panel.setAttribute('data-k2t-frame', '')
      if (look === 'chamfer') panel.setAttribute('data-k2t-ch', '')
    }
    var bodyEl = el('div', 'k2t-body')
    bodyEl.setAttribute('role', 'log')
    bodyEl.setAttribute('aria-live', 'polite')
    bodyEl.tabIndex = 0
    var footEl = el('div', 'k2t-foot')
    var glass = el('span', 'k2t-glass')
    glass.setAttribute('aria-hidden', 'true')
    glass.appendChild(el('span', 'k2t-blur-1'))
    glass.appendChild(el('span', 'k2t-blur-2'))
    glass.appendChild(el('span', 'k2t-tint'))
    var newerBtn = el('button', 'k2t-newer')
    newerBtn.type = 'button'
    newerBtn.hidden = true
    newerBtn.setAttribute('data-k2t-ch', '')
    var errEl = el('div', 'k2t-err')
    errEl.hidden = true
    errEl.setAttribute('role', 'alert')
    var earEl = el('div', 'k2t-ear')
    earEl.hidden = true
    earEl.setAttribute('data-k2t-ch', '')
    earEl.setAttribute('role', 'status')
    var form = el('form', 'k2t-compose')
    form.hidden = true
    var field = el('div', 'k2t-field')
    field.setAttribute('data-k2t-ch', '')
    var textEl = el('textarea')
    textEl.rows = 1
    var sendBtn = el('button', 'k2t-send')
    sendBtn.type = 'submit'
    sendBtn.disabled = true
    sendBtn.setAttribute('aria-label', 'Send')
    sendBtn.title = 'Send (Enter)'
    sendBtn.setAttribute('data-k2t-ch', '')
    var arrow = d.createElementNS(SVG, 'svg')
    arrow.setAttribute('width', '14')
    arrow.setAttribute('height', '14')
    arrow.setAttribute('viewBox', '0 0 16 16')
    arrow.setAttribute('fill', 'none')
    arrow.setAttribute('stroke', 'currentColor')
    arrow.setAttribute('stroke-width', '2')
    arrow.setAttribute('stroke-linecap', 'round')
    arrow.setAttribute('stroke-linejoin', 'round')
    arrow.setAttribute('aria-hidden', 'true')
    ;['M8 13V3', 'M3.5 7.5 8 3l4.5 4.5'].forEach(function (p) {
      var path = d.createElementNS(SVG, 'path')
      path.setAttribute('d', p)
      arrow.appendChild(path)
    })
    sendBtn.appendChild(arrow)
    field.appendChild(textEl)
    field.appendChild(sendBtn)
    form.appendChild(field)
    footEl.appendChild(glass)
    footEl.appendChild(newerBtn)
    footEl.appendChild(errEl)
    footEl.appendChild(earEl)
    footEl.appendChild(form)
    panel.appendChild(bodyEl)
    panel.appendChild(footEl)
    root.replaceChildren(panel)

    var current = { address: '', label: '' }
    var offThread = null
    var offAgents = null
    var rows = []
    var posting = false
    var destroyed = false
    var lastView = null
    var timers = []

    function reducedMotion() {
      return !!(k2.motion && k2.motion.reduced)
    }
    panel.classList.toggle('k2t-reduced', reducedMotion())

    function report(e) {
      if (typeof o.onError === 'function') {
        try {
          o.onError(e)
        } catch (x) {
          console.warn('K2Thread: onError threw', x)
        }
      }
    }

    function note(text, cls) {
      forgetList()
      bodyEl.replaceChildren(el('div', cls || 'k2t-note', text))
    }

    function showErr(msg) {
      errEl.hidden = !msg
      errEl.textContent = msg || ''
      footSize()
    }

    // ── Messages ──────────────────────────────────────────────────────────
    // A person's post (yours, from K2's Thread or this panel) says
    // via=compose; everything else is the agent's.
    function isMine(doc) {
      return doc.via === 'compose'
    }

    function authorOf(doc, mine) {
      if (mine) return 'You'
      var from = String(doc.from || '')
      if (!from) return 'agent'
      // `from` is the address, or the handle on this agent's own server.
      var row = rows.find(function (r) {
        return r.address === from
      }) || rows.find(function (r) {
        return r.address.split('::')[0] === from && r.address === current.address
      })
      if (row && row.label) return row.label
      var here = current.address
      if (here && (from === here || from === here.split('::')[0])) return current.label || from
      return from
    }

    function nowSec() {
      return Math.floor(Date.now() / 1000)
    }

    function fillBody(node, text, item) {
      var t = String(text || '')
      if (typeof o.renderBody === 'function') {
        o.renderBody(node, t, item)
        return
      }
      var marked = window.marked
      var purify = window.DOMPurify
      if (o.markdown !== false && marked && typeof marked.parse === 'function' && purify && typeof purify.sanitize === 'function') {
        node.classList.add('k2t-md')
        node.innerHTML = purify.sanitize(marked.parse(t))
      } else {
        node.classList.add('k2t-text')
        node.textContent = t
      }
    }

    function answer(id, value) {
      if (!current.address || !id) return
      if (!k2.can('thread.answer')) {
        showErr('This widget can’t answer cards.')
        return
      }
      k2.thread.answer(current.address, id, value).catch(function (e) {
        showErr('Answer failed: ' + errText(e))
        report(e)
      })
    }

    function choiceRow(id, c) {
      var opts = el('div', 'k2t-opts')
      ;(c.options || []).forEach(function (opt) {
        var text = opt == null ? '' : typeof opt === 'string' ? opt : String(opt.label || opt.value || '')
        if (!text) return
        var b = el('button', 'k2t-chip', text)
        b.type = 'button'
        b.setAttribute('data-k2t-ch', '')
        b.addEventListener('click', function () {
          answer(id, text)
        })
        opts.appendChild(b)
      })
      return opts
    }

    function fileName(f) {
      var n = f.name || f.path || f.url || 'file'
      return String(n).split('/').pop()
    }

    function attachment(f, item) {
      if (typeof o.renderAttachment === 'function') {
        var custom = o.renderAttachment(f, item)
        if (custom) return custom
      }
      var name = fileName(f)
      var src = typeof f.preview === 'string' ? f.preview : typeof f.url === 'string' ? f.url : ''
      var image = (typeof f.mime === 'string' && f.mime.indexOf('image/') === 0) || IMAGE_EXT.test(name)
      if (image && /^data:image\/(png|jpeg|gif|webp);/.test(src)) {
        var img = el('img', 'k2t-img')
        img.src = src
        img.alt = name
        return img
      }
      var open = typeof o.onOpenFile === 'function'
      var chip = el(open ? 'button' : 'span', 'k2t-file')
      if (open) {
        chip.type = 'button'
        chip.addEventListener('click', function () {
          o.onOpenFile(f, item)
        })
      }
      chip.setAttribute('data-k2t-ch', '')
      chip.setAttribute('data-k2t-file', name)
      chip.title = String(f.path || name)
      chip.appendChild(el('span', null, name))
      return chip
    }

    function renderItem(item, now) {
      var doc = (item && item.doc) || {}
      var id = item.id || doc.id || ''
      var kind = doc.kind || 'text'
      var mine = isMine(doc)
      var wrap = el('div', 'k2t-msg ' + (mine ? 'k2t-mine' : 'k2t-theirs'))
      wrap.setAttribute('data-kind', kind)
      if (id) wrap.setAttribute('data-msg-id', id)
      if (typeof item.seq === 'number') wrap.setAttribute('data-seq', String(item.seq))
      var files = Array.isArray(doc.attachments) ? doc.attachments.filter(function (f) { return f && typeof f === 'object' }) : []
      var bodyText = kind === 'choice' ? (doc.choice && doc.choice.prompt) || doc.body || '' : doc.body || ''
      // A post that is only files has no bubble (as K2's Thread).
      if (!(files.length && !bodyText && kind === 'text')) {
        var bubble = el('div', 'k2t-bubble ' + (mine ? 'k2t-bubble-me' : 'k2t-bubble-agent'))
        bubble.setAttribute('data-k2t-ch', '')
        wrap.appendChild(bubble)
        if (kind === 'secret') {
          bubble.appendChild(el('div', 'k2t-text', doc.body || 'Secret requested'))
          bubble.appendChild(el('div', 'k2t-status', 'Answer secret cards in K2’s own Thread (widgets can’t send secrets).'))
        } else {
          var body = el('div')
          fillBody(body, bodyText, item)
          bubble.appendChild(body)
          if (kind === 'choice') {
            var c = doc.choice || {}
            if (c.status === 'pending') {
              bubble.appendChild(choiceRow(id, c))
              if (c.allow_custom) {
                var row = el('div', 'k2t-custom')
                var inp = el('input')
                inp.type = 'text'
                inp.placeholder = 'Custom answer'
                var go = el('button', 'k2t-chip', 'Answer')
                go.type = 'button'
                go.setAttribute('data-k2t-ch', '')
                go.addEventListener('click', function () {
                  var t = (inp.value || '').trim()
                  if (t) answer(id, t)
                })
                row.appendChild(inp)
                row.appendChild(go)
                bubble.appendChild(row)
              }
            } else {
              bubble.appendChild(el('div', 'k2t-status', c.status === 'voided' ? 'Voided' : 'Answered' + (c.answer ? ': ' + c.answer : '')))
            }
          }
        }
      }
      if (files.length) {
        var box = el('div', 'k2t-files')
        files.forEach(function (f) {
          box.appendChild(attachment(f, item))
        })
        wrap.appendChild(box)
      }
      var meta = el('div', 'k2t-meta')
      meta.appendChild(el('span', 'k2t-author', authorOf(doc, mine)))
      if (typeof doc.created_at === 'number' && isFinite(doc.created_at)) {
        meta.appendChild(d.createTextNode(' · '))
        var t = el('span', 'k2t-time', ago(doc.created_at, now))
        t.setAttribute('data-at', String(doc.created_at))
        meta.appendChild(t)
      }
      wrap.appendChild(meta)
      return wrap
    }

    function refreshTimes() {
      var now = nowSec()
      bodyEl.querySelectorAll('.k2t-time[data-at]').forEach(function (t) {
        var at = Number(t.getAttribute('data-at'))
        if (isFinite(at)) t.textContent = ago(at, now)
      })
    }
    timers.push(setInterval(refreshTimes, 30000))

    // ── Scrolling (K2's stick-to-bottom, hooks/useStickToBottom.ts) ─────────
    // At the bottom (within STICK_PX) the list stays pinned to its true
    // bottom; scrolled up, nothing moves and the pill counts new messages.
    // "At the bottom" changes only on a scroll the person made. The list is
    // patched, never rebuilt (WebKit resets scrollTop when a scroll box's
    // content is replaced), and a changed message above the view keeps the
    // first visible message where it was. Your own post always goes down.
    var stuck = true
    var seenSize = null
    var unseen = 0
    var autoUntil = 0
    var settleTimer = null
    var stickNext = false
    var listEl = null
    var nodes = {}
    var known = {}
    var sizeObs = null

    function bottomOf() {
      return Math.max(0, bodyEl.scrollHeight - bodyEl.clientHeight)
    }

    function noteSize() {
      seenSize = { client: bodyEl.clientHeight, scroll: bodyEl.scrollHeight }
    }

    function showNewer() {
      newerBtn.hidden = !unseen
      var words = unseen === 1 ? '1 new message' : unseen + ' new messages'
      newerBtn.textContent = words + ' ↓'
      newerBtn.setAttribute('aria-label', words + ', go to the newest')
    }

    function clearNewer() {
      if (!unseen && newerBtn.hidden) return
      unseen = 0
      showNewer()
    }

    function pin(smooth) {
      stuck = true
      clearNewer()
      if (bodyEl.clientHeight === 0) return
      var to = bottomOf()
      if (smooth && !reducedMotion() && typeof bodyEl.scrollTo === 'function') {
        autoUntil = Date.now() + SMOOTH_MS
        bodyEl.scrollTo({ top: to, behavior: 'smooth' })
        clearTimeout(settleTimer)
        settleTimer = setTimeout(function () {
          autoUntil = 0
          sync()
        }, SMOOTH_MS + 50)
      } else if (bodyEl.scrollTop !== to) {
        bodyEl.scrollTop = to
      }
      noteSize()
    }

    function sync() {
      if (destroyed || bodyEl.clientHeight === 0) return
      if (stuck) {
        var to = bottomOf()
        if (Date.now() < autoUntil && typeof bodyEl.scrollTo === 'function') bodyEl.scrollTo({ top: to, behavior: 'smooth' })
        else if (bodyEl.scrollTop !== to) bodyEl.scrollTop = to
      }
      noteSize()
    }

    bodyEl.addEventListener('scroll', function () {
      if (bodyEl.clientHeight === 0) return
      if (Date.now() < autoUntil) {
        if (bottomOf() - bodyEl.scrollTop <= 1) autoUntil = 0
        noteSize()
        return
      }
      if (!seenSize || seenSize.client !== bodyEl.clientHeight || seenSize.scroll !== bodyEl.scrollHeight) {
        sync()
        return
      }
      stuck = bottomOf() - bodyEl.scrollTop <= STICK_PX
      if (stuck) clearNewer()
    })
    ;['wheel', 'touchstart', 'pointerdown', 'keydown'].forEach(function (type) {
      bodyEl.addEventListener(
        type,
        function () {
          autoUntil = 0
        },
        { passive: true },
      )
    })
    newerBtn.addEventListener('click', function () {
      pin(true)
    })

    // The box floats over the list's end: the list keeps that much room.
    function footSize() {
      var h = footEl.offsetHeight
      bodyEl.style.paddingBottom = (h > 0 ? h + 8 : 8) + 'px'
    }

    function watchSizes() {
      if (typeof ResizeObserver === 'undefined') return
      if (!sizeObs) {
        sizeObs = new ResizeObserver(function () {
          footSize()
          sync()
        })
        sizeObs.observe(footEl)
      }
      sizeObs.observe(bodyEl)
      if (listEl) sizeObs.observe(listEl)
    }

    function anchorNow() {
      if (!listEl) return null
      var top = bodyEl.getBoundingClientRect().top
      var kids = listEl.children
      for (var i = 0; i < kids.length; i++) {
        var r = kids[i].getBoundingClientRect()
        if (r.bottom > top) return { node: kids[i], at: r.top - top, scrollTop: bodyEl.scrollTop }
      }
      return { node: null, at: 0, scrollTop: bodyEl.scrollTop }
    }

    function keepAnchor(a) {
      if (!a) return
      if (a.node && a.node.isConnected) {
        var moved = a.node.getBoundingClientRect().top - bodyEl.getBoundingClientRect().top - a.at
        if (moved) bodyEl.scrollTop += moved
      } else if (bodyEl.scrollTop !== a.scrollTop) {
        bodyEl.scrollTop = a.scrollTop
      }
      noteSize()
    }

    function forgetList() {
      listEl = null
      nodes = {}
    }

    function resetScroll() {
      forgetList()
      known = {}
      stuck = true
      stickNext = false
      autoUntil = 0
      clearNewer()
    }

    function keyOf(item, i) {
      var doc = (item && item.doc) || {}
      return String(item.id || doc.id || (typeof item.seq === 'number' ? 'seq:' + item.seq : 'i:' + i))
    }

    function drawView(view) {
      if (destroyed || !view || view.address !== current.address) return
      if (view.phase === 'unavailable' || view.phase === 'failed') {
        note(view.note || 'This Thread isn’t available right now.')
        lastView = view
        paintStrip()
        return
      }
      var now = nowSec()
      var items = Array.isArray(view.items) ? view.items : []
      if (!items.length) {
        note(
          view.phase === 'opening'
            ? 'Opening…'
            : o.emptyText || 'Message ' + (current.label || 'this agent') + '. It reads this like a text.',
          'k2t-note k2t-empty',
        )
      } else {
        var first = !listEl || !listEl.isConnected
        var anchor = first || stuck ? null : anchorNow()
        if (first) {
          listEl = el('div', 'k2t-msgs')
          nodes = {}
          bodyEl.replaceChildren(listEl)
        }
        var keep = {}
        var seenNow = {}
        var prev = null
        var fresh = 0
        items.forEach(function (item, i) {
          var doc = (item && item.doc) || {}
          var key = keyOf(item, i)
          var mine = isMine(doc)
          var sig = JSON.stringify([item.seq, doc, authorOf(doc, mine)])
          var had = nodes[key]
          var node = had && had.sig === sig ? had.node : renderItem(item, now)
          if (had && had.node !== node && had.node.parentNode === listEl) listEl.replaceChild(node, had.node)
          keep[key] = { node: node, sig: sig }
          var want = prev ? prev.nextSibling : listEl.firstChild
          if (want !== node) listEl.insertBefore(node, want)
          prev = node
          // New since the last draw: counts toward the pill (yours never do).
          if (!known[key] && !first && !mine) fresh++
          seenNow[key] = true
        })
        Object.keys(nodes).forEach(function (key) {
          if (!keep[key] && nodes[key].node.parentNode === listEl) listEl.removeChild(nodes[key].node)
        })
        nodes = keep
        known = seenNow
        keepAnchor(anchor)
        watchSizes()
        if (first || stickNext) {
          pin(!first)
          stickNext = false
        } else if (stuck) {
          pin(fresh > 0)
        } else if (fresh) {
          unseen += fresh
          showNewer()
        }
      }
      lastView = view
      noteReplies(view)
      paintStrip()
    }

    // ── The working tab ───────────────────────────────────────────────────
    var pending = null // { since, seq }
    var localSince = { tone: '', at: 0 }
    var sawWorking = false
    var tick = null

    function countTags(c) {
      if (!c || typeof c !== 'object') return []
      var out = []
      COUNTS.forEach(function (k) {
        var n = c[k[0]]
        if (typeof n === 'number' && n > 0) out.push({ key: k[0], n: n, words: n + ' ' + (n === 1 ? k[1] : k[2]) })
      })
      return out
    }

    function toneNow() {
      var turn = lastView && lastView.address === current.address ? lastView.turn : null
      var row = current.address
        ? rows.find(function (x) {
            return x.address === current.address
          })
        : null
      var tone = ''
      var since = 0
      if (turn && turn.state === 'needs-you') {
        tone = 'needs-you'
        since = turn.since
      } else if (turn && (turn.state === 'working' || turn.state === 'monitoring')) {
        tone = turn.state
        since = turn.since
      } else if (row && (row.needsYou || row.activity === 'needs-you')) tone = 'needs-you'
      else if (row && (row.working || row.activity === 'working')) tone = 'working'
      else if (row && row.activity === 'monitoring') tone = 'monitoring'
      var counts = (turn && turn.counts) || (row && row.counts) || null
      return { tone: tone, since: since, counts: counts }
    }

    function lastSeq() {
      var items = (lastView && lastView.items) || []
      var max = null
      items.forEach(function (it) {
        if (typeof it.seq === 'number' && (max === null || it.seq > max)) max = it.seq
      })
      return max
    }

    function noteReplies(view) {
      if (!pending) return
      var items = view.items || []
      for (var i = 0; i < items.length; i++) {
        var it = items[i]
        if (typeof it.seq === 'number' && pending.seq != null && it.seq > pending.seq && !isMine((it && it.doc) || {})) {
          pending = null
          return
        }
      }
    }

    function stopTick() {
      if (tick) {
        clearInterval(tick)
        tick = null
      }
    }

    function paintStrip() {
      if (destroyed) return
      var now = Date.now()
      var t = toneNow()
      if (t.tone) {
        sawWorking = true
        pending = null
      } else if (sawWorking) {
        sawWorking = false
        pending = null
      }
      if (pending && now - pending.since > PENDING_MAX_MS) pending = null
      var tone = t.tone
      var text = ''
      var since = t.since
      if (tone === 'needs-you') text = 'Needs you'
      else if (tone === 'working') text = 'Working'
      else if (tone === 'monitoring') text = 'Monitoring'
      else if (pending) {
        tone = 'pending'
        text = 'Sent'
        since = pending.since
      }
      var was = !earEl.hidden
      if (!tone || !current.address) {
        earEl.hidden = true
        earEl.replaceChildren()
        localSince = { tone: '', at: 0 }
        stopTick()
        if (was) footSize()
        return
      }
      if (!since) {
        if (localSince.tone !== tone) localSince = { tone: tone, at: now }
        since = localSince.at
      }
      earEl.hidden = false
      earEl.setAttribute('data-tone', tone)
      earEl.replaceChildren()
      var dot = el('span', 'k2t-ear-dot')
      dot.setAttribute('aria-hidden', 'true')
      earEl.appendChild(dot)
      earEl.appendChild(el('span', 'k2t-ear-label', tone === 'pending' ? text + ' · waiting…' : text))
      var tags = tone === 'pending' ? [] : countTags(t.counts)
      if (tags.length) {
        var box = el('span', 'k2t-ear-counts')
        tags.forEach(function (g) {
          var tag = el('span', 'k2t-tag k2t-tag-' + g.key)
          tag.title = g.words
          tag.setAttribute('data-count', g.key)
          tag.appendChild(svgPaths(ICONS[g.key], 12))
          tag.appendChild(d.createTextNode(String(g.n)))
          box.appendChild(tag)
        })
        earEl.appendChild(box)
      }
      var c = el('span', 'k2t-ear-clock', clock(now - since))
      c.title = tone === 'pending' ? 'since you sent it' : 'since this turn started'
      earEl.appendChild(c)
      earEl.setAttribute(
        'aria-label',
        text + ', ' + c.textContent + (tags.length ? ', ' + tags.map(function (g) { return g.words }).join(', ') : ''),
      )
      if (!tick) tick = setInterval(paintStrip, 1000)
      if (!was) footSize()
    }

    // ── The message box ───────────────────────────────────────────────────
    function updateSend() {
      var ready = !!(textEl.value || '').trim() && !!current.address && !posting && k2.can('thread.post')
      sendBtn.disabled = !ready
      sendBtn.classList.toggle('k2t-ready', ready)
    }

    function growCompose() {
      textEl.style.height = 'auto'
      var cap = 220
      var min = look === 'glass' ? 22 : 32
      var next = Math.min(cap, Math.max(min, textEl.scrollHeight))
      textEl.style.height = next + 'px'
      textEl.style.overflow = textEl.scrollHeight > cap ? 'auto' : 'hidden'
    }

    /** Post `text` to the open Thread. Resolves with K2's answer; the text
     *  box keeps its text until K2 took it. */
    function post(text) {
      var t = String(text == null ? '' : text).trim()
      if (!t) return Promise.reject(new Error('K2Thread: nothing to send'))
      if (!current.address) return Promise.reject(new Error('K2Thread: no Thread is open'))
      if (posting) return Promise.reject(new Error('K2Thread: a post is on its way'))
      var to = current.address
      posting = true
      showErr('')
      updateSend()
      stickNext = true
      pin(true)
      pending = { since: Date.now(), seq: lastSeq() }
      paintStrip()
      return k2.thread.post(to, t).then(
        function (posted) {
          posting = false
          if (pending && to === current.address && posted && typeof posted.seq === 'number') pending.seq = posted.seq
          updateSend()
          return posted
        },
        function (err) {
          posting = false
          pending = null
          paintStrip()
          updateSend()
          showErr('Post failed: ' + errText(err))
          report(err)
          throw err
        },
      )
    }

    form.addEventListener('submit', function (e) {
      e.preventDefault()
      var text = (textEl.value || '').trim()
      if (!text || !current.address || posting) return
      post(text).then(
        function () {
          textEl.value = ''
          growCompose()
          updateSend()
        },
        function () {
          // Said in the error line; the text stays.
        },
      )
    })
    textEl.addEventListener('input', function () {
      growCompose()
      updateSend()
    })
    textEl.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter' || e.shiftKey || e.isComposing || e.keyCode === 229) return
      e.preventDefault()
      if (typeof form.requestSubmit === 'function') form.requestSubmit()
      else form.dispatchEvent(new Event('submit', { cancelable: true, bubbles: true }))
    })

    // ── Rooms ─────────────────────────────────────────────────────────────
    function stopThread() {
      if (offThread) {
        try {
          offThread()
        } catch (e) {
          // already closed
        }
        offThread = null
      }
    }

    function resetStrip() {
      lastView = null
      pending = null
      sawWorking = false
      localSince = { tone: '', at: 0 }
      paintStrip()
    }

    function close(text) {
      stopThread()
      resetScroll()
      current.address = ''
      current.label = ''
      form.hidden = true
      showErr('')
      updateSend()
      resetStrip()
      note(text || 'Pick an agent to talk to it here.')
      footSize()
    }

    /** Open `address`'s Thread (`label` names it in the box). */
    function open(address, label) {
      if (destroyed) throw new Error('K2Thread: this panel was destroyed')
      var a = typeof address === 'string' ? address.trim() : ''
      if (!a || a.indexOf('::') <= 0) throw new TypeError('K2Thread.open needs an agent address, handle::host')
      var row = rows.find(function (r) {
        return r.address === a
      })
      current.label = label || (row && row.label) || a
      if (current.address === a && offThread) return
      stopThread()
      current.address = a
      resetScroll()
      resetStrip()
      showErr('')
      textEl.placeholder = o.placeholder || 'Message ' + current.label
      textEl.setAttribute('aria-label', 'Message ' + current.label)
      form.hidden = !k2.can('thread.post')
      updateSend()
      footSize()
      if (!k2.can('thread.subscribe')) {
        note('This widget can’t read the Thread (it needs thread:read).')
        return
      }
      note('Opening…')
      offThread = k2.thread.subscribe(a, drawView, function (e) {
        note('Thread unavailable: ' + errText(e))
        report(e)
      })
    }

    function setLook(next) {
      var l = checkLook(next)
      panel.classList.remove('k2t-look-' + look)
      look = l
      panel.classList.add('k2t-look-' + look)
      panel.setAttribute('data-k2t-look', look)
      if (o.frame !== false) {
        if (look === 'chamfer') panel.setAttribute('data-k2t-ch', '')
        else panel.removeAttribute('data-k2t-ch')
      }
      growCompose()
      footSize()
    }

    function destroy() {
      if (destroyed) return
      stopThread()
      if (offAgents) {
        try {
          offAgents()
        } catch (e) {
          // already closed
        }
        offAgents = null
      }
      destroyed = true
      stopTick()
      timers.forEach(clearInterval)
      clearTimeout(settleTimer)
      if (sizeObs) sizeObs.disconnect()
      if (panel.parentNode) panel.parentNode.removeChild(panel)
    }

    if (o.agents !== false && k2.can('agents.subscribe')) {
      offAgents = k2.agents.subscribe(
        function (list) {
          rows = Array.isArray(list)
            ? list.filter(function (r) {
                return r && typeof r.address === 'string' && r.address
              })
            : []
          var row = current.address
            ? rows.find(function (r) {
                return r.address === current.address
              })
            : null
          if (row && row.label && current.label === current.address) {
            current.label = row.label
            textEl.placeholder = o.placeholder || 'Message ' + current.label
          }
          paintStrip()
        },
        function (e) {
          report(e)
        },
      )
    }

    growCompose()
    if (typeof o.address === 'string' && o.address) open(o.address, o.label)
    else close(o.emptyText || 'Pick an agent to talk to it here.')

    return {
      element: panel,
      open: open,
      close: close,
      post: post,
      setLook: setLook,
      destroy: destroy,
      /** For tests and layout: what the panel shows now. */
      state: function () {
        return {
          look: look,
          address: current.address,
          label: current.label,
          stuck: stuck,
          unseen: unseen,
          posting: posting,
          tone: earEl.hidden ? '' : earEl.getAttribute('data-tone') || '',
        }
      },
    }
  }

  window.K2Thread = Object.freeze({ version: '1.0.0', looks: LOOKS.slice(), mount: mount })
})()
