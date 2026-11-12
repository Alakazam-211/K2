# ── Multi-heartbeat (agent_heartbeats) CRUD ────────────────────────────
# Shared error formatter for heartbeat daemon responses. Maps structured
# owner_only / forbidden teaching (exit 3) so agents never treat a valid
# passport as "invalid token" on owner-only surfaces.
_hb_print_err() {
    # stdin: raw JSON body (or empty). Exits 1/2/3.
    python3 -c '
import json, sys
raw = sys.stdin.read().strip()
if not raw:
    print("Error: empty response from daemon (is it running?)", file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: " + raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error", d) if isinstance(d, dict) else raw
if isinstance(err, dict):
    code = err.get("code") or "error"
    hint = err.get("hint") or err.get("message") or code
    if code in ("owner_only", "forbidden", "gated") and "ask your human" not in hint:
        hint = (hint + " — " if hint else "") + "requires owner/admin — ask your human"
    print("Error: " + hint, file=sys.stderr)
    sys.exit(3 if code in ("owner_only", "forbidden", "gated") else 2 if code == "usage" else 1)
print("Error: " + str(err), file=sys.stderr)
# Classic string "Invalid or missing auth token" → treat as auth failure
if isinstance(err, str) and "token" in err.lower():
    sys.exit(3)
sys.exit(1)
'
}

# Parse frequency/day/time flags into spec JSON that matches the scheduler
# logic in should_project_fire.
#   --daily --time 07:00
#   --weekly --days mon,tue --time 07:00
#   --monthly --days 1,15 --time 07:00
#   --yearly --months jan,apr --days 1 --time 07:00
#   --hourly --every 30 --unit minutes --start 09:00 --end 17:00
hb_parse_spec() {
    local frequency=""
    local time=""
    local days=""
    local months=""
    local every=""
    local unit="minutes"
    local start=""
    local end=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --daily)   frequency="daily"; shift ;;
            --weekly)  frequency="weekly"; shift ;;
            --monthly) frequency="monthly"; shift ;;
            --yearly)  frequency="yearly"; shift ;;
            --hourly)  frequency="hourly"; shift ;;
            --time)    time="$2"; shift 2 ;;
            --days)    days="$2"; shift 2 ;;
            --months)  months="$2"; shift 2 ;;
            --every)   every="$2"; shift 2 ;;
            --unit)    unit="$2"; shift 2 ;;
            --start)   start="$2"; shift 2 ;;
            --end)     end="$2"; shift 2 ;;
            --once|--in|--at|--on)
                echo "Error: $1 is for a one-time wake: k2 heartbeat remind \"<text>\" --in 45m (see k2 heartbeat remind --help). A heartbeat repeats." >&2
                exit 2 ;;
            *)
                # GH#23/#24: unknown flags used to be silently swallowed,
                # so `--interval 1` / `--frequency hourly` typos produced
                # a default schedule with no hint anything was ignored.
                echo "Error: unknown schedule flag/token: $1" >&2
                echo "Spec: --daily|--weekly|--monthly|--yearly|--hourly [--time HH:MM] [--days ...] [--months ...] [--every N --unit minutes|hours] [--start HH:MM --end HH:MM]" >&2
                exit 2 ;;
        esac
    done
    [ -z "$frequency" ] && { echo "Error: must specify one of --daily/--weekly/--monthly/--yearly/--hourly" >&2; exit 1; }
    case "$unit" in
        minutes|hours) ;;
        *) echo "Error: --unit must be 'minutes' or 'hours' (got '$unit')" >&2; exit 2 ;;
    esac
    if [ -n "$every" ] && ! [[ "$every" =~ ^[0-9]+$ ]]; then
        echo "Error: --every must be a whole number (got '$every')" >&2; exit 2
    fi
    # S5 HB34: the daemon refuses an hourly interval under 60 seconds;
    # say so here before the request instead of after.
    if [ -n "$every" ] && [ "$((10#$every))" -lt 1 ]; then
        echo "Error: --every must be at least 1 minute (got '$every')" >&2; exit 2
    fi
    local t
    for t in "$time" "$start" "$end"; do
        if [ -n "$t" ] && ! [[ "$t" =~ ^([01][0-9]|2[0-3]|[0-9]):[0-5][0-9]$ ]]; then
            echo "Error: times must be HH:MM 24-hour (got '$t')" >&2; exit 2
        fi
    done
    # GH#27: day/month VALUES were stored unvalidated — `--weekly --days
    # foobar` persisted "days":["foobar"] and could never fire. Frequency
    # is known here, so validate per-frequency before the spec is built
    # (monthly/yearly take day-of-month numbers; everything else takes
    # 3-letter day names, matching the JSON builder below).
    local tok
    if [ -n "$days" ]; then
        case "$frequency" in
            monthly|yearly)
                for tok in $(printf '%s' "$days" | tr ',' ' '); do
                    if ! [[ "$tok" =~ ^[0-9]+$ ]] || [ "$tok" -lt 1 ] || [ "$tok" -gt 31 ]; then
                        echo "Error: --days for --$frequency takes day-of-month numbers 1-31 (got '$tok')" >&2; exit 2
                    fi
                done
                ;;
            *)
                for tok in $(printf '%s' "$days" | tr ',' ' '); do
                    case "$(printf '%s' "$tok" | tr '[:upper:]' '[:lower:]')" in
                        mon|tue|wed|thu|fri|sat|sun) ;;
                        *) echo "Error: --days for --$frequency takes a comma list of mon,tue,wed,thu,fri,sat,sun (got '$tok')" >&2; exit 2 ;;
                    esac
                done
                ;;
        esac
    fi
    if [ -n "$months" ]; then
        for tok in $(printf '%s' "$months" | tr ',' ' '); do
            case "$(printf '%s' "$tok" | tr '[:upper:]' '[:lower:]')" in
                jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec) ;;
                *) echo "Error: --months takes a comma list of jan,feb,mar,apr,may,jun,jul,aug,sep,oct,nov,dec (got '$tok')" >&2; exit 2 ;;
            esac
        done
    fi
    # Build JSON spec
    python3 -c "
import json
spec = {'frequency': '$frequency'}
if '$time': spec['time'] = '$time'
if '$days':
    d = '$days'.split(',')
    if '$frequency' in ('monthly','yearly'):
        spec['days_of_month'] = [int(x) for x in d if x.strip().isdigit()]
    else:
        spec['days'] = [x.strip().lower() for x in d]
if '$months': spec['months'] = [x.strip().lower() for x in '$months'.split(',')]
if '$every': spec['every_seconds'] = int('$every') * (60 if '$unit' == 'minutes' else 3600 if '$unit' == 'hours' else 1)
if '$start': spec['start'] = '$start'
if '$end': spec['end'] = '$end'
print(json.dumps(spec))
print('FREQ:' + '$frequency')
"
}

