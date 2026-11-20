#!/usr/bin/env bash
# release-timer.sh — how long each step of release.sh took, sourced by
# scripts/release.sh near the top. bash 3.2-safe. Sets no shell options:
# every call saves the caller's (-e, -u, pipefail), runs with them off, puts
# them back and returns 0. A timer problem (a file it can't write, a clock it
# can't read, a step it doesn't know) prints one "[release-timer] warning"
# line and the release goes on: the timer never changes what a step does or
# how the release ends.
#
# What it records, per run of release.sh:
#   - every numbered step (start, 0.3 … 10.7), as release.sh names it;
#   - sub-steps where release.sh has them: each notarization and staple,
#     each wait on K2 Compute jobs, the Windows build, each Linux signing
#     script, the push + tag, the smoke tests, and the wait for the signing
#     approval;
#   - kind: "machine" (this Mac, Apple, GitHub or the compute nodes are
#     working) or "waiting" (waiting for a person: the signing approval).
#     A waiting sub-step's time counts as waiting, not as machine time.
#
# Where (one file per version; every run of that version appends to it, so
# a K2_RELEASE_RESUME run adds its rows after its first run's):
#   real run   ${K2_RELEASE_AUDIT_DIR:-~/k2-release-audit}/<version>/timings.jsonl
#              (beside the compute audit and signatures.tsv)
#   dry run    <dry root>/timings/<version>/timings.jsonl, the dry root being
#              K2_RELEASE_DRY_ROOT (default <main checkout>/.claude/worktrees/
#              release-dry; scripts/lib/release-dry.sh). Rehearsals never mix
#              with real runs.
# A dry run's switch to its scratch tree (before the exec) writes nothing
# itself; the run in the scratch tree starts its "start" step at the time
# the switch began (K2_RELEASE_TIMER_RUN / K2_RELEASE_TIMER_T0, honoured
# only there).
#
# Format: JSON lines, four record types, each with the run's id:
#   {"type":"run","run":"20261120T101500Z-4242","version":"0.46.17",
#    "mode":"real","dry":false,"resume_from":null,"host":"…","git_sha":"…",
#    "started":"2026-11-20T10:15:00Z","started_epoch":…,"format":1}
#   {"type":"begin", run, id, parent, label, kind, start, start_epoch, resumed}
#   {"type":"step","run":"…","id":"4:app-notarize","parent":"4","depth":1,
#    "label":"Apple notarization of the app (submit + wait)","kind":"machine",
#    "status":"ok","start":"…","start_epoch":…,"end":"…","end_epoch":…,
#    "seconds":312,"resumed":false}
#   {"type":"end","run":"…","rc":0,"status":"ok","ended":"…","ended_epoch":…,
#    "seconds":…,"git_sha":"…","timer_errors":0}
# mode: real, dry-run, resume-from-<step> or dry-run-resume-from-<step>;
# "resumed" is true on every row of a resumed run.
# status: ok, fail, skipped (not run in this run: a resumed run's earlier
# steps, or a step it leaves out), interrupted (INT/TERM), unended (a
# sub-step release.sh didn't close: a timer wiring bug), or what release.sh
# passes (timeout). A "begin" with no "step" in a run with no "end" was
# still running when the process was killed. Labels only: no command lines,
# paths, environment or secrets are written.
#
# scripts/release-timings.sh <version> | --last prints a file as a table
# with totals (machine time vs waiting time).
#
# API (release.sh):
#   _rt_init <version> <project dir>  once, as soon as the version is known
#   _rt_step <id> "<label>" [kind]    a numbered step starts; the open one ends (ok)
#   _rt_start <id> "<label>" [kind]   a sub-step starts inside what is open
#   _rt_end <id> [status|rc]          it ends (0 or "" = ok, another number = fail)
#   _rt_mark <status>                 the open numbered step ends with <status>
#   _rt_skip <id>...                  numbered steps this run doesn't run (skipped)
#   _rt_finish <rc>                   from the EXIT traps: close what is open,
#                                     write the end record, print the table
# Before _rt_init (or when it couldn't start) every call does nothing.

_RT_ON=0
_RT_FILE=""
_RT_RUN=""
_RT_VERSION=""
_RT_MODE=""
_RT_DRY=false
_RT_RESUME=""
_RT_RESUMED=false
_RT_PROJ=""
_RT_T0=""
_RT_T0_ISO=""
_RT_FIRST_T0=""
_RT_FIRST_ISO=""
_RT_ERRS=0
_RT_WARNED=""
_RT_FINISHED=""
_RT_PRIOR_RUNS=0
# The open steps, innermost last.
_RT_N=0
_RT_S_ID=()
_RT_S_LABEL=()
_RT_S_KIND=()
_RT_S_T0=()
_RT_S_ISO=()
_RT_S_MARK=()
_RT_S_SEQ=()
_RT_S_INWAIT=()
# Finished rows for the table, by start order: depth TAB id TAB kind TAB
# status TAB seconds TAB counted-as-waiting (0/1) TAB label.
_RT_SEQ=0
_RT_ROW=()

# "<epoch> <iso>" (a function so tests can drive a fake clock).
_rt_clock() { date -u '+%s %Y-%m-%dT%H:%M:%SZ'; }

# Every number the timer does arithmetic with is checked first: in bash an
# arithmetic error aborts the whole top-level command it happens in (the
# rest of an if-block of release.sh, say), and that must never happen here.
_rt_isnum() {
    case "${1:-}" in ""|*[!0-9]*) return 1 ;; esac
    [ "${#1}" -le 12 ]
}

# Sets _RT_NOW_T (epoch) and _RT_NOW_ISO; 1 when the clock can't be read.
_RT_NOW_T=""
_RT_NOW_ISO=""
_rt_split_now() {
    _RT_NOW_T="${1%% *}"
    _RT_NOW_ISO="${1#* }"
    _rt_isnum "$_RT_NOW_T" || return 1
    case "$_RT_NOW_ISO" in ""|*[!0-9A-Z:.+-]*) return 1 ;; esac
    return 0
}
_rt_now() {
    local now
    now="$(_rt_clock 2>/dev/null)" || return 1
    _rt_split_now "$now"
}

_rt_warn() { # <key> <text>: once per key
    _RT_ERRS=$((_RT_ERRS + 1))
    case " $_RT_WARNED " in *" $1 "*) return 0 ;; esac
    _RT_WARNED="$_RT_WARNED $1"
    printf '[release-timer] warning: %s (timings only; the release goes on)\n' "$2" >&2
    return 0
}

# A JSON string (labels and ids only; never anything secret).
_rt_js() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/ }"
    s="${s//$'\r'/ }"
    s="${s//$'\t'/ }"
    printf '"%s"' "$s"
}
_rt_js_or_null() { if [ -n "$1" ]; then _rt_js "$1"; else printf null; fi; }

_rt_write() {
    [ -n "$_RT_FILE" ] || return 0
    if ! printf '%s\n' "$1" >> "$_RT_FILE" 2>/dev/null; then
        _rt_warn write "can't write $_RT_FILE"
    fi
    return 0
}

# 75 → "1m15s", 3900 → "1h05m00s"
_rt_dur() {
    local s="${1:-0}"
    _rt_isnum "$s" || s=0
    if [ "$s" -ge 3600 ]; then
        printf '%dh%02dm%02ds' $((s / 3600)) $(((s % 3600) / 60)) $((s % 60))
    else
        printf '%dm%02ds' $((s / 60)) $((s % 60))
    fi
}

# Run an implementation with the caller's shell options off; always 0.
_rt_guarded() {
    local _rt_o="$-" _rt_pf=0
    shopt -qo pipefail && _rt_pf=1
    set +e +u
    set +o pipefail
    "$@"
    case "$_rt_o" in *e*) set -e ;; esac
    case "$_rt_o" in *u*) set -u ;; esac
    if [ "$_rt_pf" = 1 ]; then set -o pipefail; fi
    return 0
}

_rt_init()   { _rt_guarded _rt__init "$@"; }
_rt_step()   { _rt_guarded _rt__step "$@"; }
_rt_start()  { _rt_guarded _rt__start "$@"; }
_rt_end()    { _rt_guarded _rt__end "$@"; }
_rt_mark()   { _rt_guarded _rt__mark "$@"; }
_rt_skip()   { _rt_guarded _rt__skip "$@"; }
_rt_finish() { _rt_guarded _rt__finish "$@"; }

_rt__init() {
    local version="$1" proj="$2" now dir root sha host
    [ "$_RT_ON" = 0 ] || return 0
    case "$version" in
        ""|*[!0-9A-Za-z._-]*|.*) _rt_warn version "no step timings: the version '$version' can't name a folder"; return 0 ;;
    esac
    _RT_VERSION="$version"
    _RT_PROJ="$proj"
    case "${K2_RELEASE_RESUME:-}" in
        "") _RT_RESUME="" ;;
        *[!0-9A-Za-z.]*) _RT_RESUME="other" ;;
        *) _RT_RESUME="$K2_RELEASE_RESUME" ;;
    esac
    [ -n "$_RT_RESUME" ] && _RT_RESUMED=true
    if [ "${K2_RELEASE_DRY:-0}" = 1 ]; then
        _RT_DRY=true
        _RT_MODE="dry-run${_RT_RESUME:+-resume-from-$_RT_RESUME}"
    else
        _RT_MODE="${_RT_RESUME:+resume-from-$_RT_RESUME}"
        _RT_MODE="${_RT_MODE:-real}"
    fi
    _rt_now || { _rt_warn clock "no step timings: the clock can't be read"; return 0; }
    now="$_RT_NOW_T $_RT_NOW_ISO"
    # The dry-run switch: nothing is written here; the run in the scratch
    # tree (this process, after the exec) carries on with this run id and start.
    if [ "$_RT_DRY" = true ] && [ "${K2_RELEASE_DRY_INNER:-0}" != 1 ]; then
        K2_RELEASE_TIMER_RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
        K2_RELEASE_TIMER_T0="$now"
        export K2_RELEASE_TIMER_RUN K2_RELEASE_TIMER_T0
        return 0
    fi
    _RT_RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    if [ "$_RT_DRY" = true ] && [ -n "${K2_RELEASE_TIMER_RUN:-}" ] && _rt_split_now "${K2_RELEASE_TIMER_T0:-}"; then
        case "$K2_RELEASE_TIMER_RUN" in
            *[!0-9A-Za-z-]*) ;;
            *) _RT_RUN="$K2_RELEASE_TIMER_RUN"; now="$_RT_NOW_T $_RT_NOW_ISO" ;;
        esac
    fi
    _RT_T0="${now%% *}"
    _RT_T0_ISO="${now#* }"
    _RT_FIRST_T0="$_RT_T0"
    _RT_FIRST_ISO="$_RT_T0_ISO"
    if [ "$_RT_DRY" = true ]; then
        [ -n "${K2_RELEASE_DRY_RUN_DIR:-}" ] || { _rt_warn where "no step timings: no dry-run dir"; return 0; }
        root="$(dirname "$(dirname "$K2_RELEASE_DRY_RUN_DIR")")"
        dir="$root/timings/$version"
    else
        dir="${K2_RELEASE_AUDIT_DIR:-$HOME/k2-release-audit}/$version"
    fi
    _RT_FILE="$dir/timings.jsonl"
    if ! mkdir -p "$dir" 2>/dev/null; then
        _rt_warn write "can't make $dir; the step times are only printed"
        _RT_FILE=""
    elif [ -s "$_RT_FILE" ]; then
        _RT_PRIOR_RUNS="$(grep -c '"type":"run"' "$_RT_FILE" 2>/dev/null)"
        _rt_isnum "$_RT_PRIOR_RUNS" || _RT_PRIOR_RUNS=0
    fi
    sha="$(command git -C "$proj" rev-parse HEAD 2>/dev/null)"
    host="$(hostname -s 2>/dev/null || uname -n 2>/dev/null)"
    _RT_ON=1
    _rt_write "{\"type\":\"run\",\"run\":$(_rt_js "$_RT_RUN"),\"version\":$(_rt_js "$version"),\"mode\":$(_rt_js "$_RT_MODE"),\"dry\":$_RT_DRY,\"resume_from\":$(_rt_js_or_null "$_RT_RESUME"),\"host\":$(_rt_js_or_null "$host"),\"git_sha\":$(_rt_js_or_null "$sha"),\"started\":$(_rt_js "$_RT_T0_ISO"),\"started_epoch\":$_RT_T0,\"format\":1}"
}

_rt_clean_id() {
    case "${1:-}" in ""|*[!0-9A-Za-z._:-]*) return 1 ;; esac
    return 0
}

_rt_status_of() { # <status|rc> [default] → a status word
    local s="${1:-}"
    case "$s" in
        "") printf '%s' "${2:-ok}" ;;
        0) printf ok ;;
        *[!0-9]*) case "$s" in *[!0-9A-Za-z_-]*) printf other ;; *) printf '%s' "$s" ;; esac ;;
        *) printf fail ;;
    esac
}

# Push a step: <id> <label> <kind> [<epoch> <iso>]
_rt_push() {
    local id="$1" label="$2" kind="${3:-machine}" t iso parent="" i inwait=0
    label="${label//$'\t'/ }"
    label="${label//$'\n'/ }"
    case "$kind" in
        machine|waiting) ;;
        *) _rt_warn "kind-$id" "unknown kind '$kind' for $id (machine or waiting); counted as machine"; kind=machine ;;
    esac
    if _rt_isnum "${4:-}"; then
        t="$4"; iso="$5"
    else
        _rt_now || { _rt_warn clock "the clock can't be read (at $id); $id is not timed"; return 1; }
        t="$_RT_NOW_T"; iso="$_RT_NOW_ISO"
    fi
    if [ "$_RT_N" -gt 0 ]; then
        parent="${_RT_S_ID[$((_RT_N - 1))]}"
        i=0
        while [ "$i" -lt "$_RT_N" ]; do
            [ "${_RT_S_KIND[$i]}" = waiting ] && inwait=1
            i=$((i + 1))
        done
    fi
    _RT_SEQ=$((_RT_SEQ + 1))
    _RT_S_ID[$_RT_N]="$id"
    _RT_S_LABEL[$_RT_N]="$label"
    _RT_S_KIND[$_RT_N]="$kind"
    _RT_S_T0[$_RT_N]="$t"
    _RT_S_ISO[$_RT_N]="$iso"
    _RT_S_MARK[$_RT_N]=""
    _RT_S_SEQ[$_RT_N]="$_RT_SEQ"
    _RT_S_INWAIT[$_RT_N]="$inwait"
    _RT_N=$((_RT_N + 1))
    _rt_write "{\"type\":\"begin\",\"run\":$(_rt_js "$_RT_RUN"),\"id\":$(_rt_js "$id"),\"parent\":$(_rt_js_or_null "$parent"),\"label\":$(_rt_js "$label"),\"kind\":\"$kind\",\"start\":$(_rt_js "$iso"),\"start_epoch\":$t,\"resumed\":$_RT_RESUMED}"
}

# Pop the innermost open step with <status> (a mark from _rt_mark wins over ok).
_rt_pop() {
    local status="$1" k t iso secs=0 id parent="" counted=0
    [ "$_RT_N" -gt 0 ] || return 0
    k=$((_RT_N - 1))
    id="${_RT_S_ID[$k]}"
    if [ "$status" = ok ] && [ -n "${_RT_S_MARK[$k]}" ]; then status="${_RT_S_MARK[$k]}"; fi
    if _rt_now; then
        t="$_RT_NOW_T"; iso="$_RT_NOW_ISO"
    else
        t="${_RT_S_T0[$k]}"; iso="${_RT_S_ISO[$k]}"
        _rt_warn clock "the clock can't be read (at the end of $id); its time is 0"
    fi
    if [ "$t" -ge "${_RT_S_T0[$k]}" ]; then secs=$((t - ${_RT_S_T0[$k]})); fi
    [ "$k" -gt 0 ] && parent="${_RT_S_ID[$((k - 1))]}"
    if [ "${_RT_S_KIND[$k]}" = waiting ] && [ "${_RT_S_INWAIT[$k]}" = 0 ]; then counted=1; fi
    _RT_ROW[${_RT_S_SEQ[$k]}]="$k	$id	${_RT_S_KIND[$k]}	$status	$secs	$counted	${_RT_S_LABEL[$k]}"
    _rt_write "{\"type\":\"step\",\"run\":$(_rt_js "$_RT_RUN"),\"id\":$(_rt_js "$id"),\"parent\":$(_rt_js_or_null "$parent"),\"depth\":$k,\"label\":$(_rt_js "${_RT_S_LABEL[$k]}"),\"kind\":\"${_RT_S_KIND[$k]}\",\"status\":$(_rt_js "$status"),\"start\":$(_rt_js "${_RT_S_ISO[$k]}"),\"start_epoch\":${_RT_S_T0[$k]},\"end\":$(_rt_js "$iso"),\"end_epoch\":$t,\"seconds\":$secs,\"resumed\":$_RT_RESUMED}"
    unset "_RT_S_ID[$k]" "_RT_S_LABEL[$k]" "_RT_S_KIND[$k]" "_RT_S_T0[$k]" "_RT_S_ISO[$k]" "_RT_S_MARK[$k]" "_RT_S_SEQ[$k]" "_RT_S_INWAIT[$k]"
    _RT_N=$k
}

_rt__step() {
    local id="$1" label="${2:-}" kind="${3:-machine}"
    [ "$_RT_ON" = 1 ] || return 0
    _rt_clean_id "$id" || { _rt_warn "id-$id" "bad step id '$id'"; return 0; }
    # Sub-steps still open here were never ended: a wiring bug, said so.
    while [ "$_RT_N" -gt 1 ]; do _rt_pop unended; done
    [ "$_RT_N" -eq 1 ] && _rt_pop ok
    if [ -n "$_RT_FIRST_T0" ]; then
        _rt_push "$id" "$label" "$kind" "$_RT_FIRST_T0" "$_RT_FIRST_ISO"
        _RT_FIRST_T0=""
    else
        _rt_push "$id" "$label" "$kind"
    fi
}

_rt__start() {
    local id="$1" label="${2:-}" kind="${3:-machine}"
    [ "$_RT_ON" = 1 ] || return 0
    _rt_clean_id "$id" || { _rt_warn "id-$id" "bad sub-step id '$id'"; return 0; }
    [ "$_RT_N" -gt 0 ] || _rt_warn "orphan-$id" "sub-step $id started outside a numbered step; recorded at the top level"
    _rt_push "$id" "$label" "$kind"
}

_rt__end() {
    local id="$1" status i found=-1
    [ "$_RT_ON" = 1 ] || return 0
    status="$(_rt_status_of "${2:-}")"
    i=$((_RT_N - 1))
    while [ "$i" -ge 0 ]; do
        if [ "${_RT_S_ID[$i]}" = "$id" ]; then found=$i; break; fi
        i=$((i - 1))
    done
    if [ "$found" -lt 0 ]; then
        _rt_warn "end-$id" "_rt_end $id: no such step is open"
        return 0
    fi
    while [ "$_RT_N" -gt $((found + 1)) ]; do _rt_pop unended; done
    _rt_pop "$status"
}

_rt__mark() {
    [ "$_RT_ON" = 1 ] || return 0
    [ "$_RT_N" -gt 0 ] || return 0
    _RT_S_MARK[$((_RT_N - 1))]="$(_rt_status_of "${1:-}")"
}

_rt__skip() {
    local id t iso why="not run in this run"
    [ "$_RT_ON" = 1 ] || return 0
    [ "$_RT_RESUMED" = true ] && why="not run: a resumed run (K2_RELEASE_RESUME=$_RT_RESUME); the first run did it"
    for id in "$@"; do
        _rt_clean_id "$id" || { _rt_warn "id-$id" "bad step id '$id'"; continue; }
        _rt_now || { _rt_warn clock "the clock can't be read (at $id)"; continue; }
        t="$_RT_NOW_T"; iso="$_RT_NOW_ISO"
        _RT_SEQ=$((_RT_SEQ + 1))
        _RT_ROW[$_RT_SEQ]="0	$id	machine	skipped	0	0	$why"
        _rt_write "{\"type\":\"step\",\"run\":$(_rt_js "$_RT_RUN"),\"id\":$(_rt_js "$id"),\"parent\":null,\"depth\":0,\"label\":$(_rt_js "$why"),\"kind\":\"machine\",\"status\":\"skipped\",\"start\":$(_rt_js "$iso"),\"start_epoch\":$t,\"end\":$(_rt_js "$iso"),\"end_epoch\":$t,\"seconds\":0,\"resumed\":$_RT_RESUMED}"
    done
}

_rt__finish() {
    local rc="${1:-0}" status t iso sha secs=0
    [ "$_RT_ON" = 1 ] || return 0
    [ -z "$_RT_FINISHED" ] || return 0
    _RT_FINISHED=1
    _rt_isnum "$rc" || rc=1
    case "$rc" in
        0) status=ok ;;
        130|143) status=interrupted ;;
        *) status=fail ;;
    esac
    while [ "$_RT_N" -gt 0 ]; do _rt_pop "$status"; done
    if _rt_now; then t="$_RT_NOW_T"; iso="$_RT_NOW_ISO"; else t="$_RT_T0"; iso="$_RT_T0_ISO"; fi
    if [ "$t" -ge "$_RT_T0" ]; then secs=$((t - _RT_T0)); fi
    sha="$(command git -C "$_RT_PROJ" rev-parse HEAD 2>/dev/null)"
    _rt_write "{\"type\":\"end\",\"run\":$(_rt_js "$_RT_RUN"),\"rc\":$rc,\"status\":\"$status\",\"ended\":$(_rt_js "$iso"),\"ended_epoch\":$t,\"seconds\":$secs,\"git_sha\":$(_rt_js_or_null "$sha"),\"timer_errors\":$_RT_ERRS}"
    # In a subshell: a closed stdout can't take the release's exit code with it.
    ( _rt_table "$rc" "$secs" ) || true
}

_rt_table() {
    local rc="$1" wall="$2" i row depth id kind status secs counted label pad total=0 waiting=0
    echo ""
    echo "Step timings: v${_RT_VERSION}, ${_RT_MODE}, run ${_RT_RUN} (exit ${rc})"
    printf '  %-26s %-8s %-11s %9s  %s\n' step kind status time what
    i=1
    while [ "$i" -le "$_RT_SEQ" ]; do
        row="${_RT_ROW[$i]:-}"
        i=$((i + 1))
        [ -n "$row" ] || continue
        IFS='	' read -r depth id kind status secs counted label <<< "$row"
        pad=""
        [ "$depth" -gt 0 ] && pad="$(printf '%*s' $((depth * 2)) '')"
        printf '  %-26s %-8s %-11s %9s  %s\n' "$pad$id" "$kind" "$status" "$(_rt_dur "$secs")" "$label"
        [ "$depth" = 0 ] && total=$((total + secs))
        [ "$counted" = 1 ] && waiting=$((waiting + secs))
    done
    printf '  machine %s, waiting for a person %s, steps %s, wall clock %s\n' \
        "$(_rt_dur $((total - waiting)))" "$(_rt_dur "$waiting")" "$(_rt_dur "$total")" "$(_rt_dur "$wall")"
    if [ -z "$_RT_FILE" ]; then
        echo "  timings: not written ($_RT_ERRS timer warning(s) above)"
    elif [ "$_RT_ERRS" = 0 ]; then
        echo "  timings: $_RT_FILE"
    else
        echo "  timings: $_RT_FILE ($_RT_ERRS timer warning(s) above)"
    fi
    if [ "$_RT_PRIOR_RUNS" -gt 0 ]; then
        echo "  the file has ${_RT_PRIOR_RUNS} earlier run(s) of v${_RT_VERSION}: scripts/release-timings.sh $([ "$_RT_DRY" = true ] && printf -- '--dry ')${_RT_VERSION} shows them all"
    fi
}
