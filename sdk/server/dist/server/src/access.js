// `k2.asGuest(pass).me.onAccessChanged(cb, opts)` (0.46.8, live-access-v1): what
// this person may do, kept right, with no sign-out and no refresh.
//
// The event says "read again" and nothing else; the answer is always me.get. So
// this holds a `me.events` socket and reads me.get:
//
// - after EVERY open, the first included: a change while the socket was down has
//   no replay, and the read on reopen is how it's recovered;
// - after every frame. A frame during a read means exactly one more read;
// - a 429 waits its `retryAfterSecs`, then reads again. Any other read failure goes
//   to `onError` and it keeps listening.
//
// `ended: true` means the pass is dead: the person has to sign in again. A dropped
// socket asks me.get before it retries (socket.ts), so a pass revoked while offline
// ends here too, and a rotated App pass just reconnects.
import { K2Error } from "./errors.js";
function passIsDead(e) {
    return e.status === 401 || (e.status === 403 && (e.code === 'pass_invalid' || e.code === 'pass_wrong_link'));
}
export function watchAccess(a, cb, opts) {
    let stopped = false;
    let reading = false;
    let owed = false;
    let timer = null;
    let off = null;
    const say = (change) => {
        try {
            cb(change);
        }
        catch (err) {
            // A throwing callback must not kill the watch; surface it.
            queueMicrotask(() => {
                throw err;
            });
        }
    };
    const fail = (e) => {
        if (opts?.onError)
            opts.onError(e);
        else
            console.warn(`[k2] ${e.message}`);
    };
    const stop = () => {
        stopped = true;
        if (timer)
            clearTimeout(timer);
        timer = null;
        off?.();
    };
    const ended = () => {
        if (stopped)
            return;
        stop();
        say({ me: null, ended: true });
    };
    const read = () => {
        if (stopped)
            return;
        if (reading || timer) {
            owed = true;
            return;
        }
        reading = true;
        owed = false;
        void a.call('me.get').then((v) => {
            reading = false;
            if (stopped)
                return;
            say({ me: v, ended: false });
            if (owed)
                read();
        }, (e) => {
            reading = false;
            if (stopped)
                return;
            const err = e instanceof K2Error ? e : new K2Error({ code: 'failed', hint: String(e), verb: 'me.get' });
            if (passIsDead(err)) {
                ended();
                return;
            }
            if (err.code === 'rate_limited') {
                timer = setTimeout(() => {
                    timer = null;
                    read();
                }, (err.retryAfterSecs ?? 1) * 1000);
                return;
            }
            fail(err);
            if (owed)
                read();
        });
    };
    off = a.subscribe('me.events', { scope: 'me' }, () => read(), (e) => {
        // The socket says what it knows; every open reads again anyway.
        if (e.code === 'resync' || stopped)
            return;
        if (passIsDead(e)) {
            ended();
            return;
        }
        // It won't retry from here (say, a server with no me.events, or out of
        // sockets). Say why, and leave the caller with what's true now.
        fail(e);
        read();
    }, { onOpen: () => read() });
    if (stopped)
        off();
    return stop;
}
/** `scope=guests`: one socket for all of this App's guests. */
export function watchGuestAccess(a, cb, opts) {
    return a.subscribe('me.events', { scope: 'guests' }, (f) => {
        const principalId = f.principalId;
        if (typeof principalId === 'string')
            cb({ principalId });
    }, (e) => {
        if (e.code === 'resync')
            return;
        if (opts?.onError)
            opts.onError(e);
        else
            console.warn(`[k2] ${e.message}`);
    }, opts?.onOpen ? { onOpen: opts.onOpen } : undefined);
}
