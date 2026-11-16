_study_page_feedback_loop() {
    cat <<'EOF'
k2 study feedback-loop — changing this box

THIS CHAT VS THE BOX
  Talk in this chat:     `k2 thread <addr> "…"`
  Change the product /
  policy / access:       `k2 feedback ask "<title>"`

  Thread is the overlay. Feedback is a durable card on the human's
  Feedback page. It survives your session. Do not use a vanishing
  terminal prompt for an approval you need to keep.

ASK
  k2 tickets template > brief.html      fill in Problem / Tried / Need / Options
  k2 feedback ask "<title>" --html brief.html --assign <user> [--body "…"] [--options a,b,c] [--wait]
  Returns an id immediately unless --wait. Options are choices for
  the human; --wait blocks until answered or resolved. The brief is
  how the human reads your problem — see `k2 study ticket-brief`.
  Assign every ticket to a user on this server (the owner or a Connect
  user: `k2 connections list --users`). Without one, the owner's setting
  decides: a warning or a refusal (`k2 tickets settings`).

FOLLOW
  k2 feedback list
  k2 feedback show <id>                 each message's files print under it
  k2 feedback comment <id> "…" [--file path]   text optional with --file
  k2 feedback resolve <id>     you got what you needed
  k2 feedback resolve <id> --answered "<outcome>" [--file path]   settled a discussion

FILES
  --file path[:line[-end]] on ask, comment and resolve --answered (up to
  10). A pointer to a file in the TICKET's workspace, never a copy: the
  person sees it under your message (images as pictures). Secrets are
  refused; use k2 vault. A person's files reach you as
  "Attached: <abs path>" lines; read them from disk.

HOW THE PERSON ANSWERS
  Picking one of your options answers the ticket (status answered).
  A free-text message puts it in needs_discussion: reply with
  `k2 feedback comment`, settle it, then mark it answered
  (`resolve <id> --answered "<outcome>"`) or resolved. You decide when
  the discussion is done. --wait exits 5 with their message.

WHEN TO ASK (NOT RETRY)
  Mail send is off
  db_agent_access will not let you create
  owner_only / gated / forbidden (exit 3)
  You want a license, hosting, or Connect change
  You want a setting flipped (CLI Tools, DNS, connections, apps)

WHEN NOT TO
  Do not --wait on `k2 thread ask` (cards are fire-and-continue).
  Do not `k2 msg` your human's username.
  Do not loop owner verbs hoping they become allowed.

THE COPY LINE
  K2 is MIT-licensed open source. That is the product fact. Changing
  it is a human decision (`k2 feedback ask`), not a CLI flag. Talking
  in this chat is `k2 thread`.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_ticket_brief() {
    cat <<'EOF'
k2 study ticket-brief — the HTML brief every agent ticket carries

WHEN
  Every `k2 tickets ask` you file as an agent carries a brief: a short
  HTML page that explains the problem and what you need. The human reads
  it as a formatted page above the thread, so the ticket reads in one
  pass. `--kind fyi` is exempt (a brief is optional there).

THE RULES ARE OWNER SETTINGS (Settings → Tickets)
  The owner (or an admin) sets each rule to off, warn or require:
    Require an HTML brief    missing → brief_missing warning (warn,
                             exit 0) or brief_required (require, exit 2)
    Require an assignee      none → assignee_required warning (warn) or
                             refusal (require, exit 2)
    Default assignee         a ticket with no --assign goes to this user
  Both rules start at warn. Read them: `k2 tickets settings`. You can't
  change them (owner_only, exit 3); a refusal names the setting. Attach a
  brief and assign every ticket anyway: that never fails.

ASSIGN IT
  A ticket must be assigned to a user on this server (`--assign <user>`).
  A user on this server is the owner (`owner`, or the owner's display
  name) or a Connect user: k2 connections list --users
  assignee_unknown = a name that is not a user here. `--kind fyi` is
  exempt. Fix a filed ticket with k2 tickets assign <id> <user>

HOW
  k2 tickets template > brief.html        skeleton with the five sections
  (edit it)
  k2 tickets ask "<one-line title>" --html brief.html --assign <user> [--body "summary"]
  k2 tickets ask "<title>" --assign <user> --html - < brief.html     stdin works too

  <title> stays the one line for the list, push, and search.
  --body is the plain one-or-two-sentence summary text-only readers see.

FORMAT (sections; only "What I need from you" is highlighted)
  Problem                 what is wrong, in plain words
  What I tried            commands, results, why it did not work
  What I need from you    <section class="k2-need"> — the decision or action
  Options                 <ol class="k2-options"> — match --options when used
  Context                 logs in <pre>, screenshots as data: images
  A missing k2-need section is a warning (brief_no_need), not an error.
  Each k2-options <li> becomes a quick-answer button. Start each item with
  a short <strong> label: that label is what comes back as the answer.
  A pick marks the ticket answered. Free text marks it needs_discussion:
  reply, settle it, then `k2 tickets resolve <id> --answered "<outcome>"`.

CAPS
  1 MiB of raw HTML, inline images included. Over → brief_too_large.
  It must have visible text after cleaning (else brief_empty) and be
  UTF-8 (else brief_invalid). Every brief_* error exits 2.

WHAT GETS STRIPPED (the daemon cleans it when it is saved)
  Kept tags: h1-h4 p br hr ul ol li dl dt dd strong em b i u s mark
  small sub sup code pre kbd blockquote a img table caption thead tbody
  tfoot tr th td section header footer div span details summary figure
  figcaption. Classes: k2-need k2-options k2-callout k2-brief only.
  Removed: script, style (and its contents), iframe, form, input, meta,
  base, link, svg, comments, every on* attribute, style= attributes,
  remote images, javascript: links. Only data:image/(png|jpeg|gif|webp)
  images survive. A {"warning":{"code":"brief_sanitized"}} says so.

LINKS
  Links do not click inside the frame. The human sees them as a list
  under the brief with the full URL, opened in their system browser.
  Write the URL so it reads well in that list.

FILES BESIDE THE BRIEF
  The brief keeps its own inline data: images. To share a workspace file
  (a screenshot, a report, a log), attach it: --file out/shot.png on ask
  or comment. It shows under the opening message, right beside the brief,
  and stays a pointer to the file (edits show as changed).

AFTER FILING
  The brief can't be edited. Correct it in a comment, or file a new
  ticket. Comments stay plain text (files ride them with --file).
  k2 tickets show <id>            body + brief text extract
  k2 tickets show <id> --html     the cleaned HTML only

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_zen() {
    cat <<'EOF'
k2 study zen — Zen Gardens in one page

WHAT
  Zen Mode is a mode of a K2 desktop window. In Zen the window shows a
  Garden: a personal page that lives on this computer, in
  ~/.k2/zen/gardens/<id>.toml. Agents build Gardens by editing that file
  when the human asks ("In my Zen Garden ... (id g-...)").

THE ONE RULE
  Every page keeps two buttons the human can reach in one click: the Zen
  toggle (the way out) and the Garden switcher. Where they sit, and
  whether usage, the theme control or a menu is shown, is the page's
  choice. k2 zen validate refuses a file that drops one.

THE LOOP
  k2 zen garden list                 find the Garden's id
  edit ~/.k2/zen/gardens/<id>.toml
  k2 zen validate --garden <id>      before you say it's done

THE FULL GUIDE
  k2 zen guide                       short pages; no daemon needed
  k2 zen guide gardens               start here
  k2 zen guide example --list        whole files you can pipe

A PHONE
  Every Garden shows on a phone: one screen per column until its file adds
  [[phone.screen]] (k2 zen guide phone). No per-device Garden lists.

FULL CANVAS AND LOCKS
  On [layout] canvas = "full" K2's top band floats over a custom widget:
  take clicks with k2.canvas.setHitRegions(rects), drag the window with
  k2.window.startDrag() (k2 zen guide api). A locked Garden: don't edit it;
  K2 refuses (exit 3, garden_locked) and only the person unlocks it.

WRITE THROUGH K2 (WHEN YOU'RE ON THE GARDEN'S LIST)
  k2 zen garden put <id> --from page.toml · k2 zen widget put <name> <file> --from <file>
  You edit a Garden when you're on its list (k2 zen garden access <id>);
  otherwise K2 asks the person (exit 3, a garden.access card). Never K2's
  built-ins (k2:<name>): copy one with k2 zen widget new --from. Then sign
  it: k2 zen validate --widget <name> --sign. k2 study garden-access.

THE WIDGET DEV LOOP (keep your widget outside ~/.k2)
  k2 zen validate --dir ./board              check; writes nothing
  k2 zen widget preview --dir ./board        on K2's own Garden, never one of the person's
  k2 zen widget preview --dir ./board --shot board.png --theme dark   headless PNG
  k2 zen widget put board --dir ./board      one checked write (you need the Garden's list)
  k2 zen widget errors board [--clear]       runtime errors; --clear drops stale ones
  validate never signs (--widget --sign does); "pending" exits 0, --strict keeps exit 2.
  Runtime errors fail only a placed widget's saved version in the last 24 h.

THE WIDGET SDK
  Refusals are K2Errors with a code and retryable. Subscribe with a handler
  object ({data, error, state}) and K2 re-subscribes after a retryable
  refusal; since 0.46.3 k2.sources.subscribe(addr, cb, onError) does too.
  Light/dark follow the Garden, never the OS (--zen-*, [data-zen-scheme];
  manifest "colorScheme": "own" opts out). Charts: requires.libs uplot@1 (k2 zen guide libs).

ON A PHONE (K2 Companion, 0.46.6)
  The person's paired phone runs the same widget in the same sealed frame.
  Detect it with matchMedia('(pointer: coarse)') and handle your own touch
  gestures; drop never fires there, so always offer a pick button. Never rely
  on canvas.toDataURL / toBlob (empty on WebKit): use getImageData, or
  requires.libs k2-png@1 (K2Png.toBlob(canvas)).

THE WIDGET KIT (0.46.3): don't hand-build these, require them
  "k2-thread@1"  K2Thread.mount(el, {look: 'chamfer' | 'glass', address})
                 a Thread panel: bubbles, Enter sends, the working tab, the
                 new-messages pill. chamfer = K2's Thread; glass = Companion.
  "k2-help@1"    K2Help.tip(text, {label}) → an ⓘ. Put explanations
                 behind an ⓘ, not on the canvas.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_sidecars() {
    cat <<'EOF'
k2 study sidecars — extra AI sessions in a workspace

WHAT
  A sidecar is an extra AI session in a workspace. It shares the
  workspace's role, skills and inbox; it is not a second agent (see
  k2 study identity). Its address is <workspace>/<name>.

VERBS
  k2 sidecar new <name> --harness <preset> --brief <file>   prints the address
  k2 sidecar list                                           live and asleep
  k2 sidecar stop <name>                                    stop is sleep
  k2 sidecar archive <name> | --all-stopped                 off the list; the chat stays
  k2 sidecar access on|off|status --workspace <ws>          owner/admin

RULES
  Names: lowercase letters, digits, single hyphens; not all digits; 40 max.
  new on a stopped name resumes the same conversation and address.
  stop keeps the address, name, history and brief. k2 msg wakes it.
  archive is for stopped sidecars you're done with: it takes them off
  k2 sidecar list. The name and conversation stay: new on that name with the
  same harness resumes it, k2 sessions --past finds it; a fresh chat needs a
  new name. Running ones are refused (stop first); a worktree sidecar is
  refused too (retire it, which a person does). --all-stopped skips those
  and any you may not archive, and says which.
  The brief is saved to .k2/sidecars/<name>/BRIEF.md (0600, 64 KiB max);
  the sidecar gets one line pointing at it. No secrets in a brief.
  At most 8 live sidecars per workspace.

WHO
  Agents: only the main session, only with "Let this agent manage its
  sidecars" on, only its own workspace, only presets, never more power
  than itself; it stops only sidecars an agent opened. Sidecars cannot
  open sidecars. Hiring a new agent (k2 agent hire) is a separate
  switch, "Let this agent hire agents" (agents.hire). Where agents run
  as the owner's user, these are guardrails, not locks.

HANDBACK
  Write in the brief: "when done, k2 msg <primary> with a summary".

SEE ALSO
  k2 study sessions: everyone in this workspace, past conversations to reopen.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_sessions() {
    cat <<'EOF'
k2 study sessions — who is working in this workspace, and past conversations

WHO IS HERE
  k2 sessions            live sessions: address, role, CLI, state, what it is on
  k2 sessions --all      also the asleep ones (a message wakes them)
  k2 connections         shows the same list first ("This workspace")
  Roles: main (the pinned chat, address <ws>), sidecar (opened by k2 sessions
  new/open or k2 sidecar), chat (a tab a person opened), heartbeat (a
  scheduled run). * marks you. States: working and waiting (needs a person)
  are busy; idle is free; asleep wakes when messaged.
  Addresses for k2 msg: <ws> is main (this workspace's name), <ws>/<name> a
  named one, <ws>/<n> (a number) an unnamed one.

TALK TO ONE
  k2 msg <address> "…"          live or asleep (it wakes); never yourself
  Same workspace is always allowed. Replies arrive in your terminal as
  [from <address>] …; answer with k2 msg <address>.

PAST CONVERSATIONS (this workspace's folder and its .worktrees/ only)
  k2 sessions --past [--search "<words>"] [--days N] [--cli <cli>]
  k2 sessions show <id>          first ask, last messages (facts, no summary)
  <id>: the short id --past prints (6+ characters), the full id, or an address;
  a prefix that fits several is refused (id_ambiguous) with the candidates.
  Search matches the whole phrase (any case) in titles, then message text,
  newest first, within 400 files / 64 MB / 5 s; past that it says "search
  stopped" (narrow with --days, --cli).
  CLIs: claude, codex, grok, gemini, cursor, pi, hermes. Cursor keeps no cwd
  (worktree chats are not found) and no message text (show lists facts only).

REOPEN (same history)
  k2 msg --session <id> "…"      held: delivered to its holder; past: reopened,
                                 and your message is its first turn
  k2 sessions open <id> [--name <name>]   reopen without a message; prints the address
  One conversation never runs twice: if something holds it, you get its address.
  Refused, exit 1: conversation_recently_active (written in the last 120 s,
  maybe in a terminal outside K2: --force if it is closed), sidecar_cap (8
  live), name_differs, folder_gone/worktree_gone (410), folder_trust_pending.

START AND STOP HELPERS
  k2 sessions new <name> --brief <file> [--cli <cli>]   prints <ws>/<name>; default: your CLI
  k2 sessions stop <name>        stop is sleep; open or msg brings it back
  A heartbeat run (hb:<name>) is not a helper: k2 heartbeat restart <name>
  stops it and its next fire starts fresh (any session here may; no switch).
  In the brief, say "when done, k2 msg <ws> with a summary". A helper works in
  your checkout: for a separate branch, tell it to make its own git worktree.

THE MAIN CHAT
  k2 sessions main               which chat is pinned as this workspace's main
                                 chat, and whether K2 can find its conversation
                                 (--json: state found, missing, unstarted, none
                                 or custom; missing exits 1 without --json)
  k2 sessions main set <id>      choose it: an <id> from k2 sessions --past (past
                                 chats are read from every LLM account's folder,
                                 so a chat run on a pinned token is listed too).
                                 A person runs it, in a K2 terminal or from the
                                 chat's menu; agents only read it (refused:
                                 main_chat_needs_person; ask your human with
                                 k2 thread). Takes effect the next time the chat
                                 starts: k2 agent restart <ws>, or the next k2 msg.
    --thread keep-main           (default) the main chat keeps its Thread; the
                                 chosen chat's own Thread stays at <ws>/<n>
    --thread adopt-sidecar       the chosen chat's Thread becomes the main
                                 Thread; the old main Thread stays at <ws>/<n>
                                 (nothing is deleted; the answer prints <ws>/<n>)
  If the pinned chat cannot be found anywhere, K2 keeps the pin, starts nothing
  and says so (main_chat_missing) instead of picking another chat. Fix it with
  main set. To give up on it and start over: k2 agent refresh <ws>.

WHO MAY
  Reading (list, past, show): any session, its own workspace only. A
  sandboxed session sees the list only.
  Opening, starting, stopping: the main session only, with "Let this agent
  manage its sidecars" on (exit 3 gated, or main_only: ask main with k2 msg <ws>).
  An agent never opens a session with more power than itself.

EXIT CODES
  0 ok · 1 conflict (409/410) · 2 usage · 3 refused · 4 not found

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

