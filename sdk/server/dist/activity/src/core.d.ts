/** The newest turn schema this file reads. */
export declare const ACTIVITY_SCHEMA = 2;
export type TurnState = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped' | 'idle' | 'unknown';
export type TurnPhase = 'delivering' | 'working' | 'tool' | 'thinking' | 'waiting' | 'stale' | 'children' | 'unknown';
export type StepKind = 'read' | 'search' | 'cmd' | 'edit' | 'web' | 'subagent' | 'tool';
export type OpenBecause = 'lead' | 'children' | 'waiting' | 'stale';
export type ChildKind = 'subagent' | 'shell' | 'monitor' | 'task';
/** `done` / `owed`: a subagent that finished whose result the agent hasn't taken yet. */
export type ChildState = 'running' | 'waiting' | 'done' | 'owed';
export interface TurnTally {
    read: number;
    search: number;
    cmd: number;
    edit: number;
}
export interface TurnChild {
    kind: ChildKind;
    state: ChildState;
    /** Owner frames only: a subagent's type, a shell's redacted line. */
    label: string | null;
    /** Server ms. */
    since: number;
}
export interface ChildCounts {
    subagent: number;
    shell: number;
    monitor: number;
    task: number;
}
export interface TurnEnd {
    /** Owner: reply | done | interrupted | failed | session_gone | delivery_failed
     *  | superseded | stale. Guest: done | stopped | failed. */
    reason: string;
    /** Owner only. */
    detail: string | null;
    /** Server ms. */
    at: number;
}
export interface Turn {
    turnId: string;
    rev: number;
    /** 1: a 0.46.3–0.46.4 body; 2: it names its step kind and why it is open. */
    schema: 1 | 2;
    state: TurnState;
    phase: TurnPhase;
    /** Why the turn is still open; null once it ended. */
    openBecause: OpenBecause | null;
    /** Server ms: the person's (first) message. */
    startedAt: number;
    /** Server ms: the newest message that joined the turn. */
    lastComposeAt: number;
    /** Server ms: the current phase. */
    phaseSince: number;
    /** Server ms: the newest sign of life, when the server says (schema 2). */
    evidenceAt: number | null;
    /** `serverNow − Date.now()` when the frame arrived: server times read on
     *  this client's clock as `Date.now() + skewMs` (W6). */
    skewMs: number;
    /** Server ms: when the frame was built. */
    serverNow: number;
    /** `kind` while the phase is `tool` (schema 2, or read from the owner's
     *  line); `text`: the owner's redacted tool line (never for a guest). */
    step: {
        kind: StepKind | null;
        text: string | null;
    };
    tally: TurnTally;
    subagents: number;
    subagentsDone: number;
    background: number;
    childCounts: ChildCounts;
    /** By name, owner frames only; null when the frame has none. */
    children: TurnChild[] | null;
    messages: number;
    /** The agent posted in Thread during this turn; null when not said. */
    replied: boolean | null;
    /** `owner`: a guest's coarse word ("waiting on the owner"). */
    waitingOn: 'permission' | 'question' | 'owner' | null;
    end: TurnEnd | null;
    /** Owner frames carry text (a line, labels, an end detail); guest frames never. */
    viewer: 'owner' | 'guest';
}
/** What a watcher hands its callback for one agent or room. */
export interface ActivityState {
    /** The turn, or null: nothing is running (never a guess). */
    turn: Turn | null;
    /** A turn is running (not ended). */
    live: boolean;
    /** `thread`: the server reports Thread turns; `room`: an older server's
     *  room-level guess, labelled as one; `none`: no turns at all. */
    source: 'thread' | 'room' | 'none';
    fresh: 'live' | 'catching-up' | 'retrying';
}
/**
 * One turn body as K2 sends it (any viewer, schema 1 or 2), or null when
 * it isn't one. `receivedAt` is this client's clock when it arrived. An
 * object that is already a coerced turn (or the renderer's own ThreadTurn,
 * with `skewMs` and `line`) reads the same.
 */
export declare function coerceTurn(raw: unknown, receivedAt: number): Turn | null;
/** A newer state of the turn wins: another turn replaces it, the same
 *  turn moves only forward by `rev` (W2). */
export declare function reduceTurn(current: Turn | null, incoming: Turn | null): Turn | null;
/** The turn is running (not ended). */
export declare function isLive(turn: Turn | null | undefined): turn is Turn;
/** One word per state, everywhere K2 names one (the Agents widget's dot,
 *  the drawer's head, a bar's aria): the Thread turn's and an agent row's
 *  vocabulary. `unknown` (a newer server's word) reads as Working. */
export declare const STATE_WORDS: {
    readonly working: "Working";
    readonly monitoring: "Monitoring";
    readonly 'needs-you': "Needs you";
    readonly unverifiable: "No update";
    readonly stopped: "Stopped";
    readonly idle: "Idle";
    readonly unknown: "Working";
};
/** How long "Stopped" stays after an interrupt. */
export declare const STOPPED_LINGER_MS = 3000;
/** A turn with no sign of life reads "No update" after this (row.rs). */
export declare const STALE_AFTER_MS: number;
export type Tone = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped';
export interface StripStep {
    /** `line`: a tool line with `code` spans; `thinking` shimmers. */
    kind: 'line' | 'thinking' | 'plain' | 'needs';
    text: string;
    /** Time in this step ("12s"), or null when the text carries it. */
    elapsed: string | null;
}
export interface StripView {
    tone: Tone;
    /** The state word, only where it adds something. */
    label: string;
    since: string;
    sinceTitle: string | null;
    ariaLabel: string;
    step: StripStep | null;
    tally: string[];
    /** The owner may press Stop (Esc to the session). */
    stoppable: boolean;
}
/** "8s", "1m 05s", "1h 02m". */
export declare function stripClock(ms: number): string;
/** The clock at the far right: "0:08", "1:12", "1:02:03". */
export declare function stripClockShort(ms: number): string;
/** Minutes with no sign of life: from `evidenceAt`, else from the stale
 *  phase's start less the stale wait. */
export declare function minutesQuiet(evidenceAt: number | null, staleSince: number | null, now: number, skewMs?: number): number;
export type CounterKey = 'subagents' | 'background' | 'read' | 'search' | 'cmd' | 'edit';
/** One expanded-detail bit with the counter it explains. */
export interface DetailItem {
    key: CounterKey;
    text: string;
}
/** "Read 3 files", "searched 1 time", "ran 2 commands", "edited 1 file". */
export declare function tallyItems(t: TurnTally): DetailItem[];
/** One named child in words: "Explore subagent running", "Running `npm
 *  run dev` in the background". */
export declare function childText(c: Pick<TurnChild, 'kind' | 'state' | 'label'>): string;
/** What holds the turn: by name when the frame names its children, else
 *  from the counts (a guest, or an older server). */
export declare function childItems(turn: Pick<Turn, 'children' | 'subagents' | 'subagentsDone' | 'background'>): DetailItem[];
/** A guest's step in words, from the step kind alone (never a line). */
export declare function stepKindText(kind: StepKind | null): string | null;
/** The strip for `turn` at `now` (this client's clock), or null: nothing
 *  to show (the turn ended, or it is in no state a bar draws). */
export declare function stripView(turn: Turn, now: number): StripView | null;
export interface Counter {
    key: CounterKey;
    /** Subagents: the running ones. */
    count: number;
    /** Subagents only: finished ones (the check group). */
    done: number;
}
/** Non-zero counters, in display order. */
export declare function counters(turn: Pick<Turn, 'subagents' | 'subagentsDone' | 'background' | 'tally'>): Counter[];
/** The counter in words, for screen readers. */
export declare function counterSpoken(c: Counter): string;
export interface DrawerLine {
    tone: Tone;
    ariaLabel: string;
    /** The middle of the collapsed line: the step, or the state word. */
    step: StripStep;
    counters: Counter[];
    /** Far right, "1:12". */
    clock: string;
    detail: {
        head: string;
        step: StripStep | null;
        tally: string;
        children: string;
        tallyItems: DetailItem[];
        childItems: DetailItem[];
    };
}
/** The one line for `turn` at `now` (this client's clock), or null. */
export declare function drawerLine(turn: Turn, now: number): DrawerLine | null;
/** Everything a bar draws for one turn, in one object. */
export interface Description {
    tone: Tone | 'done' | 'failed';
    /** The collapsed line's words ("Running `cargo test`", "Thinking… 4s",
     *  "Replied · Explore subagent running", "Waiting on the owner"). */
    headline: string;
    /** `line` text carries `code` spans in backticks. */
    headlineKind: StripStep['kind'];
    /** Pulse only while the agent itself works: never for monitoring,
     *  needs-you, a quiet turn or an end (never typing dots). */
    animate: boolean;
    counters: Counter[];
    clock: string;
    aria: string;
    detail: string[];
    /** An end frame ("Done", "Stopped", "Failed"), shown for `endLingerMs`. */
    ended: boolean;
}
/** The end words a guest and an owner both read. */
export declare function endWord(reason: string): 'Done' | 'Stopped' | 'Failed';
/** The bar for `turn` at `now`, or null: nothing to draw. An end frame is
 *  described (Done / Stopped / Failed) so a bar can linger on it. */
export declare function describe(turn: Turn | null, now: number): Description | null;
/** A key's turn as a read answered it: the raw body or null. */
export type Snapshot = Record<string, unknown>;
/** What the driver needs from a transport (an App socket + GET, the
 *  Garden bridge, the desktop's overlay socket). */
export interface ActivityTransport {
    /** Open the live socket and keep it open (reconnecting on its own).
     *  Returns close. Every handler may be called any number of times. */
    connect(on: {
        /** The socket is open (each (re)open): the driver reads a snapshot. */
        open(): void;
        /** One turn frame for `key` (a room id, an address). */
        frame(key: string, raw: unknown): void;
        /** The server dropped frames (`activity_reset`, 0.46.5) or said the
         *  set changed. With the snapshot the reset frame carries (the
         *  overlay socket's `activity`, the room stream's `turns`) the driver
         *  replaces its state with it; without one it reads a snapshot. */
        reset(snapshot?: Snapshot): void;
        /** The socket dropped and is reconnecting. */
        drop(): void;
    }): () => void;
    /** Read every key's current turn (null: none). Reject with an error
     *  carrying `retryAfterMs` on a 429. */
    read(): Promise<Snapshot>;
}
export interface WatchOptions {
    /** Keep an end frame this long before the turn reads as none (0). */
    endLingerMs?: number;
    /** Re-read while a turn shows and no frame came this long (60 s; never under 60 s). */
    selfCheckMs?: number;
    /** At most one read per this many ms (1000). */
    minReadGapMs?: number;
    now?: () => number;
    setTimeout?: (fn: () => void, ms: number) => unknown;
    clearTimeout?: (t: unknown) => void;
    /** Called with a function to call when the page becomes visible
     *  again; returns the unsubscribe. Defaults to `document`'s
     *  visibilitychange where there is a document. */
    onVisible?: (fn: () => void) => () => void;
    /** A read failed (not a 429). */
    onError?: (err: unknown) => void;
}
export declare const SELF_CHECK_MS = 60000;
/**
 * Keep every key's turn live and right. `cb` gets a fresh Map on every
 * change (key → ActivityState). Returns the unsubscribe: it closes the
 * socket, clears every timer and calls nothing after it returns (W8).
 */
export declare function watchActivity(transport: ActivityTransport, cb: (states: Map<string, ActivityState>) => void, opts?: WatchOptions): () => void;
