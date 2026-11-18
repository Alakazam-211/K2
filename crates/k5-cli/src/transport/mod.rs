//! How the command line talks to the daemon (PRD 6.4).
//!
//! One small HTTP/1.1 client over loopback TCP and the cell socket that
//! sends what the shell CLI sends for the same call: its request
//! functions in `cli/src/cli-tools-policy.sh` (`cli_request`,
//! `cli_request_post`, `cli_post_form`, `cli_post_json`,
//! `cli_post_json_stdin`) and `cli/src/cell-socket.sh` (their `_uds`
//! twins, `cli_send`, `cli_send_json`), all through `_k2_curl`. The
//! binary's own verbs call it (`k5 cli status`, `k5 --version --check`).
//!
//! - [`wire`]: one exchange, written as curl writes it.
//! - [`client`]: the calls, their tries and time limits.
//! - [`skew`]: the one line when the daemon's version is not the binary's.
//! - [`permission`]: the refusal's closing line.
//! - [`encode`]: the query escaping (Python's `quote`).
//!
//! # The requests
//!
//! Every request has curl's head (see [`wire`]): the request line,
//! `Host`, `User-Agent`, `Accept: */*`, then these custom headers in this
//! order, then `Content-Length` for a body:
//!
//! 1. `Authorization: Bearer <passport>`, on the cell socket only (a
//!    config line read before the command line's `-H`s).
//! 2. `X-K5-Would-Refuse: 1`, when the passport rule would refuse the
//!    call, and never otherwise.
//! 3. `X-K5-Cli-Via`: `fallthrough` from the shell when the front ran it,
//!    `native` here. The shell's two are put in front of the command
//!    line's `-H`s, the Via first, so the would-refuse line ends up above
//!    it.
//! 4. `X-K5-Owner-Source: disk`, on a call made as the owner with the
//!    token read from the K2 home's files (the shell's last hand-over
//!    header). Never with a passport.
//! 5. A custom `Content-Type`, when the call has one.
//!
//! `Host` is `<HOST>:<PORT>` over TCP and `localhost` on the cell socket
//! (its URL is `http://localhost<path>`). `<P>` is `PROJECT` escaped by
//! [`encode::quote`]; `<T>` the TCP token (the owner's, or the passport);
//! `args` the caller's pre-escaped `key=value` pairs, joined by `&`.
//! "(-G)" marks a query the shell hands curl's `-G`: its `%XX` escapes go
//! out in upper case here ([`wire::curl_get_query`]), where curl 8.5 and
//! 8.7 write them in lower case and curl 8.23 as given:
//!
//! | call | TCP | cell socket |
//! |---|---|---|
//! | `cli_request` | `GET <ep>?token=<T>&project=<P>&args` (-G) | `GET <ep>?args` (-G; no `?` with no args) |
//! | `cli_request_post` | `POST <ep>?token=<T>&project=<P>&args`, empty body, curl's form type | `POST <ep>?args`, likewise |
//! | `cli_post_form` | `POST <ep>?token=<T>&project=<P>`, body `args`, `Content-Type: application/x-www-form-urlencoded` | `POST <ep>`, likewise |
//! | `cli_post_json` | `POST <ep>?token=<T>` (no `project=`), `Content-Type: application/json` | `POST <ep>`, likewise |
//! | `cli_post_json_stdin` | as `cli_post_json` | none |
//! | `cli_send_json GET` | `GET <ep>?token=<curl-escaped T>&payload` (-G) | `GET <ep>?payload` (-G) |
//! | `cli_send_json POST` | `POST <ep>?token=<quoted T>`, JSON | `POST <ep>`, JSON |
//! | (none: [`client::Client::get_public`]) | `GET <ep>`: no token, no `project=`, no Owner-Source | none |
//! | `k2 respond`'s own curl ([`client::Client::post_form_bearer`]) | `POST <ep>`, the passport as Bearer (first), body `args`, the form type; no query | none |
//! | the embedded Python's `urlopen` in `k2 tickets show` ([`client::Client::urlopen_get`]) | `GET <ep>?<query>&token=<T>`, both as `urllib.parse.urlencode` writes them, with urllib's head ([`wire::Head::Urllib`]); one try, no heal, no line, no note | none |
//!
//! An endpoint that already has a `?` gets the `-G` query, and
//! `cli_request_post`'s, after `&`; the other POST forms always add `?`,
//! as the shell writes them. The shell gives curl a `-G` query from its
//! caller as `data-raw`, so one that starts with `@` is sent as written
//! (never read as a file), as here.
//!
//! The per-verb headers are not the shared functions': `X-K2-Tty`
//! (`k2 vault`, `k2 migrate`, `k2 clone`), `X-K2-Pane`, `X-K2-Agent-Pid`
//! and `X-K2-Hook-Source`, `-Version`, `-Report`, `-Event` (`k2 hooks
//! emit`) are `-H`s their callers give `_k2_curl` themselves, after the
//! shell's two; `X-K2-Client-IP` and `X-K2-Mode` are the daemon's (set on
//! what it forwards, or on its answers), and `X-K2-Ops-*` names are
//! `k2 vault --sign` settings for other servers. [`wire::Request`] takes
//! any ordered header list, so those calls fit in later.
//!
//! # Tries
//!
//! - Nothing accepted the connection (curl exit 7): again, at most
//!   [`client::TRIES`] tries, a second apart, every call but a body from
//!   stdin (the shell's test: a curl argument that is exactly `@-`; an
//!   `@-` inside a URL keeps the tries); a loopback one then tries the
//!   port in `daemon.port` when it differs ([`client::Client::port_file`]).
//! - The connect timed out (curl 28 before "Connected to"): again, the
//!   same way, for `cli_request` and `cli_request_post` and their twins.
//! - `503` with `"state":"migrating"`: again, at most [`client::TRIES`]
//!   tries, a second apart, the same four. The status decides, never the
//!   body alone.
//! - `cli_request_post` over TCP, an answer holding `method not allowed
//!   for this route` (a pre-0.44.4 daemon), whatever its status: the same
//!   call once as `cli_request`, a GET.
//! - An answer holding `Invalid or missing auth token`: once per client,
//!   the same call again with a healed passport ([`client::Healer`]).
//!   Every call on either channel but `cli_post_json_stdin`.
//! - `cli_send` and `cli_send_json`, from the cell socket to TCP: when
//!   nothing of the request went out on the socket (the connect failed or
//!   timed out, or the first write took no byte), or the daemon answered
//!   `not served on cell socket` (no handler ran). A GET also goes to TCP
//!   after any other failure, and `cli_send`'s after an empty body (or one
//!   of only newlines and NULs, which `$(…)` leaves empty), as the shell's
//!   do. A POST that went out never does: see below.
//!
//! An answer is read to its end (its `Content-Length`, its last chunk, or
//! the end of the stream when it has neither) and no further, so a close
//! or a reset after a whole answer leaves the answer standing.
//!
//! # Time limits
//!
//! Connect 5 s; at most 30 s per try, or `_K2_POST_MAX_TIME` for
//! `cli_post_json` and `cli_post_form` ([`client::Client::post_max_time`]),
//! and 60 s or `_K2_SEND_MAX_TIME` for `cli_send_json`
//! ([`client::Client::send_max_time`]). `cli_post_json_stdin` is 30 s.
//!
//! A request the daemon never answered (a timeout, a connection closed
//! with nothing, or a 5xx with an empty body for the two that read the
//! status) is [`client::Error::NoAnswer`]: the shell's line, `Error: K2's
//! server didn't answer <ep> within <n> s; run k2 daemon status` (or
//! `gave an empty answer to <ep> after <n> s`), instead of a JSON parse
//! error. `cli_post_json_stdin` and `cli_send_json` have no such line.
//!
//! # Exit codes
//!
//! A call whose last step is curl ends the command with curl's code (the
//! shell runs under `set -e`), so each failure carries it
//! ([`client::Error::curl_code`]): 7 nothing accepted the connection, 28
//! no answer in time, 52 an empty reply, 55 and 56 a write or a read that
//! failed, 18 an answer cut short, 1 an answer that is not HTTP, 3 a URL
//! curl won't send. `cli_request` and `cli_request_post` (and their
//! twins) run curl through `_cli_curl`, whose local check turns a failure
//! that is this machine's into 75 with no no-answer line
//! ([`client::locally_denied`]: the OS refused the connect, or `TMPDIR`
//! can't be used, but never a refused connect).
//!
//! # The daemon's version
//!
//! Every answer's `X-K5-Server-Version` ([`wire::SERVER_VERSION_HEADER`])
//! is compared with the binary's version as it comes in, a retried one
//! too. The first that differs prints [`skew::line`] on stderr, once per
//! process ([`skew::note`]); the call goes on. No header, no line. The
//! shell CLI doesn't compare yet.
//!
//! # Where this differs from the shell, on purpose
//!
//! - `User-Agent: k5/<version>` for curl's, and `X-K5-Cli-Version:
//!   <version>` (just before the Via), and `X-K5-Cli-Via: native`.
//! - PRD 13.9 #4: with a passport, the owner token is never sent on any
//!   channel. The shell's open tools put the owner token from disk back
//!   on TCP inside a K2 terminal (`_cli_apply_auth_for_verb`, COMPAT-58),
//!   and `cli_send`'s TCP arm then uses it; here [`client::Auth`] holds
//!   either a passport or the owner token, never both.
//! - No empty Bearer: `cli_send` tries the cell socket whenever
//!   `K2_HOOK_SOCK` is a socket, with `Bearer ` and nothing after it when
//!   there is no passport. Here only a passport has a cell socket.
//! - A healed passport stays healed for the client. The shell's `_uds`
//!   arms heal inside `$(…)`, so `cli_send`'s TCP try after one starts
//!   from the old passport again.
//! - A target curl would not send as given (a space or a control byte:
//!   curl exit 3; a `#`: curl cuts it off) is an error here, never sent.
//! - A failed `cli_send_json` is an error here; the shell prints status
//!   `000`.
//! - A POST that went out on the cell socket is never sent again over
//!   TCP. When no usable answer came back (none in time, a close or a
//!   reset before the answer ended, a 5xx with an empty body) it is
//!   [`client::Error::MaybeApplied`]: `Error: K2's server didn't answer
//!   <ep> (<n> s); the request may have been applied. Check before running
//!   it again.` (after the shell's line for a send it couldn't confirm,
//!   `_k2_idem_send`). An answer with an empty body is the answer. The
//!   shell's `cli_send` and `cli_send_json` send such a POST again over
//!   TCP (curl exit 28, 52 or 56 after the request went out, or an empty
//!   answer).

pub mod client;
pub mod encode;
pub mod permission;
pub mod skew;
pub mod wire;

pub use client::{
    Auth, Call, Channel, Client, Error, Healer, NoAnswer, Passport, Response, SecretsDir,
};
