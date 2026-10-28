// @k2/activity: one model of an agent's Thread turn, one set of words for
// it, and one driver that keeps it live and right (0.46.5 activity SDK,
// review-thread-activity-surface-v1 §2). FSL-1.1-Apache-2.0, Alakazam Labs.
//
// Pure TypeScript, no imports, no DOM: the desktop renderer, the Garden's
// `k2-activity@1` library, the App server SDK (`@k2/server`) and App pages
// (`<k2-activity-bar>`) all build from this one file, so every bar K2 draws
// says the same thing for the same turn.
//
// - `coerceTurn(raw, receivedAt)` reads any turn K2 sends: the owner body,
//   the App guest body (thread.activity, thread_activity frames,
//   k2.thread.onActivity), schema 1 (0.46.3–0.46.4) and schema 2 (0.46.5:
//   `stepKind`, `childCounts`, `openBecause`, `evidenceAt`). Unknown words
//   from a newer server stay `unknown` and draw as a quiet "Working" (W9).
// - `reduceTurn(current, incoming)`: one turn moves only forward by `rev`;
//   another `turnId` replaces it (W2).
// - `stripView` / `drawerLine` / `describe`: the words (one copy, moved
//   from the desktop's threadStrip.ts). `monitoring` never animates.
// - `watchActivity(transport, cb, opts)`: socket first, snapshot on every
//   open, frames newer than the snapshot win, re-read on reset and on a
//   60 s self-check, at most one read a second, 429 back-off (W1–W8).
//
// NEVER infer a turn: not from terminal output, not from "I sent and no
// reply came yet", not from the agent's reply (a reply doesn't end a turn).
// No turn = no bar.
// ── Model ───────────────────────────────────────────────────────────────
/** The newest turn schema this file reads. */
export const ACTIVITY_SCHEMA = 2;
const STATES = ['working', 'monitoring', 'needs-you', 'unverifiable', 'stopped', 'idle'];
const PHASES = ['delivering', 'working', 'tool', 'thinking', 'waiting', 'stale', 'children'];
const STEP_KINDS = ['read', 'search', 'cmd', 'edit', 'web', 'subagent', 'tool'];
const OPEN_BECAUSE = ['lead', 'children', 'waiting', 'stale'];
const CHILD_KINDS = ['subagent', 'shell', 'monitor', 'task'];
const CHILD_STATES = ['running', 'waiting', 'done', 'owed'];
function isObj(v) {
    return v !== null && typeof v === 'object' && !Array.isArray(v);
}
function finite(v) {
    return typeof v === 'number' && Number.isFinite(v) ? v : null;
}
function count(v) {
    const n = finite(v);
    return n !== null && n > 0 ? Math.floor(n) : 0;
}
function word(list, v) {
    return list.find((x) => x === v) ?? null;
}
/** The step kind an owner's tool line names (a schema-1 owner frame). */
function kindFromLine(line) {
    if (!line)
        return null;
    if (/^Read(ing)?\b/.test(line))
        return 'read';
    if (/^(Search(ing)?|Grep|Glob|Find(ing)?)\b/.test(line))
        return 'search';
    if (/^(Run(ning)?|Ran)\b/.test(line))
        return 'cmd';
    if (/^(Edit(ing)?|Writ(e|ing))\b/.test(line))
        return 'edit';
    if (/^(Fetch(ing)?|Brows(e|ing))\b/.test(line))
        return 'web';
    return 'tool';
}
function coerceChildren(raw, fallbackSince) {
    if (!Array.isArray(raw))
        return null;
    const out = [];
    for (const c of raw) {
        if (!isObj(c))
            continue;
        const kind = word(CHILD_KINDS, c.kind);
        const state = word(CHILD_STATES, c.state);
        if (!kind || !state)
            continue;
        const label = typeof c.label === 'string' && c.label.trim() ? c.label.trim() : null;
        out.push({ kind, state, label, since: finite(c.since) ?? fallbackSince });
    }
    return out;
}
function childCountsOf(a, children, subagents, background) {
    if (isObj(a.childCounts)) {
        const c = a.childCounts;
        return { subagent: count(c.subagent), shell: count(c.shell), monitor: count(c.monitor), task: count(c.task) };
    }
    if (children) {
        const live = children.filter((c) => c.state === 'running' || c.state === 'waiting');
        const n = (k) => live.filter((c) => c.kind === k).length;
        return { subagent: n('subagent'), shell: n('shell'), monitor: n('monitor'), task: n('task') };
    }
    // A guest's schema-1 body: background tasks of no named kind.
    return { subagent: subagents, shell: 0, monitor: 0, task: background };
}
function openBecauseOf(raw, phase, ended) {
    if (ended)
        return null;
    const said = word(OPEN_BECAUSE, raw);
    if (said)
        return said;
    if (phase === 'waiting')
        return 'waiting';
    if (phase === 'stale')
        return 'stale';
    if (phase === 'children')
        return 'children';
    return 'lead';
}
/**
 * One turn body as K2 sends it (any viewer, schema 1 or 2), or null when
 * it isn't one. `receivedAt` is this client's clock when it arrived. An
 * object that is already a coerced turn (or the renderer's own ThreadTurn,
 * with `skewMs` and `line`) reads the same.
 */
export function coerceTurn(raw, receivedAt) {
    if (!isObj(raw))
        return null;
    const a = raw;
    const turnId = typeof a.turnId === 'string' ? a.turnId : '';
    if (!turnId)
        return null;
    // A turn already coerced (it carries `skewMs`) keeps its own skew; a
    // wire body's skew is its `serverNow` against this arrival.
    const givenSkew = finite(a.skewMs);
    const wireNow = finite(a.serverNow);
    const skewMs = givenSkew ?? (wireNow !== null ? wireNow - receivedAt : 0);
    const serverNow = wireNow ?? receivedAt + skewMs;
    const startedAt = finite(a.startedAt) ?? finite(a.phaseSince) ?? serverNow;
    const state = word(STATES, a.state) ?? 'unknown';
    const phase = word(PHASES, a.phase) ?? 'unknown';
    const tally = isObj(a.tally) ? a.tally : {};
    let end = null;
    if (isObj(a.end)) {
        end = {
            reason: typeof a.end.reason === 'string' ? a.end.reason : 'done',
            detail: typeof a.end.detail === 'string' ? a.end.detail : null,
            at: finite(a.end.at) ?? serverNow,
        };
    }
    const stepIn = isObj(a.step) ? a.step : null;
    const line = typeof a.line === 'string' && a.line.trim()
        ? a.line
        : stepIn && typeof stepIn.text === 'string' && stepIn.text.trim()
            ? stepIn.text
            : null;
    const children = coerceChildren(a.children, startedAt);
    const subagents = count(a.subagents);
    const background = count(a.background);
    // The owner body always carries `line` (null between tools); a guest's
    // never does. A coerced turn says which it was.
    const owner = a.viewer === 'owner' || (a.viewer !== 'guest' && ('line' in a || children !== null));
    const stepKind = word(STEP_KINDS, a.stepKind) ?? (stepIn ? word(STEP_KINDS, stepIn.kind) : null);
    const waiting = a.waitingOn;
    const schema = a.schema === 1 || a.schema === 2 ? a.schema : 'stepKind' in a || 'openBecause' in a || 'childCounts' in a ? 2 : 1;
    return {
        turnId,
        rev: finite(a.rev) ?? 0,
        schema,
        state,
        phase,
        openBecause: openBecauseOf(a.openBecause, phase, end !== null),
        startedAt,
        lastComposeAt: finite(a.lastComposeAt) ?? startedAt,
        phaseSince: finite(a.phaseSince) ?? startedAt,
        evidenceAt: finite(a.evidenceAt),
        skewMs,
        serverNow,
        step: {
            kind: phase === 'tool' ? stepKind ?? kindFromLine(line) : null,
            text: phase === 'tool' ? line : null,
        },
        tally: { read: count(tally.read), search: count(tally.search), cmd: count(tally.cmd), edit: count(tally.edit) },
        subagents,
        subagentsDone: count(a.subagentsDone),
        background,
        childCounts: childCountsOf(a, children, subagents, background),
        children,
        messages: Math.max(1, count(a.messages)),
        replied: typeof a.replied === 'boolean' ? a.replied : null,
        waitingOn: waiting === 'permission' || waiting === 'question' || waiting === 'owner' ? waiting : null,
        end,
        viewer: owner ? 'owner' : 'guest',
    };
}
/** A newer state of the turn wins: another turn replaces it, the same
 *  turn moves only forward by `rev` (W2). */
export function reduceTurn(current, incoming) {
    if (!incoming)
        return current;
    if (current && current.turnId === incoming.turnId && incoming.rev < current.rev)
        return current;
    return incoming;
}
/** The turn is running (not ended). */
export function isLive(turn) {
    return !!turn && turn.end === null;
}
// ── Words ───────────────────────────────────────────────────────────────
// One copy for every bar K2 draws (desktop drawer and strip, Zen, the
// Garden library, App pages). Moved from SessionView/threadStrip.ts.
/** One word per state, everywhere K2 names one (the Agents widget's dot,
 *  the drawer's head, a bar's aria): the Thread turn's and an agent row's
 *  vocabulary. `unknown` (a newer server's word) reads as Working. */
export const STATE_WORDS = {
    working: 'Working',
    monitoring: 'Monitoring',
    'needs-you': 'Needs you',
    unverifiable: 'No update',
    stopped: 'Stopped',
    idle: 'Idle',
    unknown: 'Working',
};
/** How long "Stopped" stays after an interrupt. */
export const STOPPED_LINGER_MS = 3_000;
/** A turn with no sign of life reads "No update" after this (row.rs). */
export const STALE_AFTER_MS = 30 * 60_000;
/** "8s", "1m 05s", "1h 02m". */
export function stripClock(ms) {
    const sec = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = sec % 60;
    if (h)
        return `${h}h ${String(m).padStart(2, '0')}m`;
    if (m)
        return `${m}m ${String(s).padStart(2, '0')}s`;
    return `${s}s`;
}
/** The clock at the far right: "0:08", "1:12", "1:02:03". */
export function stripClockShort(ms) {
    const sec = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = String(sec % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}
function plural(n, one, many) {
    return `${n} ${n === 1 ? one : many}`;
}
/** Minutes with no sign of life: from `evidenceAt`, else from the stale
 *  phase's start less the stale wait. */
export function minutesQuiet(evidenceAt, staleSince, now, skewMs = 0) {
    const from = evidenceAt ?? (staleSince !== null ? staleSince - STALE_AFTER_MS : null);
    if (from === null)
        return 0;
    return Math.max(0, Math.floor((now + skewMs - from) / 60_000));
}
/** "Read 3 files", "searched 1 time", "ran 2 commands", "edited 1 file". */
export function tallyItems(t) {
    const out = [];
    if (t.read)
        out.push({ key: 'read', text: `read ${plural(t.read, 'file', 'files')}` });
    if (t.search)
        out.push({ key: 'search', text: `searched ${plural(t.search, 'time', 'times')}` });
    if (t.cmd)
        out.push({ key: 'cmd', text: `ran ${plural(t.cmd, 'command', 'commands')}` });
    if (t.edit)
        out.push({ key: 'edit', text: `edited ${plural(t.edit, 'file', 'files')}` });
    const first = out[0];
    if (first)
        out[0] = { ...first, text: first.text.charAt(0).toUpperCase() + first.text.slice(1) };
    return out;
}
/** One named child in words: "Explore subagent running", "Running `npm
 *  run dev` in the background". */
export function childText(c) {
    if (c.kind === 'subagent') {
        const name = c.label ? `${c.label} subagent` : 'A subagent';
        const state = c.state === 'waiting' ? 'waiting on a prompt' : c.state === 'done' || c.state === 'owed' ? 'done' : 'running';
        return `${name} ${state}`;
    }
    const name = c.label ?? (c.kind === 'monitor' ? 'A monitor' : c.kind === 'task' ? 'A task' : 'A background task');
    return `${name} in the background`;
}
/** What holds the turn: by name when the frame names its children, else
 *  from the counts (a guest, or an older server). */
export function childItems(turn) {
    if (turn.children && turn.children.length > 0) {
        const named = turn.children.map((c) => ({ key: c.kind === 'subagent' ? 'subagents' : 'background', text: childText(c) }));
        const listedDone = turn.children.filter((c) => c.kind === 'subagent' && (c.state === 'done' || c.state === 'owed')).length;
        const done = turn.subagentsDone - listedDone;
        if (done > 0)
            named.push({ key: 'subagents', text: `${plural(done, 'subagent', 'subagents')} done` });
        return named;
    }
    const out = [];
    if (turn.subagents > 0) {
        const done = turn.subagentsDone > 0 ? `, ${turn.subagentsDone} done` : '';
        out.push({ key: 'subagents', text: `${plural(turn.subagents, 'subagent', 'subagents')} running${done}` });
    }
    else if (turn.subagentsDone > 0) {
        out.push({ key: 'subagents', text: `${plural(turn.subagentsDone, 'subagent', 'subagents')} done` });
    }
    if (turn.background > 0)
        out.push({ key: 'background', text: plural(turn.background, 'background task', 'background tasks') });
    return out;
}
/** A guest's step in words, from the step kind alone (never a line). */
export function stepKindText(kind) {
    switch (kind) {
        case 'read':
            return 'Reading files';
        case 'search':
            return 'Searching';
        case 'cmd':
            return 'Running a command';
        case 'edit':
            return 'Editing files';
        case 'web':
            return 'Reading the web';
        case 'subagent':
            return 'Starting a subagent';
        case 'tool':
            return 'Using a tool';
        case null:
            return null;
    }
}
function stepOf(turn, serverNow) {
    const inPhase = stripClock(serverNow - turn.phaseSince);
    switch (turn.phase) {
        case 'tool': {
            if (turn.step.text)
                return { kind: 'line', text: turn.step.text, elapsed: inPhase };
            // An owner between lines keeps "Working" (the desktop's words); a
            // guest, who never gets a line, reads the step's kind.
            if (turn.viewer === 'owner')
                return null;
            const said = stepKindText(turn.step.kind);
            return said ? { kind: 'plain', text: said, elapsed: inPhase } : null;
        }
        case 'thinking':
            return { kind: 'thinking', text: `Thinking… ${inPhase}`, elapsed: null };
        case 'waiting':
            return {
                kind: 'needs',
                text: turn.waitingOn === 'question'
                    ? 'Stuck on a question'
                    : turn.waitingOn === 'owner'
                        ? 'Waiting on the owner'
                        : 'Stuck on a permission prompt',
                elapsed: inPhase,
            };
        case 'delivering':
            return { kind: 'plain', text: 'Delivering…', elapsed: null };
        case 'working':
        case 'stale':
        case 'children':
        case 'unknown':
            return null;
    }
}
function spokenStep(step) {
    return step ? `: ${step.text.replace(/`/g, '')}` : '';
}
/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state a bar draws). */
export function stripView(turn, now) {
    const serverNow = now + turn.skewMs;
    if (turn.end) {
        if (turn.end.reason !== 'interrupted' || serverNow - turn.end.at >= STOPPED_LINGER_MS)
            return null;
        const since = `you stopped it after ${stripClock(turn.end.at - turn.startedAt)}`;
        return { tone: 'stopped', label: 'Stopped', since, sinceTitle: null, ariaLabel: `Stopped, ${since}`, step: null, tally: [], stoppable: false };
    }
    const clock = stripClock(serverNow - turn.startedAt);
    const sinceMessage = `${clock} since your message`;
    const children = childItems(turn).map((i) => i.text);
    const tallyText = tallyItems(turn.tally).map((i) => i.text);
    const tally = [...tallyText, ...children];
    const step = stepOf(turn, serverNow);
    if (turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring')) {
        const label = children.length ? children.join(' · ') : 'Background work';
        return {
            tone: turn.state,
            label,
            since: clock,
            sinceTitle: 'since your message',
            ariaLabel: `${turn.replied === false ? 'Agent' : 'Agent replied'}; ${label.charAt(0).toLowerCase()}${label.slice(1)}, ${sinceMessage}`,
            step: null,
            tally: tallyText,
            stoppable: false,
        };
    }
    switch (turn.state) {
        case 'working':
        case 'monitoring':
        case 'unknown': {
            // The pulse says "working"; the word only stands in for an unknown
            // step. Monitoring keeps its word: it tells you something. A word
            // from a newer server reads "Working" (W9).
            const tone = turn.state === 'monitoring' ? 'monitoring' : 'working';
            const w = tone === 'working' ? 'Working' : 'Monitoring';
            return {
                tone,
                label: tone === 'monitoring' || !step || turn.state === 'unknown' ? w : '',
                since: clock,
                sinceTitle: 'since your message',
                ariaLabel: `Agent ${w.toLowerCase()}, ${sinceMessage}${spokenStep(step)}`,
                step,
                tally,
                stoppable: turn.viewer === 'owner' && turn.state !== 'unknown',
            };
        }
        case 'needs-you':
            return {
                tone: 'needs-you',
                label: 'Needs you',
                since: clock,
                sinceTitle: 'since your message',
                ariaLabel: `Agent needs you, ${sinceMessage}${spokenStep(step)}`,
                step,
                tally,
                stoppable: false,
            };
        case 'unverifiable': {
            const minutes = minutesQuiet(turn.evidenceAt, turn.phaseSince, now, turn.skewMs);
            return {
                tone: 'unverifiable',
                label: `No update in ${minutes}m`,
                since: 'the session is still open',
                sinceTitle: null,
                ariaLabel: `No update in ${minutes}m, the session is still open`,
                step: null,
                tally,
                stoppable: false,
            };
        }
        case 'stopped':
        case 'idle':
            return null;
    }
}
// ── The one line (the drawer's collapsed line and its detail) ───────────
/** Display order, left to right. */
const COUNTER_ORDER = ['subagents', 'background', 'read', 'search', 'cmd', 'edit'];
/** Non-zero counters, in display order. */
export function counters(turn) {
    const counts = {
        subagents: turn.subagents,
        background: turn.background,
        read: turn.tally.read,
        search: turn.tally.search,
        cmd: turn.tally.cmd,
        edit: turn.tally.edit,
    };
    const out = [];
    for (const key of COUNTER_ORDER) {
        const n = Math.max(0, counts[key] || 0);
        const done = key === 'subagents' ? Math.max(0, turn.subagentsDone || 0) : 0;
        if (n > 0 || done > 0)
            out.push({ key, count: n, done });
    }
    return out;
}
/** The counter in words, for screen readers. */
export function counterSpoken(c) {
    switch (c.key) {
        case 'subagents':
            if (c.count > 0) {
                const done = c.done > 0 ? `, ${c.done} done` : '';
                return `${plural(c.count, 'subagent', 'subagents')} running${done}`;
            }
            return `${plural(c.done, 'subagent', 'subagents')} done`;
        case 'background':
            return plural(c.count, 'background task', 'background tasks');
        case 'read':
            return `read ${plural(c.count, 'file', 'files')}`;
        case 'search':
            return `searched ${plural(c.count, 'time', 'times')}`;
        case 'cmd':
            return `ran ${plural(c.count, 'command', 'commands')}`;
        case 'edit':
            return `edited ${plural(c.count, 'file', 'files')}`;
    }
}
/** The one line for `turn` at `now` (this client's clock), or null. */
export function drawerLine(turn, now) {
    const view = stripView(turn, now);
    if (!view)
        return null;
    const tallyList = tallyItems(turn.tally);
    const childList = childItems(turn);
    const tally = tallyList.map((i) => i.text).join(' · ');
    const children = childList.map((i) => i.text).join(' · ');
    if (turn.end) {
        return {
            tone: view.tone,
            ariaLabel: view.ariaLabel,
            step: { kind: 'plain', text: 'Stopped', elapsed: null },
            counters: [],
            clock: stripClockShort(turn.end.at - turn.startedAt),
            detail: { head: `Stopped after ${stripClock(turn.end.at - turn.startedAt)}`, step: null, tally, children, tallyItems: tallyList, childItems: childList },
        };
    }
    const serverNow = now + turn.skewMs;
    const sinceMessage = `${stripClock(serverNow - turn.startedAt)} since your message`;
    const replied = turn.phase === 'children' && (turn.state === 'working' || turn.state === 'monitoring');
    let w;
    let step;
    if (replied) {
        // What holds the turn, not only that the agent replied. A frame that
        // doesn't say (`replied` null) only sent this phase after a reply.
        const said = turn.replied !== false;
        w = said ? 'Replied' : turn.state === 'monitoring' ? 'Monitoring' : 'Working';
        const first = childList[0]?.text;
        const more = childList.length > 1 ? ` +${childList.length - 1}` : '';
        const what = first ? `${first}${more}` : null;
        const text = said ? (what ? `Replied · ${what}` : 'Replied') : what ?? w;
        step = { kind: text.includes('`') ? 'line' : 'plain', text, elapsed: null };
    }
    else if (view.tone === 'unverifiable') {
        w = view.label;
        step = { kind: 'plain', text: view.label, elapsed: null };
    }
    else {
        w = turn.state === 'needs-you' ? 'Needs you' : turn.state === 'monitoring' ? 'Monitoring' : 'Working';
        step = view.step ?? { kind: 'plain', text: view.label || w, elapsed: null };
    }
    return {
        tone: view.tone,
        ariaLabel: view.ariaLabel,
        step,
        counters: counters(turn),
        clock: stripClockShort(serverNow - turn.startedAt),
        detail: {
            head: `${w} · ${sinceMessage}`,
            step: replied || view.tone === 'unverifiable' ? null : view.step,
            tally,
            children,
            tallyItems: tallyList,
            childItems: childList,
        },
    };
}
/** The end words a guest and an owner both read. */
export function endWord(reason) {
    if (reason === 'reply' || reason === 'done')
        return 'Done';
    if (reason === 'interrupted' || reason === 'superseded' || reason === 'stale' || reason === 'stopped')
        return 'Stopped';
    return 'Failed';
}
/** The bar for `turn` at `now`, or null: nothing to draw. An end frame is
 *  described (Done / Stopped / Failed) so a bar can linger on it. */
export function describe(turn, now) {
    if (!turn)
        return null;
    if (turn.end) {
        const w = endWord(turn.end.reason);
        const tone = w === 'Done' ? 'done' : w === 'Failed' ? 'failed' : 'stopped';
        const took = stripClock(turn.end.at - turn.startedAt);
        return {
            tone,
            headline: w,
            headlineKind: 'plain',
            animate: false,
            counters: [],
            clock: stripClockShort(turn.end.at - turn.startedAt),
            aria: `${w} after ${took}`,
            detail: [`${w} after ${took}`, ...tallyItems(turn.tally).map((i) => i.text)],
            ended: true,
        };
    }
    const line = drawerLine(turn, now);
    if (!line)
        return null;
    const detail = [line.detail.head];
    if (line.detail.step)
        detail.push(line.detail.step.text);
    if (line.detail.tally)
        detail.push(line.detail.tally);
    if (line.detail.children)
        detail.push(line.detail.children);
    return {
        tone: line.tone,
        headline: line.step.text,
        headlineKind: line.step.kind,
        animate: line.tone === 'working' && turn.state !== 'unknown',
        counters: line.counters,
        clock: line.clock,
        aria: line.ariaLabel,
        detail,
        ended: false,
    };
}
export const SELF_CHECK_MS = 60_000;
/** A 429's wait in ms: `retryAfterMs`, or `retryAfterSecs` (the server
 *  SDK's K2Error), on the error or inside its `body` / `error`. */
function retryAfter(err) {
    for (const o of [err, isObj(err) ? err.body : null, isObj(err) ? err.error : null]) {
        if (!isObj(o))
            continue;
        const ms = finite(o.retryAfterMs);
        if (ms !== null)
            return ms;
        const secs = finite(o.retryAfterSecs);
        if (secs !== null)
            return secs * 1000;
    }
    return null;
}
function defaultVisible(fn) {
    const d = globalThis.document;
    if (!d || typeof d.addEventListener !== 'function' || typeof d.removeEventListener !== 'function')
        return () => undefined;
    const h = () => {
        if (d.visibilityState === 'visible')
            fn();
    };
    d.addEventListener('visibilitychange', h);
    return () => d.removeEventListener?.('visibilitychange', h);
}
/**
 * Keep every key's turn live and right. `cb` gets a fresh Map on every
 * change (key → ActivityState). Returns the unsubscribe: it closes the
 * socket, clears every timer and calls nothing after it returns (W8).
 */
export function watchActivity(transport, cb, opts = {}) {
    const now = opts.now ?? (() => Date.now());
    const setT = opts.setTimeout ?? ((fn, ms) => setTimeout(fn, ms));
    const clearT = opts.clearTimeout ?? ((t) => clearTimeout(t));
    const linger = Math.max(0, opts.endLingerMs ?? 0);
    const selfCheck = Math.max(SELF_CHECK_MS, opts.selfCheckMs ?? SELF_CHECK_MS);
    const minGap = Math.max(0, opts.minReadGapMs ?? 1000);
    let stopped = false;
    let fresh = 'catching-up';
    const turns = new Map();
    /** Frames seen per key: a read older than a frame never overwrites it (W1). */
    const gen = new Map();
    const lastFrameAt = new Map();
    const lingerTimers = new Map();
    let reading = false;
    let again = false;
    let lastReadAt = -Infinity;
    let readTimer = null;
    let checkTimer = null;
    const emit = () => {
        if (stopped)
            return;
        const out = new Map();
        for (const [k, t] of turns)
            out.set(k, { turn: t, live: isLive(t), source: 'thread', fresh });
        cb(out);
    };
    const setTurn = (key, t) => {
        const old = lingerTimers.get(key);
        if (old !== undefined) {
            clearT(old);
            lingerTimers.delete(key);
        }
        if (t && t.end) {
            if (linger === 0) {
                turns.set(key, null);
                return;
            }
            turns.set(key, t);
            lingerTimers.set(key, setT(() => {
                lingerTimers.delete(key);
                if (stopped || turns.get(key) !== t)
                    return;
                turns.set(key, null);
                emit();
            }, linger));
            return;
        }
        turns.set(key, t);
    };
    /** REPLACE each key's turn with the snapshot's (W1): an ended turn reads
     *  as none; a key whose frames moved on since `since` keeps its frame. */
    const applySnapshot = (snap, since) => {
        const at = now();
        for (const [key, raw] of Object.entries(snap)) {
            if (since && (gen.get(key) ?? 0) !== (since.get(key) ?? 0))
                continue; // a frame came after the read began: it wins
            const t = raw === null || raw === undefined ? null : coerceTurn(raw, at);
            if (raw !== null && raw !== undefined && !t)
                continue;
            setTurn(key, t && t.end ? null : t);
            lastFrameAt.set(key, at);
        }
        fresh = 'live';
        emit();
    };
    const read = () => {
        if (stopped)
            return;
        if (reading) {
            again = true;
            return;
        }
        const wait = lastReadAt + minGap - now();
        if (wait > 0) {
            if (readTimer === null) {
                readTimer = setT(() => {
                    readTimer = null;
                    read();
                }, wait);
            }
            return;
        }
        reading = true;
        again = false;
        lastReadAt = now();
        const startGen = new Map(gen);
        transport.read().then((snap) => {
            reading = false;
            if (stopped)
                return;
            applySnapshot(snap, startGen);
            if (again)
                read();
        }, (err) => {
            reading = false;
            if (stopped)
                return;
            const ms = retryAfter(err);
            if (ms === null)
                opts.onError?.(err);
            // 429: wait as told (at least the gap), then read again.
            const wait = Math.max(minGap, ms ?? 5_000);
            if (readTimer === null) {
                readTimer = setT(() => {
                    readTimer = null;
                    read();
                }, wait);
            }
        });
    };
    const scheduleCheck = () => {
        checkTimer = setT(() => {
            checkTimer = null;
            if (stopped)
                return;
            const t = now();
            for (const [key, turn] of turns) {
                if (isLive(turn) && t - (lastFrameAt.get(key) ?? 0) >= selfCheck) {
                    read();
                    break;
                }
            }
            scheduleCheck();
        }, selfCheck);
    };
    const close = transport.connect({
        open: () => {
            if (stopped)
                return;
            fresh = 'catching-up';
            emit();
            read();
        },
        frame: (key, raw) => {
            if (stopped)
                return;
            const t = coerceTurn(raw, now());
            if (!t)
                return;
            gen.set(key, (gen.get(key) ?? 0) + 1);
            lastFrameAt.set(key, now());
            const cur = turns.get(key) ?? null;
            const next = reduceTurn(cur, t);
            if (next === cur)
                return;
            setTurn(key, next);
            emit();
        },
        reset: (snapshot) => {
            if (stopped)
                return;
            if (snapshot)
                applySnapshot(snapshot, null);
            else
                read();
        },
        drop: () => {
            if (stopped)
                return;
            fresh = 'retrying';
            emit();
        },
    });
    scheduleCheck();
    const offVisible = (opts.onVisible ?? defaultVisible)(() => {
        for (const t of turns.values()) {
            if (isLive(t)) {
                read();
                return;
            }
        }
    });
    return () => {
        if (stopped)
            return;
        stopped = true;
        if (readTimer !== null)
            clearT(readTimer);
        if (checkTimer !== null)
            clearT(checkTimer);
        for (const t of lingerTimers.values())
            clearT(t);
        lingerTimers.clear();
        offVisible();
        close();
    };
}
