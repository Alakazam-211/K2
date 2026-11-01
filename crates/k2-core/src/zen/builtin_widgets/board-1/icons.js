// BoardIcons: small 24×24 stroke icons. Board's own drawing (MIT, see NOTICE.md), except crop, which is Lucide's (ISC, see NOTICE.md). icon(name) → <svg>.
(function (g) {
  'use strict'
  const NS = 'http://www.w3.org/2000/svg'
  // Each icon: a list of SVG path data strings (stroked, round caps).
  const I = {
    select: ['M5 3l13 7.5-5.6 1.4L9.8 18z', 'M12.5 12l4.5 5'],
    hand: ['M8 12V5.5a1.5 1.5 0 0 1 3 0V11', 'M11 10.5V4a1.5 1.5 0 0 1 3 0v6.5', 'M14 10.5V5.5a1.5 1.5 0 0 1 3 0v8c0 4-2.5 7-6.5 7-3 0-4.5-1.5-6-4L3 13a1.5 1.5 0 0 1 2.5-1.6L8 14'],
    sticky: ['M4 4h16v10l-6 6H4z', 'M14 20v-6h6'],
    text: ['M5 6V4h14v2', 'M12 4v16', 'M9 20h6'],
    shape: ['M3.5 13.5h7v7h-7z', 'M17 3.5a4 4 0 1 1 0 8 4 4 0 0 1 0-8z', 'M14 20.5l3.5-6 3.5 6z'],
    line: ['M5 19L19 5', 'M12 5h7v7'],
    pen: ['M4 20l1-4L16.5 4.5a2.1 2.1 0 0 1 3 3L8 19z', 'M14.5 6.5l3 3'],
    highlighter: ['M9 15l-3 3h6l1-1', 'M9 15l7.5-11 3.5 2.5-7.5 11z', 'M4 21h16'],
    eraser: ['M8 20l-4-4 10-10 6 6-8 8z', 'M8 20h12', 'M9 11l5 5'],
    frame: ['M7 3v18', 'M17 3v18', 'M3 7h18', 'M3 17h18'],
    card: ['M3.5 5h17v14h-17z', 'M3.5 9h17', 'M7 13h7', 'M7 16h4'],
    table: ['M3.5 4.5h17v15h-17z', 'M3.5 10h17', 'M3.5 15h17', 'M10 4.5v15'],
    mind: ['M12 10a2 2 0 1 1 0 4 2 2 0 0 1 0-4z', 'M4 5h4', 'M4 19h4', 'M16 5h4', 'M16 19h4', 'M10.5 10.5L8 5', 'M10.5 13.5L8 19', 'M13.5 10.5L16 5', 'M13.5 13.5L16 19'],
    comment: ['M4 5h16v11H9l-5 4z'],
    template: ['M4 4h7v7H4z', 'M13 4h7v4h-7z', 'M13 10h7v10h-7z', 'M4 13h7v7H4z'],
    undo: ['M9 14L4 9l5-5', 'M4 9h10a6 6 0 0 1 0 12h-3'],
    redo: ['M15 14l5-5-5-5', 'M20 9H10a6 6 0 0 0 0 12h3'],
    plus: ['M12 5v14', 'M5 12h14'],
    minus: ['M5 12h14'],
    fit: ['M4 9V4h5', 'M20 9V4h-5', 'M4 15v5h5', 'M20 15v5h-5'],
    map: ['M3 6l6-2 6 2 6-2v14l-6 2-6-2-6 2z', 'M9 4v14', 'M15 6v14'],
    search: ['M10.5 4a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13z', 'M15.5 15.5L20 20'],
    play: ['M7 4l13 8-13 8z'],
    chevron: ['M7 10l5 5 5-5'],
    sun: ['M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8z', 'M12 2.5v2', 'M12 19.5v2', 'M2.5 12h2', 'M19.5 12h2', 'M5.3 5.3l1.4 1.4', 'M17.3 17.3l1.4 1.4', 'M5.3 18.7l1.4-1.4', 'M17.3 6.7l1.4-1.4'],
    moon: ['M19.5 14.5A8 8 0 1 1 9.5 4.5a6.5 6.5 0 0 0 10 10z'],
    folder: ['M3.5 7a1.5 1.5 0 0 1 1.5-1.5h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v8.5a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5z'],
    chat: ['M5 5.5h14a1.5 1.5 0 0 1 1.5 1.5v8.5a1.5 1.5 0 0 1-1.5 1.5H10l-4.5 3.5V17H5a1.5 1.5 0 0 1-1.5-1.5V7A1.5 1.5 0 0 1 5 5.5z', 'M8 10h8', 'M8 13h5'],
    people: ['M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z', 'M2.5 20c.6-3.4 3.2-5.5 6.5-5.5s5.9 2.1 6.5 5.5', 'M16 4.3a3.5 3.5 0 0 1 0 6.4', 'M18 14.8c2 .7 3.2 2.5 3.5 5.2'],
    contrast: ['M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16z', 'M12 4v16', 'M12 8h4.5', 'M12 12h7', 'M12 16h4.5'],
    timer: ['M12 7a7 7 0 1 1 0 14 7 7 0 0 1 0-14z', 'M12 10.5v3.5l2 1.5', 'M10 3h4', 'M12 3v4'],
    vote: ['M12 4a8 8 0 1 1 0 16 8 8 0 0 1 0-16z', 'M8.5 12l2.5 2.5 4.5-5'],
    lock: ['M6 11h12v9H6z', 'M8.5 11V8a3.5 3.5 0 0 1 7 0v3'],
    unlock: ['M6 11h12v9H6z', 'M8.5 11V8a3.5 3.5 0 0 1 6.8-1.2'],
    trash: ['M4 7h16', 'M9 7V4h6v3', 'M6.5 7l1 13h9l1-13'],
    copy: ['M8 8h12v12H8z', 'M16 8V4H4v12h4'],
    front: ['M8 8h12v12H8z', 'M4 4h10v2', 'M4 4v10h2'],
    back: ['M4 4h12v12H4z', 'M18 8h2v12H8v-2'],
    group: ['M3.5 3.5h7v7h-7z', 'M13.5 13.5h7v7h-7z', 'M3 15v6h6', 'M21 9V3h-6'],
    ungroup: ['M3.5 3.5h7v7h-7z', 'M13.5 13.5h7v7h-7z'],
    alignL: ['M4 3v18', 'M8 7h10v4H8z', 'M8 14h6v4H8z'],
    alignC: ['M12 3v18', 'M6 7h12v4H6z', 'M8 14h8v4H8z'],
    alignR: ['M20 3v18', 'M6 7h10v4H6z', 'M10 14h6v4h-6z'],
    alignT: ['M3 4h18', 'M7 8h4v10H7z', 'M14 8h4v6h-4z'],
    alignM: ['M3 12h18', 'M7 6h4v12H7z', 'M14 8h4v8h-4z'],
    alignB: ['M3 20h18', 'M7 6h4v10H7z', 'M14 10h4v6h-4z'],
    distH: ['M4 4v16', 'M20 4v16', 'M10 8h4v8h-4z'],
    distV: ['M4 4h16', 'M4 20h16', 'M8 10h8v4H8z'],
    smile: ['M12 3.5a8.5 8.5 0 1 1 0 17 8.5 8.5 0 0 1 0-17z', 'M8.5 14c1 1.5 2.2 2.2 3.5 2.2s2.5-.7 3.5-2.2', 'M9 9.5v.5', 'M15 9.5v.5'],
    tag: ['M3.5 12.5V4h8.5l8.5 8.5-8.5 8.5z', 'M8 8.5v.5'],
    grid: ['M4 4h.01', 'M12 4h.01', 'M20 4h.01', 'M4 12h.01', 'M12 12h.01', 'M20 12h.01', 'M4 20h.01', 'M12 20h.01', 'M20 20h.01'],
    more: ['M5 12h.01', 'M12 12h.01', 'M19 12h.01'],
    close: ['M6 6l12 12', 'M18 6L6 18'],
    send: ['M4.5 12L19.5 5l-4 14.5-3.5-6z', 'M12 13.5l7.5-8.5'],
    check: ['M5 12.5l4.5 4.5L19 7.5'],
    up: ['M12 19V5', 'M6 11l6-6 6 6'],
    down: ['M12 5v14', 'M6 13l6 6 6-6'],
    left: ['M15 6l-6 6 6 6'],
    right: ['M9 6l6 6-6 6'],
    export: ['M12 15V3', 'M7 8l5-5 5 5', 'M4 14v6h16v-6'],
    bold: ['M7 4h6a4 4 0 0 1 0 8H7z', 'M7 12h7a4 4 0 0 1 0 8H7z'],
    italic: ['M14 4h-4', 'M14 20h-4', 'M14 4l-4 16'],
    arrowEnd: ['M4 12h15', 'M14 7l5 5-5 5'],
    image: ['M3.5 4.5h17v15h-17z', 'M3.5 16l5-5 4 4 3-3 5 5', 'M15.5 8.5h.01'],
    reset: ['M4 12a8 8 0 1 0 2.5-5.8', 'M4 4v5h5'],
    pause: ['M8 5v14', 'M16 5v14'],
    row: ['M3.5 4.5h17v15h-17z', 'M3.5 12h17', 'M12 15v3', 'M10.5 16.5h3'],
    col: ['M3.5 4.5h17v15h-17z', 'M12 4.5v15', 'M15 12h3', 'M16.5 10.5v3'],
    flag: ['M5 21V4', 'M5 4h11l-2 4 2 4H5'],
    link: ['M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1', 'M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1'],
    checklist: ['M4 6l1.5 1.5L8 5', 'M11 6h9', 'M4 12l1.5 1.5L8 11', 'M11 12h9', 'M4.5 16.5h3v3h-3z', 'M11 18h9'],
    crop: ['M6 2v14a2 2 0 0 0 2 2h14', 'M18 22V8a2 2 0 0 0-2-2H2'],
    history: ['M3 12a9 9 0 1 0 2.6-6.4', 'M3 4v4h4', 'M12 7v5l3 2'],
    brush: ['M18.4 2.6a2 2 0 0 1 2.9 2.9L12 14.8 9.2 12z', 'M9 12.5c-2 0-3.5 1.5-3.5 3.5 0 1.2-.8 2.3-2.5 2.5 1 1.7 3 2.5 5 2.5 3 0 4.5-2 4.5-4.5'],
    child: ['M4 6h7', 'M11 6c3 0 3 12 6 12h3', 'M17 15v6'],
  }
  function icon(name, size) {
    const svg = document.createElementNS(NS, 'svg')
    svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('width', size || 20); svg.setAttribute('height', size || 20)
    svg.setAttribute('fill', 'none'); svg.setAttribute('stroke', 'currentColor'); svg.setAttribute('stroke-width', '1.7')
    svg.setAttribute('stroke-linecap', 'round'); svg.setAttribute('stroke-linejoin', 'round'); svg.setAttribute('aria-hidden', 'true')
    for (const d of I[name] || I.more) { const p = document.createElementNS(NS, 'path'); p.setAttribute('d', d); svg.appendChild(p) }
    if (name === 'grid' || name === 'more') svg.setAttribute('stroke-width', '3')
    return svg
  }
  g.BoardIcons = { icon, names: Object.keys(I) }
})(window)
