#!/usr/bin/env bash
# 0.46.17 held App cards / --assign / vault.accept-from: the bites. Each one
# breaks one rule in the source, runs the test that pins it, and expects it
# to FAIL; then the file is put back. Run on a compute node from a clean
# checkout of the branch: bash scripts/test/held-cards-bites.sh
set -uo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
pass=0; fail=0

# bite <name> <file> <python replace: old> <new> <nextest filter>
bite() {
    local name="$1" file="$2" old="$3" new="$4" filter="$5"
    # BITES=<words>: only the bites whose name holds them (a job each fits
    # well under the 2 h job limit).
    case "$name" in *"${BITES:-}"*) ;; *) return ;; esac
    cp "$file" "/tmp/bite.bak"
    if ! python3 - "$file" "$old" "$new" <<'PY'
import sys
p, old, new = sys.argv[1:4]
s = open(p).read()
if old not in s:
    sys.exit(1)
open(p, "w").write(s.replace(old, new, 1))
PY
    then
        echo "BITE $name: the line to break wasn't found (stale bite)"; fail=$((fail + 1)); cp /tmp/bite.bak "$file"; return
    fi
    if cargo nextest run -p k2-daemon --lib -E "test(/$filter/)" >/tmp/bite.log 2>&1; then
        echo "BITE $name: NOT caught (the test still passes)"; fail=$((fail + 1))
    elif grep -q "error\[E" /tmp/bite.log; then
        echo "BITE $name: didn't compile"; grep -E -A6 "error\[E" /tmp/bite.log | head -20; fail=$((fail + 1))
    else
        echo "BITE $name: caught"; pass=$((pass + 1))
    fi
    cp /tmp/bite.bak "$file"
}

A=crates/k2-daemon/src/apps
bite "an agent lists another workspace's requests" "$A/approvals.rs" \
    "status: state, requester: own_ws(who) };" "status: state, requester: None };" \
    "an_agent_sees_only_its_own_requests"
bite "Deny on the card leaves the held request pending" crates/k2-daemon/src/permission/answer.rs \
    "crate::apps::held_cards::on_denied(&denied, who, reason);" "let _ = (&denied, who, reason);" \
    "deny_on_the_card_denies_the_held_request"
bite "approved in Settings leaves the card open" "$A/approvals.rs" \
    "    super::held_cards::close_decided(&done);" "    let _ = &done;" \
    "a_request_decided_elsewhere_closes_its_card"
bite "--assign is ignored" "$A/held_cards.rs" \
    "if let Some(names) = assigned() {" "if let Some(names) = None::<Vec<String>> {" \
    "assign_sends_the_card_to_the_named_person_only"
bite "a cards-off agent's hold raises no card" "$A/approvals.rs" \
    "let p = super::held_cards::present_for(r);" "let p = json!({ \"card\": null });" \
    "every_held_request_of_an_agent_gets_a_card"
bite "a bad --assign name runs the command anyway" "$A/held_cards.rs" \
    "Some(names) => approvers::check_named(&names, floor).map(Some).map_err(refused)," "Some(names) => Ok(Some(names))," \
    "assign_refuses_a_name_that_cant_approve"
bite "a vault send needs a connection even with accept-from" crates/k2-daemon/src/vault_handoff_routes.rs \
    ".filter(|_| !accepts)" ".filter(|_| true)" \
    "accept_from_is_consent_to_receive_without_a_connection"
bite "accept-from Allow changes nothing" crates/k2-daemon/src/permission/actions/vault_accept_from.rs \
    "\"action\": \"add\"" "\"action\": \"remove\"" \
    "one_card_names_every_receiver_and_allow_lets_each_accept"
bite "the accept-from card takes a swapped receiver" crates/k2-daemon/src/permission/actions/vault_accept_from.rs \
    "out.params.insert(\"who\".into(), json!(and_list(&who)));" "out.params.insert(\"who\".into(), json!(\"them\"));" \
    "what_it_takes_is_checked_at_the_ask_and_at_the_click"

echo "bites: $pass caught, $fail not"
[ "$fail" -eq 0 ]
