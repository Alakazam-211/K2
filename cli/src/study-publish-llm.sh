_study_page_publish_visibility() {
    cat <<'EOF'
k2 study publish-visibility — who may open your published site

THE THREE MODES
  Every site at https://<name>.<host>.k2.dev has one:
    private   Only the server's owner and Admins. K2 checks who they are
              before sending a byte of the site. This is the default for a
              new site.
    pin       Nothing loads until the visitor types the site's PIN.
    public    A normal website: anyone with the address.
  Sites from before 0.46.7 stay public until someone changes them. The owner
  got one card listing them.

SEE IT, CHANGE IT
  k2 publish ps                               the VISIBILITY column
  k2 publish visibility <name>                this site's mode
  k2 publish visibility <name> private
  k2 publish visibility <name> pin < pin.txt  (or type it when asked)
  k2 publish visibility <name> public --reason "launch day"

WHO MAY
  Make a site private: an agent on its own site, or any person.
  Set a PIN or make a site public: the server's owner, or a login with the Admin
  role. An agent asks instead: the same command puts a card on its Thread (its
  chat with its person) and exits 2, which means waiting for a person. The answer
  arrives as a [permission:<id>] message in its chat. For a PIN, the person types
  it into K2's own form, so the agent never sees it. Exit 3 means refused.

OPEN A PRIVATE SITE
  In a K2 tab: just go to the address. K2 signs you in.
  In another browser: k2 publish open <name>  gives a link that works once,
  within 60 seconds. Sign-in lasts 12 hours and ends within a minute if your role
  changes. There is no password form on the site, on purpose. Anyone else who
  opens a private site sees K2's "This site is private" page and nothing of it.
  Agents: test on the local port (http://127.0.0.1:<port>). k2 publish open
  refuses an agent (person_only).

SHARE WITH A PIN
  Send the address and the PIN separately. PIN: 6 to 64 characters. Under 8, K2
  warns. A sign-in lasts 7 days (--pin-days 1 to 30). Changing the PIN or the mode
  ends every PIN sign-in. There's no timer: to share for an hour, switch back to
  private after it. Too many wrong tries wait 15 minutes per address, and an hour
  per site; people already in stay in.
  k2 publish sessions <name>                  who is signed in (private and PIN)
  k2 publish sessions <name> --revoke-all     end all of them

THE WARNING ABOUT LISTENING ON EVERY ADDRESS
  K2's check sits at the site's address. If your program listens on 0.0.0.0 or a
  LAN address, anyone who can reach this machine on that port gets the site
  without the check. K2 says so in ps and visibility ("<name> listens on
  0.0.0.0:3000: anyone who can reach this machine…"). The fix is your program's:
  bind it to 127.0.0.1 and restart it. A public site doesn't get the warning.

WHAT CAN'T BE PRIVATE OR PIN
  A private or PIN site must speak HTTP/1.1, because K2 reads every request. A
  raw-TCP service can stay public. K2 warns before it changes a public site, and
  asks for --yes when there is no terminal.
  K2 uses the path /_k2/site/ on a private or PIN site, so your site's own files
  under that path aren't reachable there.
  With K2 Connect's end-to-end encryption off, private and PIN are refused
  (visibility_door_unsupported), because K2 can't check visitors on that path.
  Apps are not covered: their own logins decide.
EOF
}

_study_page_publish_dev() {
    cat <<'EOF'
k2 study publish-dev — host a dev server with hot reload

START
  k2 publish run web --cmd "npm run dev" --port 5173 --dev
  It runs in your live workspace folder (or --cwd), so your edits hot reload, at
  https://web.<host>.k2.dev. It is a normal k2 publish service: ps, logs, stop,
  start, rm, visibility and open all work. To turn it back into a normal service,
  run it again without --dev.

WHAT YOUR SERVER GETS
  HOST=127.0.0.1  PORT=<n>  K2_DEV_PUBLIC_HOST=web.<host>.k2.dev
  K2_DEV_PUBLIC_PORT=443  K2_DEV_PUBLIC_PROTOCOL=wss

CONFIGURE IT
  Both block unknown hosts by default, and Vite's socket needs the public port.
  Vite (vite.config.js):
    server: {
      host: process.env.HOST, port: Number(process.env.PORT),
      allowedHosts: [process.env.K2_DEV_PUBLIC_HOST],
      hmr: {
        host: process.env.K2_DEV_PUBLIC_HOST,
        clientPort: Number(process.env.K2_DEV_PUBLIC_PORT),
        protocol: process.env.K2_DEV_PUBLIC_PROTOCOL,
      },
    }
  Next (next.config.js):
    allowedDevOrigins: [process.env.K2_DEV_PUBLIC_HOST],
    and run: next dev -H 127.0.0.1 -p $PORT
  tauri dev: publish its web side the same way (the Vite or Next server above),
  and leave the native window on your own machine.

WHO CAN SEE IT
  It starts private. Open it in a K2 tab: you just see the site.
  Pair programming: k2 publish visibility web pin. Your friend opens the address,
  types the PIN and gets the same hot reload. Switching back to private cuts them
  off. Send the address and the PIN separately.
  Public dev needs an owner or Admin, --dev-is-public, a reason, and typing the
  name to confirm. An agent can't do it or ask for it (dev_public_person_only): ask
  for a PIN instead.

WHAT IT SHOWS
  A dev server shows more than a normal site: source maps, error pages with file
  paths, and anything the dev server evaluates. Whoever can open it can see them.
  Every answer carries X-K2-Mode: dev and X-Robots-Tag: noindex. K2 shows a PIN
  visitor a one-screen notice first.

IDLE STOP
  With no visits for --idle (default 60m, 5m to 24h) K2 stops the server. ps says
  idle-stopped, which isn't a crash. The next visitor who passes K2's check sees
  "Starting the dev server…", and it comes back. An agent brings it back with
  k2 publish start. --idle off is for the owner and Admins. A server someone
  stops with k2 publish stop stays stopped until k2 publish start.

WHERE IT RUNS
  As your workspace's user, in your folder, with no wall (the sandbox Apps run
  in) around it. So an agent's first --dev run of a name waits for the owner (exit
  2), as a new --cmd service does.
  Linux caps it (--mem, default 4G; --cpus, default 2) where systemd user scopes
  exist. ps --long shows what applied, or "no caps". macOS has none.
  K2 warns when the box's file-watch limit could stop hot reload silently.

SHIP IT
  --dev is the opposite of an App: live folder, no copy, no rollback, no wall. A
  dev site never becomes an App. To ship, build it and make an App (k2 study
  app-deploys). The App is separate: the dev site stays, under its own name and
  idle stop, until you k2 publish rm it.
EOF
}

_study_page_publish_framing() {
    cat <<'EOF'
k2 study publish-framing — who may put your App in a frame

WHY
  A site that frames your App can lay invisible buttons over it and
  trick a signed-in guest into clicking (clickjacking). The browser
  only refuses when the App's response says who may frame it:
    Content-Security-Policy: frame-ancestors <who>
  A <meta> tag cannot carry frame-ancestors. It must be a header.

--skin APPS (K2 serves the responses)
  Every response, login page and error pages included, carries:
    Content-Security-Policy: frame-ancestors 'self' tauri://localhost http://tauri.localhost
    X-Frame-Options: SAMEORIGIN
  'self' is the App's own origin; the tauri ones are K2's desktop
  windows. Every other site is refused.
  Owner only, from the UI or a shell outside K2 (an agent passport gets
  owner_only, exit 3):
    k2 publish frame <name> --allow https://your.site    add a site (max 8)
    k2 publish frame <name> --none                       refuse every frame
    k2 publish frame <name> --reset                      back to the default
  https only (http:// only for localhost dev); no wildcards, no paths.
  Never allow *.k2.dev: other customers live there. A running App
  restarts with the new policy. k2 publish ps shows it (FRAME column).

--cmd APPS (your own server)
  K2 passes your server's bytes through untouched, so it cannot add the
  header. k2 publish ps prints a warning when GET / is HTML without
  frame-ancestors or X-Frame-Options. Add the header yourself:

  Next.js (next.config.js):
    async headers() {
      return [{ source: '/:path*', headers: [
        { key: 'Content-Security-Policy', value: "frame-ancestors 'self'" },
        { key: 'X-Frame-Options', value: 'SAMEORIGIN' },
      ] }]
    }
  Express:
    app.use((req, res, next) => {
      res.set('Content-Security-Policy', "frame-ancestors 'self'")
      res.set('X-Frame-Options', 'SAMEORIGIN')
      next()
    })
  Plain Node (http.createServer handler):
    res.setHeader('Content-Security-Policy', "frame-ancestors 'self'")
    res.setHeader('X-Frame-Options', 'SAMEORIGIN')

  To let a partner frame it, list it: frame-ancestors 'self' https://your.site
  (and drop X-Frame-Options, which cannot name other sites).
  If your App already sets a Content-Security-Policy, add frame-ancestors
  to that same header; two CSP headers both apply.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_llm_tokens() {
    cat <<'EOF'
k2 study llm-tokens — LLM tokens: subscriptions and API tokens

WHAT
  A token is what a chat spends: a SUBSCRIPTION (a Claude, Codex or Grok
  account and plan, signed in with the tool's own sign-in) or an API
  TOKEN (a provider key, billed per token).
  Each tool has ONE server default token per server. A subscription that
  is the server default lives where the tool always keeps it (Claude's
  keychain item or ~/.claude/.credentials.json, ~/.codex/auth.json,
  ~/.grok/auth.json). Every chat set to Server default uses it. Your
  other tokens wait on the server: ~/.k2/llm-accounts/<tool>/<id>/,
  folder 0700, file 0600. K2 keeps labels, state and usage in its
  database, never a token.

SERVER DEFAULT
  Changing the server default affects every chat that uses Server
  default on this server. Running sessions pick up the new token;
  conversations stay where they are. K2 first saves the outgoing
  subscription back to its folder (the tool may have refreshed it),
  then puts the chosen one in place.
  No default: when an API token is the server default, Admin or Owner can
  clear it (k2 llm tokens clear-default <tool>, or "Stop using as
  default" in Settings → Language Models → Subs / Tokens), or remove it
  with --clear-default. A subscription signed in
  to the tool itself stays the default (switch to another token first).
  When a session hits its limit, "Use next token" is one click on
  Settings → Language Models → Subs / Tokens (k2 llm tokens next <tool>).
  K2 never rotates tokens on its own.

ADDING A SUBSCRIPTION
  Settings → Language Models → Subs / Tokens → Add a token. K2 runs the tool's own
  sign-in in a terminal it owns: Open sign-in page, Copy code, Paste
  code. This works from the web app and a phone against a headless box.
  Over SSH: k2 llm tokens add <tool> <label> [--device]; paste the code
  at the prompt (read from stdin, never an argument).
  The sign-in goes into its own folder, so the server default is not
  touched. If a tool can't do that, K2 signs in through the tool's live
  store, saves the new subscription, and puts the previous one back; a
  banner says the tool is switched for every session while that runs.

FRESH TOKENS
  The tool refreshes the server default subscription itself. K2 never
  refreshes the server default one. K2 refreshes the other subscriptions
  (once a day, and right before a switch). Under air-gap: no sign-in, no
  refresh, no usage call.

A TOKEN FOR ONE WORKSPACE OR CHAT
  The Token picker (workspace settings → Agent → LLM tokens, the chat
  header of the pinned chat or any agent chat tab, or right-click a chat
  tab → Token) says "Use this token for this workspace / this chat". Which
  token a chat uses: its own token > its workspace's token > Server
  default. A chat's own token sticks to that chat: it applies every time
  the chat starts or is revived (refresh, reopened from history in any
  tab, restored after a restart). It is kept under the chat's key and its
  conversation id (conversation:<id>); Codex's id is added once K2 finds
  it. Without its own token, a resumed chat keeps the token it started on;
  a workspace token never moves an existing chat.
  A subscription set for a workspace or chat is never also the server
  default and is skipped by "Use next token" (one subscription is never
  live in two places, since sign-ins rotate); set those back to Server
  default first. "Use next token" moves only chats on Server default; a
  chat with its own token at its limit: pick another token for it.
  A chat on its own Claude subscription keeps its Claude history with
  that subscription; moving a chat to another token copies its Claude or
  Grok history into that token's home before it resumes (never moved or
  deleted). Codex subscriptions share conversations with the normal Codex
  home.
  API tokens (k2 llm tokens add-key <tool> <label>, read from stdin) are
  billed per token, go only into sessions that use them, and are never
  shown again. Gemini takes API tokens only.
  CLI: k2 llm tokens pin --workspace <ws>|--session <key> <tool> <label>;
  unpin goes back to Server default.

WHICH TOKEN ONE CHAT IS ON
  k2 llm tokens for <agent> (a workspace's pinned chat, or a sidecar
  ws/2) shows the token that chat runs on and why: set for the chat, set
  for its workspace, the token its conversation started on, or the
  server default. A sidecar with no token of its own uses its
  workspace's. A running chat shows the token it started on; a change
  shows "from its next start". The usage percent is the same number the
  top-bar chip shows for that token. --refresh re-reads it (at most once
  a minute per token).

WHO
  Owner, Admin and Member users change tokens on Settings → Language
  Models → Subs / Tokens. Agents and K2 terminals are read-only: list,
  status, usage, pins, for. Change tokens on Settings → Language Models →
  Subs / Tokens, or run k2 from a terminal outside K2 (otherwise exit 3,
  owner_only). No route ever returns a token.

ISOLATION
  All agents run as the same Unix user as the daemon, so any agent with
  a shell can read the token folders. Tokens decide which subscription
  or key the server bills to; this is an organizing feature, not a
  security boundary.

IN THE K2 VAULT (0.46.6, opt-in per server)
  k2 llm tokens storage            where saved tokens are kept, and for
                                   each: file yes/no, vault active /
                                   disabled / none, in step yes/no
  k2 llm tokens storage vault      seal every saved token into the K2
                                   vault (Admin or Owner; audited)
  k2 llm tokens storage files      back to files only (Admin or Owner)
  With vault, each saved token (subscriptions and API tokens, removed
  ones kept disabled) is also sealed in the vault's daemon-only system
  area as llm-login.<tool>.<id>: nobody can reveal it and it is never
  copied to another server. The plain files stay in place in 0.46.6, so
  switching back is instant and loses nothing. If a tool's CLI renews a
  saved token in its folder, the newer file is sealed within minutes.
  The server default stays where each tool reads it.
  If the vault isn't open (a locked keychain), the server default and
  every file keep working; a token only the vault holds answers
  vault_unavailable (503) until it opens. K2 tries again after 1, 2,
  4 … up to 30 minutes; a cancelled Keychain prompt stops the retries
  (a notice on your Thread) until `k2 vault open` or the next start.
  K2_LLM_ACCOUNTS_STORAGE=files|vault in the daemon's env wins over the
  setting.

MOVING A BOX
  Clone and migration bundles never carry the tokens. Sign each
  subscription in again on the new box and add API tokens again.

NEEDS SIGN-IN
  needs_login means the tool or K2 could not refresh that subscription.
  Click Sign in again on Settings → Language Models → Subs / Tokens (or k2 llm tokens
  login <tool> <label>). For the server default, run the tool's own /login, or switch.

`k2 llm accounts` is an alias of `k2 llm tokens`; `k2 study llm-accounts`
is an alias of this page.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

