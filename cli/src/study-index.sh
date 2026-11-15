# ── `k2 study` — bounded teaching pages (daemon-optional) ────────────
# Static in-CLI copy. Not --help. Not a crawl of .k2/wiki or .k2/prds.
# Runtime cannot cat sibling markdown after install — pages live here.

_STUDY_TOPIC_IDS="what source map identity send human people auth errors context db mail connect-boundary apps app-deploys app-move app-api app-errors live app-heartbeats app-tickets app-identity app-surfaces app-resources app-runtime app-handover app-push app-pairing auth-pages app-profile app-account-deletion app-transfers app-transfers-http app-chat app-recording chat-voice chat-voice-rooms voice-rooms tauri tauri-voice skins feedback-loop ticket-brief zen sidecars sessions llm-tokens llm-accounts publish-framing publish-visibility publish-dev compute compute-shared ci ci-release ci-stats provenance vault notifications data-sources projects hooks agent-hire garden-data garden-access garden-changes changes docs sim design db-upgrade app-databases jobs heartbeats mail-watch mail-reader folder-trust email-notifications box-move board chat-refresh desktop send-feedback organization agent-templates suite-apps"

_study_usage() {
    cat <<'EOF'
Usage: k2 study [list|<topic>|--json]

Bounded teaching pages. Not a reprint of --help. Not a crawl of wiki or PRDs.

  k2 study                 catalog
  k2 study list            same
  k2 study <topic>         one page
  k2 study --json          catalog JSON
  k2 study <topic> --json  page JSON {id,body}

Topics:
  what  source  map  identity  send  human  people  auth  errors
  context  db  mail  connect-boundary  apps  app-deploys  app-move  app-api  live  app-heartbeats  app-tickets
  app-identity  app-surfaces  app-resources  app-runtime  app-handover  app-push  app-pairing  auth-pages  app-profile
  app-account-deletion  app-transfers  app-recording
  app-transfers-http  app-chat  chat-voice  chat-voice-rooms  voice-rooms  tauri  tauri-voice  app-errors
  skins  feedback-loop  ticket-brief  zen  sidecars  sessions  llm-tokens  llm-accounts  publish-framing
  publish-visibility  publish-dev  compute  compute-shared  ci  ci-release  ci-stats  vault
  notifications  data-sources  projects  hooks  agent-hire
  garden-data  garden-access  garden-changes  changes  sim  docs  db-upgrade  app-databases  jobs  heartbeats
  folder-trust  email-notifications  box-move  board  chat-refresh  desktop  send-feedback  organization
  agent-templates  provenance  suite-apps
EOF
}

_study_title() {
    case "$1" in
        what) echo "What K2 is" ;;
        source) echo "License and repo boundary" ;;
        map) echo "How the surface is organized" ;;
        identity) echo "Who you are" ;;
        send) echo "How to send work" ;;
        human) echo "Talking to your human" ;;
        people) echo "Four people lists" ;;
        auth) echo "Identity and gates" ;;
        errors) echo "Exit codes" ;;
        context) echo "Always-on context" ;;
        db) echo "Workspace database" ;;
        db-upgrade) echo "Moving a database to a newer Postgres" ;;
        app-databases) echo "An App's database: what reaches what, and connecting one" ;;
        mail) echo "Agent email" ;;
        connect-boundary) echo "K2 vs Connect" ;;
        apps) echo "Apps" ;;
        app-deploys) echo "Deploying Apps" ;;
        suite-apps) echo "One-click suite Apps (Hearth, Meet)" ;;
        app-move) echo "Moving your --cmd service to an App" ;;
        app-api) echo "The App API: server SDK and gateway routes" ;;
        app-errors) echo "App refusals: every id, its fix and who fixes it" ;;
        live) echo "Live surfaces: what an App or a Garden widget can show live" ;;
        app-heartbeats) echo "Heartbeats in an app" ;;
        app-tickets) echo "Tickets in an app" ;;
        app-identity) echo "An agent's name, color, picture and handle in an app" ;;
        app-surfaces) echo "Room members, and an App's own self and usage" ;;
        app-resources) echo "Box usage, the disk guard, clean and trends" ;;
        app-runtime) echo "K2 updates and your App: the re-host" ;;
        app-handover) echo "Handing an App to another agent" ;;
        app-push) echo "Notifications to a custom App's own phone app" ;;
        app-pairing) echo "Pairing a custom App's own phone app by QR" ;;
        auth-pages) echo "Your own sign-in pages: passwords straight to K2" ;;
        app-profile) echo "A person's own profile, picture and sessions in an App" ;;
        app-account-deletion) echo "A person deletes their own App account: store rules, modes, what stays" ;;
        app-transfers) echo "Big files that survive the app in the background" ;;
        app-transfers-http) echo "Resumable uploads over plain HTTP: every field, refusal and curl" ;;
        app-chat) echo "Chat, voice and calls in your App (Chat / Voice Hosting)" ;;
        app-recording) echo "Recording in an App's calls: consent, required, server reads, the lock" ;;
        chat-voice) echo "Voice rooms and channels in your App: the setup" ;;
        chat-voice-rooms) echo "Rooms and channels in your App Space" ;;
        voice-rooms) echo "Voice rooms on this server (preview)" ;;
        tauri) echo "The K2 side of a Tauri app, end to end" ;;
        tauri-voice) echo "Calls inside a Tauri app's window" ;;
        skins) echo "Apps" ;;
        feedback-loop) echo "Changing this box" ;;
        ticket-brief) echo "Ticket HTML brief" ;;
        zen) echo "Zen Gardens" ;;
        sidecars) echo "Sidecars" ;;
        sessions) echo "Sessions in this workspace" ;;
        llm-tokens) echo "LLM tokens" ;;
        llm-accounts) echo "LLM tokens" ;;
        publish-framing) echo "Who may frame your App" ;;
        publish-visibility) echo "Private, PIN or public: who may open your site" ;;
        publish-dev) echo "Host a dev server with hot reload" ;;
        compute) echo "Compute nodes" ;;
        compute-shared) echo "Shared compute nodes" ;;
        ci) echo "CI on K2 Compute" ;;
        ci-release) echo "Signed release builds on K2 Compute" ;;
        ci-stats) echo "CI stats, monitors and the [ci] run message" ;;
        provenance) echo "Build statements: what a node signs about a build, and how to check one" ;;
        vault) echo "Secrets (the vault)" ;;
        jobs) echo "Scheduled jobs the daemon runs" ;;
        heartbeats) echo "Heartbeats and one-time reminders (routines, cron)" ;;
        folder-trust) echo "Who may start AI tools in a folder" ;;
        email-notifications) echo "One-way email from senders no agent can read" ;;
        mail-watch) echo "Hear that mail arrived: VIPs, the digest, what's safe" ;;
        mail-reader) echo "The mail reader: people open inboxes; agents can't; every access logged" ;;
        box-move) echo "Moving a box: the vault and the tunnel token" ;;
        board) echo "The Board Garden: k2 board, grants, the rules" ;;
        chat-refresh) echo "Refreshing your own chat: k2 agent refresh, the note, what carries on" ;;
        desktop) echo "Remote desktop: this computer's screen in a Desktop tab (people only)" ;;
        send-feedback) echo "Telling K2's makers about K2: k2 send-feedback, what is sent, agents' limits" ;;
        organization) echo "The Org Chart: orgs, heads and the daily bulletin" ;;
        agent-templates) echo "Agent templates: hire a type, save your own, upgrade" ;;
        notifications) echo "Thread posts and phone pushes" ;;
        data-sources) echo "Data sources and triggers" ;;
        projects) echo "K2 projects and who may change them" ;;
        hooks) echo "Custom agents: Thread and lifecycle hooks" ;;
        agent-hire) echo "Hiring agents through the v1 API" ;;
        garden-data) echo "Garden data: a widget's keyspace and grants" ;;
        changes) echo "Git changes: review, stage, discard, commit" ;;
        sim) echo "The mobile device: tree first, act, read the diff" ;;
        docs) echo "Live docs: edit Markdown, code and text with people and agents" ;;
        design) echo "Design notes: what a Design tab sends you" ;;
        permissions) echo "Permission cards (Beta)" ;;
        garden-access) echo "Garden access: who may edit a Garden" ;;
        garden-changes) echo "What changed for Gardens in 0.46.3" ;;
        *) echo "" ;;
    esac
}

_study_is_topic() {
    local id="$1" t
    for t in $_STUDY_TOPIC_IDS; do
        [ "$t" = "$id" ] && return 0
    done
    return 1
}

# Permission cards (Beta): `permissions` is a topic only while both
# switches are on for this agent (inside a K2 cell; PRD rev 3.1 §11.4).
# Off, these print exactly `_STUDY_TOPIC_IDS` / `_study_usage` /
# `_study_catalog`.
_study_topic_ids_all() {
    local ids="$_STUDY_TOPIC_IDS"
    if _permission_cli_enabled; then ids="$ids permissions"; fi
    printf '%s\n' "$ids"
}

_study_usage_all() {
    # After the topics block's last line. The block ends the usage text, so
    # the line goes at the end: anchoring on one line's words broke when the
    # block reflowed (`agent-hire  garden-data` no longer shares a line), and
    # `k2 study --help` lost the topic while permission cards were on.
    _study_usage
    if _permission_cli_enabled; then
        echo "  permissions"
    fi
}

_study_catalog_all() {
    _study_catalog | {
        if _permission_cli_enabled; then
            awk '{ print } /^  design   / { print "  permissions         Permission cards (Beta): ask a person to allow what K2 refused you" }'
        else
            cat
        fi
    }
}

_study_catalog() {
    cat <<'EOF'
k2 study — bounded teaching pages (not --help)

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.

  what                What this product is (agents as infrastructure)
  source              License + repo boundary (MIT)
  map                 How the surface is organized (pages, not --help)
  identity            Who you are; handles vs names; sidecars
  send                k2 msg vs k2 thread vs k2 mail vs talk
  human               Thread overlay, feedback, [thread:addr]
  people              Four lists. Never k2 msg usernames
  auth                Owner vs agent; Open vs ID; stop on exit 3
  errors              Exit 0/1/2/3/4 and owner_only / gated / forbidden
  context             AGENTS.md, skills, layers — keep always-on lean
  db                  Loopback Postgres; db_agent_access is create-only
  db-upgrade          Move a database to a newer Postgres; old copy kept 14 days
  app-databases       Connect an App to Postgres (k2 app db); which App kinds reach what
  mail                Agent email; bodies untrusted; send is governed
  connect-boundary    K2 app/daemon/CLI vs Connect (separate product)
  apps                App guests; k2skn_ prefix never grid
  app-deploys         Host a server App: Seeds, deploys, rollback (beta)
  suite-apps          One-click Hearth and Meet: the job, addresses, waits, refusals
  app-move            Move your --cmd service to an App when ready, step by step
  app-api             An App's server calls: the SDK, every route, passes, refusals
  app-errors          Every App refusal id: what happened, the fix, who fixes it (k2 explain)
  live                Live surfaces (turns, counts, unread, files…): caps, read + live, never infer
  app-heartbeats      App heartbeats: routes, caps, wait reasons, contract
  app-tickets         App tickets: routes, live ticket_changed frames, contract
  app-identity        App agent identity: name, color, picture, handle; caps
  app-surfaces        Room members (who holds a grant), an App's own self and usage
  app-resources       Box usage: the disk guard's lines, clean, CPU and memory trends
  app-runtime         K2 updates move your App to the new runtime: re-host, setting, checks
  app-handover        Hand an App to another agent: offer, accept, the owner's card, undo
  app-push            Your App's own phone app: keys in the vault, register, send, limits
  app-pairing         Your App's own phone app signs in by QR: setup, start, check, redeem
  auth-pages          Your own sign-in page posts passwords to K2, not your server: codes, PKCE
  app-profile         A person's own profile, picture and sessions in an App; who decides
  app-account-deletion  Delete my account from inside an App: the store rules, request or immediate, what stays
  app-transfers       Big uploads and downloads that survive a phone app in the background
  app-transfers-http  Resumable uploads without the SDK: every route, field, refusal; curl
  app-chat            Chat, voice and calls in your App: caps, guests.setChat, chat.member/whois
  app-recording       Recording in an App's calls: all off, consent, required, server reads, the card, the lock
  chat-voice          Voice rooms and channels in your App: the order, who does each step, pitfalls
  chat-voice-rooms    Your App Space: static rooms, per-project channels, who's in, joining, limits
  voice-rooms         This server's own voice rooms (preview): k2 voice, who may change or join
  tauri               A Tauri app on K2: web build, server, updates, signing, links, push, uploads
  tauri-voice         Calls in a Tauri window: --voice files, the mic hook, CSP, what is tested
  skins               leftover alias of apps
  feedback-loop       Ask to change this box; talk in this chat
  ticket-brief        The HTML brief every agent ticket carries
  zen                 Zen Gardens: the one rule; the guide is k2 zen guide
  sidecars            Open, list and stop sidecars; the brief; who may
  sessions            Who works in this workspace; past conversations; reopen; one runner
  llm-tokens          LLM tokens: subscriptions and API tokens; Server default
  llm-accounts        leftover alias of llm-tokens
  publish-framing     Who may frame your App; the header for --cmd Apps
  publish-visibility  Private, PIN or public: who may open your site
  publish-dev         Host a dev server with hot reload: k2 publish run --dev
  compute             Run builds/tests on your owner's other machines; logs are data
  compute-shared      A node many K2 servers share: its queue, what's hidden, the wall
  ci                  CI on compute: .k2/ci.toml, k2 ci run/watch, GitHub statuses
  ci-release          Signed releases: k2 ci release on your seat, evidence, the signer
  ci-stats            k2 ci stats, monitors, the one [ci] message per run, mem_auto checks
  provenance          Build statements a node signs (in-toto/SLSA): get one, verify it, why none
  vault               Secrets: fetch not paste; references; who sets what
  jobs                Scripts on a schedule: k2 job, job:<name> secrets, logs, kill
  heartbeats          Wake yourself on a schedule or once (routines, reminders, cron)
  notifications       Your Thread posts may push a phone; who gets it; post less
  data-sources        Checks this server runs: try, propose, approve; triggers; keys
  projects            k2 project: open by default; the per-agent switch
  hooks               Custom (no-model) agents: their Thread; k2 hooks emit
  agent-hire          Apps hire agents/teams with a hire key; links; k2 msg who
  garden-data         A widget's data: k2 store --widget, grants, revs, export
  garden-access       Who may edit a Garden: the list, asking, signing, History
  garden-changes      Gardens in 0.46.3: what's retired, what to update before 0.47.0
  changes             Git changes (the Changes tab): your own workspace; discard = Trash
  sim                 The mobile device: tree, act, read the diff; held = wait; secrets from the vault
  docs                Live docs: k2 doc verbs, presence, rights, shares, exits
  design              Design notes from a page: the shape, k2 browser note, answer in Thread
  folder-trust        Folder trust: the card, asking vs deciding, re-scan, honest limits
  email-notifications  Email from senders: K2's resets, an App's email.send, scope, limits
  mail-watch          Hear that mail arrived: VIPs, the digest, what's safe
  mail-reader         The mail reader: who opens which inbox, peek only, the log; agents refused
  box-move            Move a box: carry the vault (tunnel token too), freeze, undo, who confirms
  board               The Board Garden's whiteboards: k2 board, grants, cascade, no cut values
  chat-refresh        Refresh your own chat: k2 agent refresh, the note, what carries on
  desktop             Remote desktop: this computer's screen in a Desktop tab, people only
  send-feedback       Tell Alakazam Labs about K2 itself: what is sent, never sent, agents' limits
  organization        The Org Chart: read the bulletin daily, reach a head, heads and members
  agent-templates     Hire a type of agent, review first, save your own setup, upgrade safely

Usage: k2 study [list|<topic>|--json]
EOF
}

_study_page() {
    case "$1" in
        what) _study_page_what ;;
        source) _study_page_source ;;
        map) _study_page_map ;;
        identity) _study_page_identity ;;
        send) _study_page_send ;;
        human) _study_page_human ;;
        people) _study_page_people ;;
        auth) _study_page_auth ;;
        errors) _study_page_errors ;;
        context) _study_page_context ;;
        db) _study_page_db ;;
        db-upgrade) _study_page_db_upgrade ;;
        app-databases) _study_page_app_databases ;;
        mail) _study_page_mail ;;
        connect-boundary) _study_page_connect_boundary ;;
        apps) _study_page_skins ;;
        app-deploys) _study_page_app_deploys ;;
        suite-apps) _study_page_suite_apps ;;
        app-move) _study_page_app_move ;;
        app-api) _study_page_app_api ;;
        app-errors) _study_page_app_errors ;;
        live) _study_page_live ;;
        app-heartbeats) _study_page_app_heartbeats ;;
        app-tickets) _study_page_app_tickets ;;
        app-identity) _study_page_app_identity ;;
        app-surfaces) _study_page_app_surfaces ;;
        app-resources) _study_page_app_resources ;;
        app-runtime) _study_page_app_runtime ;;
        app-handover) _study_page_app_handover ;;
        app-push) _study_page_app_push ;;
        app-pairing) _study_page_app_pairing ;;
        auth-pages) _study_page_auth_pages ;;
        app-profile) _study_page_app_profile ;;
        app-account-deletion) _study_page_app_account_deletion ;;
        app-transfers) _study_page_app_transfers ;;
        app-transfers-http) _study_page_app_transfers_http ;;
        app-chat) _study_page_app_chat ;;
        app-recording) _study_page_app_recording ;;
        chat-voice) _study_page_chat_voice ;;
        chat-voice-rooms) _study_page_chat_voice_rooms ;;
        voice-rooms) _study_page_voice_rooms ;;
        tauri) _study_page_tauri ;;
        tauri-voice) _study_page_tauri_voice ;;
        email-notifications) _study_page_email_notifications ;;
        mail-watch) _study_page_mail_watch ;;
        mail-reader) _study_page_mail_reader ;;
        skins)
            echo "k2 study skins is now k2 study apps — this page still works."
            echo
            _study_page_skins
            ;;
        feedback-loop) _study_page_feedback_loop ;;
        ticket-brief) _study_page_ticket_brief ;;
        zen) _study_page_zen ;;
        sidecars) _study_page_sidecars ;;
        sessions) _study_page_sessions ;;
        llm-tokens) _study_page_llm_tokens ;;
        llm-accounts) printf '%s\n\n' "k2 study llm-accounts is now k2 study llm-tokens — this page still works."; _study_page_llm_tokens ;;
        publish-framing) _study_page_publish_framing ;;
        publish-visibility) _study_page_publish_visibility ;;
        publish-dev) _study_page_publish_dev ;;
        compute) _study_page_compute ;;
        compute-shared) _study_page_compute_shared ;;
        ci) _study_page_ci ;;
        ci-release) _study_page_ci_release ;;
        ci-stats) _study_page_ci_stats ;;
        provenance) _study_page_provenance ;;
        data-sources) _study_page_data_sources ;;
        garden-data) _study_page_garden_data ;;
        changes) _study_page_changes ;;
        sim) _study_page_sim ;;
        docs) _study_page_docs ;;
        vault) _study_page_vault ;;
        jobs) _study_page_jobs ;;
        heartbeats) _study_page_heartbeats ;;
        folder-trust) _study_page_folder_trust ;;
        notifications) _study_page_notifications ;;
        projects) _study_page_projects ;;
        hooks) _study_page_hooks ;;
        agent-hire) _study_page_agent_hire ;;
        design) _study_page_design ;;
        permissions) _study_page_permissions ;;
        garden-access) _study_page_garden_access ;;
        garden-changes) _study_page_garden_changes ;;
        box-move) _study_page_box_move ;;
        board) _study_page_board ;;
        chat-refresh) _study_page_chat_refresh ;;
        desktop) _study_page_desktop ;;
        send-feedback) _study_page_send_feedback ;;
        organization) _study_page_organization ;;
        agent-templates) _study_page_agent_templates ;;
        *) return 1 ;;
    esac
}

