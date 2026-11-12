# ── k2 compute — run commands on your compute nodes ───────────────────
# (prd-k2-compute-nodes-v1 §15.2). `compute` is an `id` tool and locked:
# inside a K2 terminal the request carries this session's passport, so an
# agent only ever sees its own workspace and needs "Allow compute nodes"
# plus a grant. Owner verbs (node add/confirm/remove, grant, local pause…)
# refuse a passport with `owner_only`: run them outside K2.
# Agent verbs ride the per-cell socket when present; owner verbs go TCP.

_compute_usage() {
    cat <<'EOF'
k2 compute — run commands on your own machines (compute nodes)

Agents:
  k2 compute nodes [--json]                  nodes granted to this workspace. The
                 second column is the platform (ubuntu24.04-x86_64, arch-x86_64,
                 macos15.5-arm64; a k2-node older than 0.46.9: linux-x86_64);
                 "reboot pending" ends the row when a kernel update waits.
                 --json: a host object per node (distro, distroVersion,
                 pkgManager, kernel, libc, gpu, rebootRequired, line)
  k2 compute status [--json]                 every node at a glance: state, waits
                 (foreign locks), run/queue and the jobs, free CPU/mem, load,
                 disk free vs floor; plus this controller (disk, Time Machine
                 local snapshots). Hand-run lock files, cache sizes and pinned
                 long-runners need node reporting, which nodes don't send yet.
                 (k2 compute status <job> is get)
  k2 compute info <node> [--json]
                 adds a host: line (Ubuntu 24.04.1 LTS · apt · kernel … · glibc
                 2.39 · x86_64 · no GPU) and a packages: line (manager · the
                 policy for this server · open requests). A node with a GPU says
                 which and whether you may use it (GPU <model> (amdgpu, 96G
                 unified) · GPU jobs: yes); --json: host.gpus[] (jobs: true =
                 open to this server)
  k2 compute allow-nodes [--workspace WS] [--json]
                 this workspace's node list. With one, its jobs go ONLY to those
                 nodes: a node named off it is refused (node_not_allowed, exit 3),
                 --any and every move or retry stay on it, however the job ships
                 (--sha, --dirty, --with, --shards). No list = any granted node
  k2 compute allow-nodes <node>[,<node>…] | --add N | --remove N | --clear [--workspace WS]
                 set it. An agent may only narrow its own (a first list, --remove);
                 adding a node or --clear is a person's: the owner or an Admin,
                 from a terminal outside K2 (an agent gets allow_nodes_needs_person,
                 exit 3, and asks on a card). Nodes by name; a node removed and
                 enrolled again is a new node, off the list until added
  k2 compute git-access [--json] | grant --github <owner>/<repo>[,…] | revoke … | setup …
                 let this workspace's jobs read private GitHub repos: git clone
                 and fetch just work, with short-lived read-only tokens, and the
                 node fetches the job's source itself (no relay cap). No --secret,
                 so jobs keep --cache and warm slots. A person grants (an agent
                 gets git_access_needs_person, exit 3, and asks on a card; with
                 no GitHub credential yet, git_access_needs_setup and the
                 setup card first); k2 compute git-access --help
  k2 compute run [<node>] [--os linux|macos] [--arch x86_64] [--label k=v]
                 No node: K2 picks one of the nodes granted to you that can
                 start the job now (a warm one if it isn't busier), else the
                 one with the shortest wait, and prints where and why
                 ("placed on …"). Name a node only to pin the job there
                 (its full name, or a start of it that fits only one: k2n12;
                 a start that fits two or more: node_ambiguous, exit 2).
                 It prefers a node that already has the --needs tools.
                 --label k=v picks nodes by a label the node has.
                 [--distro ubuntu|ubuntu@24.04|debian|arch]  only nodes of that
                                distro: its /etc/os-release ID or ID_LIKE (Ubuntu
                                counts as debian), the version on a dot prefix
                                (ubuntu@24 fits 24.04). A skipped node says "of
                                another --distro"
                 A pinned job never moves, even when other nodes are idle:
                 leave the node off unless the job needs that machine. When
                 the named node can't start it now and another could, run
                 prints a hint (k2 compute move <id> --auto releases it).
                 [--avoid <node>]...  K2 never picks that node for this job: not
                                at submit, not when it moves, not for a retry.
                                Repeatable; a name or a start of one. Not with a
                                named node (avoid_with_pinned_node, exit 2). When
                                every node that fits is avoided: avoid_leaves_no_node,
                                exit 3, nothing queued. A name that isn't one of
                                your nodes is fine (it could never be picked): one
                                note, "--avoid k2n9: not one of your nodes;
                                nothing to avoid", so a standing list works as flags
                 [--only <node>]...  K2 picks only among these nodes (repeatable; a
                                name or a start of one): keep private work on your
                                own nodes without listing every node to --avoid.
                                Not with a named node. A node left out shows as
                                "off your --only list" in the placed-on line
                 [--preset NAME] settings from .k2/compute.toml, a TOML file in
                                the folder or git worktree you run from (else the
                                --repo folder, else the workspace folder; .k2/ isn't
                                committed, so it stays on this machine). [defaults]
                                apply to every run, [presets.NAME] on top, and a
                                flag beats both. --needs and --env merge: a flag
                                replaces the file's entry for the same tool or name
                                (--needs rust@stable over rust@1.99.1) and the
                                file's others stay. Keys: os arch distro mem
                                cpus needs env timeout stall_timeout tier cache.
                                --plan shows what came from where
                 [--now]        start at once or fail (exit 3 not_now, naming the
                                shortest wait); nothing is queued
                 [--same-node-as JOB]  run on the node that ran one of your jobs and
                                wait for it there however long (a resume that needs
                                that node's files)
                 [--tag NAME]   a name you give the job (e.g. fuzz:p3; 1 to 64
                                characters, each one of A-Z a-z 0-9 . _ : / -):
                                k2 compute jobs --tag NAME, cancel --all-mine --tag
                                NAME. (--label k=v is different: it picks nodes)
                 [--detach] [--sha REV] [--no-source]
                                The job's checkout has your tags that point into
                                its history (an unpushed local one too), so git
                                describe --tags and --baseline v0.9.2 work there
                 [--dirty]      ship your uncommitted changes too (no commit needed;
                                HEAD + the diff + untracked, not-ignored files;
                                never .k2/), from the checkout the job ships
                                (yours, --repo's, or the worktree you're in);
                                never for --with repos
                 [--repo DIR]   ship a git repo nested in the workspace (or a worktree
                                of one) instead of the workspace's own: a multi-repo
                                workspace whose root isn't a repo. DIR is relative to
                                the workspace folder. Run from inside one (or from
                                a worktree of one) and it's found for you. Same jail:
                                inside the workspace, no '..', no link out, never .k2/
                 [--with DIR[@SHA]]...  ship sibling repos too (up to 8): DIR is a repo
                                or a worktree of one, relative to the workspace
                                folder, at its HEAD (a worktree's own, unpushed
                                commits included) or SHA. Commits only: commit a
                                sibling's changes first (--dirty is the main repo's).
                                Every repo, the main one too, lands where its main
                                checkout sits in the workspace, worktree or not:
                                --with wt-base-y (a worktree of k2-multiplayer-base)
                                lands at k2-multiplayer-base, so ../k2-multiplayer-base
                                path deps work. The job starts in the main repo
                 [--cache NAME] a folder on the node that outlives the job, per
                                workspace and NAME: $K2_JOB_CACHE (fuzz corpora,
                                installs, a cargo target). With --no-source too.
                                Evicted when idle and the node needs the disk;
                                never with --secret. k2 compute cache lists them.
                                With no node named, K2 picks the node your last
                                job with this cache ran on; when that node is busy
                                it waits there if the wait is under 15 min, else
                                it runs elsewhere with an empty cache and says so
                                (--same-node-as JOB always waits)
                 --mem 16G      always pass it: the memory the job needs (a cargo
                                build, check or test of a big workspace like K2:
                                24G; a smaller crate: 8G; a script: 1-2G). Left
                                out, K2 reserves 4G and prints a note. A job
                                that uses more than its --mem is stopped
                                (failed/mem_cap). It starts only when its --mem
                                fits beside what runs (k2 compute nodes shows
                                each node's free memory and cap)
                 [--env K=V]... [--cpus N]
                 [--secret NAME[=ENVNAME]]...  a vault secret as env, sealed to the
                                node at spawn; names only in the plan (k2 study compute)
                 [--timeout D (≤ the grant's max, default 2h; k2 compute nodes shows it)]
                 [--exclusive --why "<one line>"]  the whole node to itself; needs
                                a reason (else exit 2) and a grant with --ci, or the
                                owner or an Admin (else exit 3 exclusive_needs_grant)
                 [--gpu [any|amd|intel]]  one GPU of the node, to this job alone
                                ($K2_GPU_RENDER is its /dev/dri/renderD* node; no
                                display). Only nodes whose owner turned GPU jobs on
                                for this server, with a grant that says --gpu (else
                                no_gpu_node or gpu_needs_grant, exit 3; a busy GPU:
                                it waits, gpu_busy). Bare --gpu = any. k2 study
                                compute: GPU JOBS
                 [--gpu-mem 4G]  only with --gpu (alone: exit 2): the job's GPU
                                memory budget. On an APU or iGPU (GPU memory is
                                system RAM, outside --mem) it is reserved beside
                                --mem, 2G when left out, and a job whose own GPU
                                memory goes over it by more than 10% is stopped
                                (failed/gpu_mem_cap; the 2G default too: pass more
                                for bigger textures). On a discrete GPU: nothing
                                reserved, watched only when given. Needs k2-node
                                0.46.11 on the node (else gpu_mem_needs_update,
                                exit 3)
                 [--receipt-v2]  the node signs receipt v2 (how and where the job
                                ran, and its signed self-statement; k2 compute receipt
                                shows both); only nodes on protocol 9 take it
                                (preview: k2 settings set ciPreview true)
                 [--retry-interrupted|--no-retry] [--raw] [--workspace WS]
                 [--wait [--failures] [--max-wait 9m] [--quiet]]
                                print the job id, then stream its output and exit
                                with its code (--quiet: no output, only the exit).
                                --max-wait counts from submit, queue time included;
                                when it passes: exit 3 still_running and the line to
                                resume with (k2 compute wait <id> --follow). K2 has
                                no cap: 9m is an example for a shell tool that is
                                killed at ~10 minutes (your shell's limit, not K2's)
                 [--artifact GLOB]... [--json]
                                GLOB is relative to the job's working directory
                                (the repo root, or where you ran k2 from inside it),
                                not to a folder you cd into inside the command
                 [--queue-timeout 2h|off] [--retries N] [--stall-timeout 30m|off]
                                queue timeout: 2h attached or --wait, 24h --detach,
                                48h --tier batch. Stall: a job that prints nothing
                                that long ends; use --stall-timeout off for a job
                                that goes quiet (a soak)
                 [--shards N]   N jobs, {shard}/{shards} in the command, each on its
                                own node when it can (no node named)
                 Which line (queue order only; never stops a running job; k2 study compute: TIERS)?
                      (no flag)  Standard: a normal build or test
                      --priority --why "<one line>"
                                 the critical path (one result blocks others now). Priority and
                                 Standard take turns, so Standard never starves. --why is required
                                 and shows in the queue; at most 2 Priority jobs queued or running
                                 per workspace (more run Standard, with a note)
                      --tier batch
                                 idle time (soaks, overnight sweeps): starts only when the node is
                                 idle (any hour, not a clock window), one at a time; while it waits
                                 it holds nobody up; once started it runs to its end. Always
                                 detached (no --wait); 48h queue timeout; --timeout is still capped
                                 by the grant's max per job (ask the owner for --max-time)
                      --tier ci  release and CI gates: next free slot (needs a grant with --ci)
                 [--tier standard|priority|batch|ci]   the same as the flags above
                 [--gate]       = --tier ci (kept for scripts)   [--critical] = --priority
                 [--supersede KEY]   cancel your older QUEUED jobs with the same KEY (never a running one)
                 [--submitter NAME]  your own turn in the queue (builders sharing a session: pass it),
                                the per-submitter cap, and the views (not security)
                 [--plan]     dry run: the node it would pick and why, queue, warm
                                slot, estimate from earlier runs, grant left, each
                                need ready or to install there, the next nodes,
                                the limits that end it, the settings and where each
                                came from (JSON)
                 [--needs node@22,python@3.12,rust@stable,cargo-nextest,cmake@3,sys:pkg-config]
                 (tools: node python go java bun deno zig cmake ninja protoc uv pnpm,
                  rust@<ch> (clippy and rustfmt come with it on a 0.46.9 node;
                  rust@stable+minimal leaves them out; more with +:
                  rust@stable+rust-src+llvm-tools; rust-analyzer miri),
                  cargo-nextest, or aqua:owner/repo@v;
                  system packages (Linux): sys:<debian-name> (sys:libssl-dev; K2
                  maps it to each distro's own name), sys:<apt|pacman|dnf|zypper>:<name>
                  (that manager's nodes only), or profile:<name>[@N], a bundle
                  K2 ships (build-base, tauri-linux, rust-gui-ci, k2-workspace,
                  gpu-headless).
                  A package the node's owner hasn't approved: package_not_approved,
                  exit 3, and a request for it is filed (k2 compute node packages);
                  anything else is refused: tool_unsupported, exit 2)
                 -- <command…>
  k2 compute test [<node>] [--shards N] [run flags] [-- <nextest args>]
                 a cargo-nextest suite split across nodes (nextest --partition),
                 JUnit kept and parsed, one combined result; --profile P to use
                 the repo's own nextest profile, --runner to replace cargo nextest run
                 (--preset NAME is the .k2/compute.toml preset, as for run)
  k2 compute prewarm [--preset NAME | --needs …] [--os …] [--distro …] [--arch …] [--node N]...
                 [--plan] [--wait] [--json]
                 install the tools ahead of time: one idle-time (Batch) job on
                 each node granted to you that matches (--os/--arch/--node, or
                 the preset's os/arch; none: every granted node) and lacks one
                 of them; a node that has them all is only reported. The jobs
                 are detached and wait for an idle node; --wait waits for them
                 (exit as k2 compute wait). System packages their owners must
                 approve go out as one request for every node (--plan: would ask)
  k2 compute node packages <node> [--json]   the node's package manager, the
                 policy for this server, approved and installed names, requests
  k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why "…" [--job ID]
                 ask the node's owner to install them (Debian names, native
                 pacman:… names, or profile:…); you hear back in a [compute] message
  k2 compute node packages requests [--node N] [--state S] [--json]
                 your requests and their state (the owner: every request)
  k2 compute jobs [--node N] [--mine] [--state S] [--tier ci|priority|standard|batch] [--tag T] [--limit N] [--workspace WS] [--json]
                 TAG and WHO columns: the job's --tag, and who sent it
                 ("(owner token)" when a shell had no passport: it ran as the owner)
                 From a git worktree inside the workspace (.claude/worktrees/x) the
                 workspace is found for you, and run takes HEAD and --dirty from
                 that worktree; from anywhere else pass --workspace <handle>.
  k2 compute get <job> [--full] [--json]     a long command shows its first 120 characters
                                             (--full: all of it). k2 compute status <job>
                                             is the same.
                                             --json: the result (exit, duration, node,
                                             artifacts + sha256, tests, receipt id,
                                             wait while unfinished: where, position,
                                             of, reason, words, etaSecs, etaBasis,
                                             holdsQueue, since; k2 study compute RESULTS)
  k2 compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay]
                                             copy kept artifacts here (all, or NAMEs);
                                             --over-relay: files over 100 MB through
                                             the relay too (up to 500 MB)
  k2 compute wait <job>... [--any] [--follow] [--failures] [--max-wait 9m] [--quiet] [--json]
                 block until the jobs end, no log text; exit = the job's exit
                 code (first failing job for several); 3 = a job was cancelled,
                 failed or timed out, OR --max-wait passed first (still_running),
                 OR a job's own command exited 3: the stderr JSON says which.
                 --any: return when the FIRST of them ends (exit = its code)
                 and name the ones still running.
                 --max-wait counts from this call and has no K2 cap; 9m suits
                 a shell tool killed at ~10 minutes: call it again while it
                 exits 3 still_running. A command that itself exited 3 also
                 gives 3: the stderr JSON (or --json) says which.
                 --follow (one job): stream its output too (resume a run --wait
                 whose shell timed out). While queued it prints where it waits,
                 "N of M" and why.
                 run --wait = submit, print the id, then stream (no [compute]
                 message while it waits; a --detach job always gets one).
  k2 compute attach <job> [--since SEQ] [--raw]
  k2 compute logs <job> [--tail N] [--failures] [--raw]
                                             --failures: failing tests first, then log lines
  k2 compute cancel <job> [--force]
                 a queued job is cancelled at once; one that already started
                 (or was just assigned) needs --force (else job_started, exit 3).
                 On a shared node, a job in the node's queue is asked to cancel
                 only if it hasn't started: it ends cancelled, or job_started
  k2 compute cancel <job> --if-queued
                 cancel only if it is still queued; if it already started (or
                 ended), say so, leave it alone and exit 0 (for scripts)
  k2 compute events [--follow] [--mine] [--tag T] [--since CURSOR] [--limit N] [--max-wait D] [--json]
                 events of every job of this workspace (--mine: only the jobs
                 this session sent), instead of polling logs: queued, started,
                 finished (state, exit, reason), requeued, moved. No --follow:
                 the newest N (default 20). --follow: only events after now
                 (or after --since CURSOR), as they happen; it never exits on
                 its own unless --max-wait passes (then exit 0 and the line to
                 continue). --json: one JSON object per line (NDJSON), each
                 with its cursor. (The audit log moved to events --audit.)
  k2 compute cancel --all-mine [--tag T] [--state queued|running|any] [--node N] [--force]
                 every job THIS session sent (this terminal's passport; an
                 earlier session's jobs need --workspace-all) that matches:
                 queued ones by default; started ones only with --force
                 (else listed as skipped). --workspace-all instead of
                 --all-mine: every job of the workspace (other sessions'
                 too). Prints how many and their ids
  k2 compute cache [list] [--node N] [--workspace WS] [--name NAME] [--json]
  k2 compute cache clear (--workspace WS | --all) [--name NAME] [--node N]
                                (an agent: its own workspace's caches only,
                                no --workspace or --all needed)
                 this server's --cache folders on its nodes: size, last use,
                 busy (a running job holds it; never cleared). Owner/admin,
                 or an agent allowed to manage compute nodes
  k2 compute queue <node> [--json]
                 the node's whole queue, with WHY each job waits: on a shared
                 node, other servers' jobs show only as "another server" with
                 tier, class, size and ETA
  k2 compute retier <job> --tier standard|priority|batch|ci [--why "…"] [--json]
                 move a QUEUED job to another line (--priority needs --why);
                 your workspace's jobs, or any job for the owner / an Admin
  k2 compute move <job>... --auto [--avoid <node>]... [--os O|any] [--arch A|any] [--label k=v] [--json]
                 let K2 move a QUEUED job that was pinned to a named node: it
                 becomes auto-placed (as if no node had been named) and moves
                 within ~5 s to a node that can start it now; if none can, it
                 waits where it is. It keeps the OS and arch of its node
                 unless --os/--arch say otherwise (any = either). --avoid
                 keeps it off a node. No cancel, no new job id
  k2 compute move <job>... --to <node> [--json]
                 move a QUEUED job to that node and pin it there (the same
                 checks as naming the node in run)
  k2 compute move --all-mine|--workspace-all [--tag T] [--node N] (--auto … | --to <node>)
                 every QUEUED job this session (or the workspace) sent that
                 matches; e.g. move --all-mine --node k2n12 --auto.
                 A started job is never moved (job_started, exit 3)
  k2 compute retry <job>
  k2 compute receipt <job>
Owner / admin (refused for agents and inside K2 terminals, except where
"Allow managing compute nodes" is on for that agent: see k2 compute manage):
  k2 compute preview on|off|status             turn the preview on for this server (owner)
  k2 compute placement v1|v2|status           how K2 ranks nodes when no node is
                   named (owner): v2 (default) = load first, a warm slot worth
                   one load step, bursts spread; v1 = the 0.46.5 order
  k2 compute node add <name> [--url URL] [--json]   mint a 10-minute enroll code (CODE.FP);
                   --json: {code, expiresAt, name, controller} for scripts
  k2 compute node confirm <name> <code>        the 6 digits the node shows
  k2 compute node remove <name>                k2 compute node rename <old> <new>
  k2 compute node retire <name>                drain, then remove once its jobs end
  k2 compute node pause|drain|resume <name>    this server stops/starts assigning
  k2 compute node route add|rm <name> <url>    LAN or tailnet URL the node may dial
Sharing a node with other K2 servers (the node's owner server; 0.46.3):
  k2 compute node servers <node> [--json]      every server on it: slot, state,
                   trust, share, today's use, end date, pending SAS
  k2 compute node servers <node> add --url URL --code CODE.FP [--trust own|team]
                   [--for 7d|none] [--weight N] [--max-running N] [--max-queued N]
                   [--cpu-hours N] [--max-time 2h] [--cpus N] [--mem 8G]
                   [--disk 40G] [--network on|no-lan|none] [--gate] [--exclusive]
                   CODE.FP is what `k2 compute node add <node>` printed on the
                   NEW server. Held for your approval (Touch ID / vault
                   password); the new server's owner then confirms the SAS.
                   Needs the node's remote-admin level `full`; on a node set
                   to `policy`, add servers at the machine instead:
                   sudo k2-node servers add --url URL --code - < code-file
                   (--code - here too: the code on stdin, never in argv)
  k2 compute node servers <node> approve|deny <request>   (= node admin approve/deny)
  k2 compute node servers <node> limits <server> [--weight N] key=value…
                   narrowing at once, widening (a higher weight too) waits
                   for approval (max_running, max_queued, cpu_hours_day,
                   max_job_secs, max_cpus, max_mem_gb, disk_gb, weight,
                   network, top_class, gate, exclusive; for=7d|none;
                   priority=true|false: its --tier priority counts here, a
                   0.46.5 node only; true is a widening). Every server
                   starts at weight 1, the owner too; <server> may be
                   slot:0 (this server) for its weight
  k2 compute node servers <node> pause|resume <server>
  k2 compute node servers <node> revoke <server> [--now] [--wipe]   (owner)
  k2 compute node servers <node> owner <server> [--old keep|revoke]
                   [--caches move|wipe] [--now] [--reason TEXT]   (owner, 0.46.4)
                   make another of your own servers on the node its owner
                   (active, confirmed, online). Held for your approval; the
                   node drains (--now stops running jobs), the two swap
                   slots and root re-pins to the new owner. This server
                   stays as one of your servers, with a new server's
                   default share (--old revoke: it leaves, its queued jobs
                   end share_revoked, its files go after 24 h); caches move
                   with their server (or --caches wipe). Before approving,
                   check the full key on the card against the new owner's
                   controllerFp (k2 compute nodes --json there). Needs
                   remote admin `full`; else at the machine:
                   sudo k2-node servers owner <server> --fp <full key>
  k2 compute node take-owner <node>   (owner, in person; 0.46.4)
                   a node another of your servers made this server's owner:
                   it says so, but stays that server's here until you take
                   it over. It shows the node's full key, the old owner and
                   every compute:<ws>/* secret grant that starts reaching
                   it; you type the node's name, then Touch ID / vault
                   password. An offer alone never makes a node this server's.
CI seats on shared nodes (0.46.10): node rosters, lanes, seats (preview: k2 settings set ciPreview true).
Off, every verb below exits 3 (ci_preview_off). Agents with
"operate compute nodes" file and read; a person (owner or Admin, from a
terminal outside K2, Touch ID / vault password) decides every change:
  k2 compute roster [show] [--json]   this server's signed node roster (seq,
                   keys, hosts), its nodes' self-statements, waiting changes
  k2 compute roster key               mint/show the node-roster key's
                   fingerprint (a person; clients compare it on this screen)
  k2 compute roster set <node> [--host H] [--provider P] [--site S]
                   [--admin-reach a,b] [--attest "what you checked"]
                   list the node's current key as active. A key whose
                   statement says taint: unknown needs --attest (the owner
                   vouches it never ran a job without the job wall) or a
                   re-key; a tainted key never goes on (node_key_tainted)
  k2 compute roster retire <node>  |  revoke <node> --reason compromised|key_lost|other
                   compromised voids every receipt that key ever signed
  k2 compute roster publish [--resign]   hand the newest roster to the nodes
                   (they relay it to their servers) and the seated clients;
                   --resign signs an unchanged roster again (seq + 1)
  k2 compute lanes [--json]           approved lane templates (ci-standard ships)
  k2 compute lanes set <name> [--from ci-standard] [key=value …]   held
  k2 compute seats [--json]           seats this server offered or holds
  k2 compute seats request <owner-host> [--lane ci-standard] [--seat ID]
                   [--secrets] [--why "…"]   (client) ask a paired node
                   owner for a seat. Held for your person: approving pins that
                   owner's node-roster key (compare its fingerprint with
                   `k2 compute roster key` on the owner's screen). It never
                   sends vault secrets: --secrets only asks; a person agrees
                   to that separately (k2 compute owners consent <owner> on)
  k2 compute seats offer <request> --nodes a,b,c   (owner) name a client's
                   seat nodes; the owner approves: one card signs the seat and
                   sets up every node (lanes file + the client as a server)
  k2 compute seats suspend|resume|end <seat> [--why "…"]   (owner, in person:
                   a fresh confirm) the client hears it at once (no lane
                   secret on a seat that isn't active); every seat node gets
                   its lanes file without the seat and the client's server
                   paused, resumed or revoked (end: running jobs stop)
  k2 compute trust [requests] [--all] [--json]  |  approve <id>  |  deny <id> [--why]
                   |  withdraw <id>   approve/deny: a person, in person
  k2 compute owners [--json]          node owners this server pinned (seq,
                   secrets consent) and this server's own roster key
  k2 compute owners consent <owner> on|off   lane secrets (0.46.10): on = a
                   person here agrees that this server's vault secrets may go to
                   <owner>'s nodes for jobs on your seat of a lane of theirs
                   that allows secrets (their root can read them; other tenants
                   can't). The seat request card never gives it: it is its
                   own yes. on takes a fresh confirm, bound to that
                   owner's pinned roster key; off works at once (an agent may
                   narrow). Then: k2 vault grant NAME --to
                   compute:<ws>/lane:<owner>/<lane> --as env
  k2 compute grant <node> --workspace WS [--max-time 2h] [--max-disk 60G]
                   [--max-parallel 1] [--max-queued 10] [--ci|--no-ci] [--gpu|--no-gpu]
                   [--submitter-max-running N]   at most N running jobs per
                   submitter on that node (0 = off, the default; CI exempt).
                   Changing --max-parallel or this re-sends the workspace's
                   jobs already waiting in a shared node's queue
                   --ci: this workspace may send --tier ci (it jumps every queue;
                   owner or an Admin login only; absent keeps what it was)
                   --gpu: this workspace may send --gpu jobs (owner or an Admin
                   login only; an agent asks: k2 permission request compute.gpu
                   node=<node> --why "…"). The node's own owner must also turn
                   GPU jobs on for this server at that machine
                   (sudo k2-node admin gpu on). It doesn't give --exclusive
  k2 compute revoke <node> --workspace WS
  k2 compute grants [--node N] [--workspace WS] [--json]
                   --workspace WS: that workspace's grants. The owner, an
                   Admin or a person in a K2 shell tab may name any
                   workspace; without it, the owner and an Admin see every
                   workspace's, a shell tab sees its own workspace's. An
                   agent sees only its own workspace's grants; naming
                   another is refused (forbidden, exit 3)
  k2 compute access on|off --workspace WS
                   the Agent tab's "Allow compute nodes" (Settings → Workspaces →
                   <agent> → Agent): on = a grant on every machine switched on,
                   default limits (and ones added later); off removes them all.
                   The owner or an admin sets it. An agent can't; with
                   permission cards on, it asks for its own workspace with
                   k2 permission request compute.use --why "…" (no workspace:
                   always its own). Allowed, it is on: run your command again
  k2 compute usage [--days N]    k2 compute events --audit [--limit N]
                   the audit log (plain `events` shows job events since 0.46.7)
Node admin (agents need "Allow this agent to operate compute nodes"):
  k2 compute node health <node> [--json]       uptime, disk, RAM, FileVault, update state
  k2 compute node policy <node> show
  k2 compute node policy <node> set key=value... [--reason "…"]
                   narrowing applies at once; widening waits for the owner (exit 2)
  k2 compute node update <node> [--canary] [--now]   send this server's k2-node
                   build to one node, which installs it (--canary: this node
                   goes first in every later --all)
  k2 compute node update --all [--batch N] [--now]   a rollout to every node this
                   server owns (not nodes it only uses). Default: all at once
                   (--batch 0 is the same); --batch N keeps N updating at a
                   time. Offline nodes get it when they come back. Running it
                   again while a rollout to that version is open shows that one.
  k2 compute node rollout [show|pause|resume|stop] [--id ID] [--json]
                   the latest rollout and each node's state: pending (not sent
                   yet), offered (installing), healthy, rolled_back (the new
                   build failed its 120 s health check and the node went back
                   to its old one by itself: k2 compute node health <node>
                   says why), refused / failed (with the node's reason),
                   timed_out (no answer in 10 min), skipped (with why).
                   "settled" = any state but pending or offered. The first
                   rolled_back pauses the rollout: nodes already sent finish,
                   no more are sent. resume sends to the rest (never again to
                   a node that rolled back); stop ends it. explicit = started
                   by a person or agent; auto = this server updated itself.
  k2 compute node rollback <node> [--now]      back to the node's own previous build
                   Running jobs: a node that runs k2-node 0.46.9 or newer
                   before the update keeps them running (the new k2-node
                   picks them up; new jobs start on the new build). A node on
                   an older k2-node still stops them, or, when other servers
                   share the node, waits for them first (up to 30 min, then it
                   updates anyway; --now skips the wait). Linux nodes without
                   the job wall also still wait (turn it on: k2 compute node
                   wall <node>). Nodes on 0.46.3 or older need a person at the
                   machine once: their updater can't install a new build.
                   Install the new k2-node there (sudo k2-node setup); a later
                   rollout then reaches them.
  k2 compute node wall <node> [--no-restart] [--reason "…"]
                   Linux: turn the job wall on (jobs run as k2job, walled off
                   from the node's key, /home and /root). At once on 0.46.4+
                   nodes at remote admin policy; k2-node restarts to use it once
                   its running jobs end, only if this call changed the wall's
                   files; after 30 min with jobs still running it gives up and
                   restarts nothing (--no-restart: at the next restart). An
                   older node, a node whose owner ran setup --no-job-wall, or
                   one with its own job user holds it for the owner (exit 2)
  k2 compute node admin propose <node> <verb> [key=value…] --reason "…"
                   verbs today: unit.restart unit=k2-node [now=true], policy.set key=value,
                   control.pause|drain|resume, gc, jobs.kill job_id=…,
                   wall.enable [restart=true] (Linux: jobs run as k2job; or k2 compute node wall),
                   homes.private (chmod 700 the homes others can read)
  k2 compute node admin propose <node> --plan plan.json --reason "…"
                   held for the owner: exit 2 with the request id
  k2 compute node admin list [--node N] [--state open|pending|all] [--json]
  k2 compute node admin withdraw <id>
                   Ask the owner to approve a held request in their chat
                   (permission cards on; an agent that may operate or manage
                   compute nodes; its own requests only, another's is refused):
                   the hold ends with that line, or run
                   k2 permission request compute.admin-approve request=<id> --why "…"
                   Allow on the card approves and sends it exactly as Settings
                   does (don't run your command again); Deny denies it. It stays
                   in Settings too: the first answer decides it, and a request
                   already decided can't be asked for
Node admin, owner only (refused for agents and inside K2 terminals):
  k2 compute node admin approve <id>           Touch ID / your password, then signed + sent
  k2 compute node admin deny <id> [--why "…"]
  k2 compute node packages approve|deny <request> [--add] [--note "…"]
                   a package request (the card in chat is the main path);
                   --add also puts the names on that server's approved list
  k2 compute node packages policy <node> [<server>] [level=list|profiles|official]
                   [profiles=a,b] [max_install_mb=N] [--reason "…"]
                   what a server (own, or another server's label) may install
                   without asking; no key=value shows it. Narrowing applies at
                   once, widening is held for you
  k2 compute node alerts <node> [--after 10m|off]   offline alert after N minutes
  k2 compute node alerts --to <workspace>|none      where alerts go (an IT agent)
  k2 compute operate on|off --workspace WS
                   the Agent switch "Allow this agent to operate compute nodes"
  k2 compute manage on|off --workspace WS      (owner only)
                   the Agent switch "Allow managing compute nodes": that agent may
                   node add/confirm/rename/pause/drain/resume/retire/route and
                   grant/revoke any workspace (each act in Access Audit). Still
                   yours: preview on|off, node remove, and this switch
This computer (its owner):
  k2 compute local status|pause [--now]|drain|resume
  k2 compute local policy key=value...    (cpu_cap_percent, mem_cap_gb, availability, …)
  k2 compute local enable [--controller URL --enroll CODE.FP --name NAME]
                          [--private-home | --no-private-home]
                          [--remote-admin off|policy|full]   (default policy:
                          K2 keeps this node updated and healthy; full also
                          lets it run root actions, each approved by you)
  k2 compute local remote-admin off|policy|full   change it later (root: one
                          sudo or admin prompt; off = K2 can't update the node).
                          Only the owner: this machine's K2, or the owner
                          signed in over K2 Connect; never an agent
  k2 compute local disable [--yes]   lists the jobs running here first; with
                          jobs running it asks (or needs --yes): they stop now

Never stuck: a job that can't start within --queue-timeout (default 2h
attached or --wait, 24h --detach, 48h --tier batch) fails with queue_timeout, naming the last
thing it waited on; jobs K2 placed (no node named) move off a node
that sleeps or is paused; infrastructure failures (node lost, sync, spawn)
retry --retries times (default 1) with backoff, on another node when K2
placed the job.
A job that prints nothing for --stall-timeout (default 30m) ends as
timeout/stalled. A non-zero exit, a timeout, a stall or a cancel is a
result and never retried.
Exit codes: 0–255 = the command's own exit (attached runs); 2 = bad usage;
3 = infra, with a JSON code on stderr (node_offline, not_granted,
compute_off, queue_full, owner_only, job_started, …); 130 = you detached
with Ctrl-C (the job keeps running). run --json prints one result object
on stdout (logs go to stderr). Jobs see K2_COMPUTE_JOB=<id>, K2_JOB_CPUS,
K2_JOB_PORT_BASE, K2_JOB_PORT_COUNT and K2_NODE_OS, K2_NODE_DISTRO,
K2_NODE_DISTRO_VERSION, K2_NODE_PKG (all set by the node; --env can't set
them; no K2 token or socket). --secret: the job gets a vault
secret as env; only a person grants it (k2 vault grant NAME --to
compute:<ws>/<node> --as env), and the job can print or send what it's
given. Read `k2 study compute` first.
EOF
}

# _compute_send METHOD PATH [payload] [tcp] → `<body>\n<status>` on stdout.
# GET payload = urlencoded query; POST payload = JSON body.
# Is this shell inside a K2 terminal (a k2-ptyd or k2-daemon ancestor)?
# CE-9: an agent there whose K2_* passport vars went missing falls back to
# the owner token silently; the submit says so (a warning, never a refusal).
_compute_in_k2_tree() {
    local pid="$PPID" i=0 line comm
    while [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null && [ "$i" -lt 40 ]; do
        line="$(ps -o ppid=,comm= -p "$pid" 2>/dev/null)" || return 1
        [ -n "$line" ] || return 1
        comm="${line#"${line%%[![:space:]]*}"}"
        comm="${comm#*[[:space:]]}"
        case "$comm" in *k2-ptyd*|*k2-daemon*) return 0 ;; esac
        pid="$(printf '%s' "$line" | awk '{print $1}')"
        i=$((i + 1))
    done
    return 1
}

_compute_send() {
    local method="$1" path="$2" payload="${3:-}" force_tcp="${4:-}"
    local sock out="" _K2_CFG=""
    if [ "$method" = "POST" ] && [ "$path" = "/cli/compute/run" ] && [ -z "${_CP_OWNER_WARNED:-}" ] && ! _compute_in_session && _compute_in_k2_tree; then
        _CP_OWNER_WARNED=1
        echo "[k2 compute] no passport in this shell: this job is submitted as the owner (its jobs show via: owner_token). Keep K2_* set in agent shells." >&2
    fi
    sock="$(_hook_sock)"
    if [ "$force_tcp" != "tcp" ] && [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            [ -n "$payload" ] && _k2_cfg_add data "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 60 -w $'\n%{http_code}' 2>/dev/null)" || out=""
        else
            _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" -H "Content-Type: application/json" \
                -w $'\n%{http_code}' 2>/dev/null)" || out=""
        fi
        if [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            printf '%s' "$out"
            return 0
        fi
    fi
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_add data-urlencode "token=${TOKEN}"
        [ -n "$payload" ] && _k2_cfg_add data "$payload"
        _k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time 60 -w $'\n%{http_code}' 2>/dev/null || printf '\n000'
    else
        _k2_cfg_add url "${BASE_URL}${path}?token=$(urlencode "$TOKEN")"
        _k2_cfg_add data-raw "$payload"
        _k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time "${K2_CP_MAX_TIME:-120}" \
            -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null || printf '\n000'
    fi
}

_compute_split() {
    local raw="${1:-}"
    _CP_STATUS="${raw##*$'\n'}"
    _CP_BODY="${raw%$'\n'*}"
    [ "$_CP_STATUS" = "$raw" ] && _CP_BODY=""
    [ -z "$_CP_STATUS" ] && _CP_STATUS="000"
    # 0.46.4 (field report #11): the compute/App transport never noted the
    # refusal's (or hold's) `permission` object, so an agent refused or held
    # on `k2 app install` was never shown its "Ask with: k2 permission
    # request …" line. The flush prints it only on exit 2 or 3.
    _permission_note "$_CP_BODY"
    return 0
}

# Print the refusal and exit: 2 for a bad request, 3 for everything else
# (infra and permission problems; a test failure is never exit 3).
# 0.46.7 (prd-tauri-core-0467-v1 TC-19): an App refusal (`_K2_REFUSAL_AREA`
# app) whose id is in the registry also carries `next` (the fix) and
# `explain` ("k2 explain <id>"), and, unless the command asked for --json,
# a `fix: <next>` line follows (before K2's own Ask with: line). Exit codes
# are unchanged.
_compute_fail() {
    local status="$1" body="$2" line="" rows=""
    [ "${_K2_REFUSAL_AREA:-}" = app ] && rows="$(_k2_refusals_json 2>/dev/null)"
    if [ -n "$body" ]; then
        line="$(printf '%s' "$body" | K2_RF_ROWS="$rows" K2_RF_JSON="${_K2_ARGS_JSON:-0}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    print("daemon_error: " + raw[:200]); sys.exit(0)
e = v.get("error") if isinstance(v, dict) else None
if isinstance(e, dict):
    out = {"error": e.get("code", "error"), "hint": e.get("hint", "")}
elif isinstance(e, str) and isinstance(v.get("message"), str):
    # The route floor answers {"error": <code>, "message": <text>}.
    out = {"error": e, "hint": v["message"]}
else:
    out = {"error": "error", "hint": str(e)}
row = None
try:
    reg = json.loads(os.environ.get("K2_RF_ROWS") or "null")
except Exception:
    reg = None
if isinstance(reg, dict) and out["error"] != "error":
    code = out["error"]
    row = next((r for r in reg.get("rows", []) if r["id"] == code), None) or next((r for r in reg.get("rows", []) if code in r.get("aliases", [])), None)
if row:
    out["next"] = row["fix"]
    out["explain"] = "k2 explain " + row["id"]
print(json.dumps(out))
if row and os.environ.get("K2_RF_JSON") != "1":
    print("fix: " + row["fix"])
' 2>/dev/null)"
    fi
    [ -z "$line" ] && line="{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the K2 daemon (status ${status:-none})\"}"
    echo "$line" >&2
    case "$status" in
        400|413) exit 2 ;;
        # 0.46.1: `k2 compute get <missing>` sets _CP_404_EXIT=4 (not
        # found); other verbs keep their documented exit 3.
        404) exit "${_CP_404_EXIT:-3}" ;;
        *) exit 3 ;;
    esac
}

# GET/POST and fail on a refusal; the body lands in _CP_BODY.
_compute_call() {
    local raw
    raw="$(_compute_send "$@")"
    _compute_split "$raw"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

# JSON from KEY=VALUE env pairs: _compute_json K2_CJ_a=… (values are strings
# unless the key ends in _n (number), _b (bool) or _j (raw JSON)).
_compute_json() {
    env "$@" python3 -c '
import json, os
out = {}
for k, v in os.environ.items():
    if not k.startswith("K2_CJ_"):
        continue
    key = k[6:]
    if v == "":
        continue
    if key.endswith("_n"):
        out[key[:-2]] = float(v) if "." in v else int(v)
    elif key.endswith("_b"):
        out[key[:-2]] = v == "1"
    elif key.endswith("_j"):
        out[key[:-2]] = json.loads(v)
    else:
        out[key] = v
print(json.dumps(out))
'
}

# 2h / 90m / 300s / 300 → seconds.
_compute_secs() {
    # Plain seconds, or <n>h / <n>m / <n>s parts in any mix (1h30m). Only
    # digits ever reach $(( )): anything else is the usage error, never
    # arithmetic on the raw value.
    local v="$1" rest total=0 n
    if [[ "$v" =~ ^[0-9]+$ ]]; then echo "$((10#$v))"; return; fi
    if [[ ! "$v" =~ ^([0-9]+[hms])+$ ]]; then
        echo "usage: bad duration '$v' (use 2h, 90m, 1h30m, 300s)" >&2; exit 2
    fi
    rest="$v"
    while [[ "$rest" =~ ^([0-9]+)([hms])(.*)$ ]]; do
        n=$((10#${BASH_REMATCH[1]}))
        case "${BASH_REMATCH[2]}" in
            h) total=$((total + n * 3600)) ;;
            m) total=$((total + n * 60)) ;;
            s) total=$((total + n)) ;;
        esac
        rest="${BASH_REMATCH[3]}"
    done
    echo "$total"
}

# 16G / 512M / bytes → bytes.
_compute_bytes() {
    local v="$1"
    if [[ ! "$v" =~ ^([0-9]+)([GgMm]?)$ ]]; then
        echo "usage: bad size '$v' (use 16G or 512M)" >&2; exit 2
    fi
    local n=$((10#${BASH_REMATCH[1]}))
    case "${BASH_REMATCH[2]}" in
        G|g) echo $((n * 1073741824)) ;;
        M|m) echo $((n * 1048576)) ;;
        *) echo "$n" ;;
    esac
}

# Is this call carrying a session passport (inside a K2 terminal)?
_compute_in_session() {
    [ -n "${SCOPED_TOKEN:-}" ] && [ "${TOKEN:-}" = "${SCOPED_TOKEN:-}" ]
}

# The workspace to name: explicit --workspace, else this checkout (only
# outside a session; a passport's workspace comes from the passport).
_compute_ws() {
    local explicit="${1:-}"
    if [ -n "$explicit" ]; then
        printf '%s' "$explicit"
    elif ! _compute_in_session; then
        printf '%s' "$PROJECT"
    fi
}

_compute_render() {
    local verb="$1"
    printf '%s' "$_CP_BODY" | K2_CP_VERB="$verb" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
verb = os.environ["K2_CP_VERB"]
def gb(n):
    return "%d GB" % round((n or 0) / (1 << 30))
def cores(m):
    return "%g" % ((m or 0) / 1000)
def ago(t):
    if not t: return "-"
    d = int(time.time()) - int(t)
    return "now" if d < 60 else ("%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400)))
def node_line(n):
    o = n.get("offer") or {}
    st = n.get("state")
    if st == "pending":
        status = "pending (code %s)" % (n.get("sas") or "?")
    elif not n.get("online"):
        status = "offline"
    else:
        status = "online/%s" % (n.get("liveRoute") or "?")
        if o.get("control") and o["control"] != "active":
            status = o["control"] + (" by " + o["controlBy"] if o.get("controlBy") else "")
        elif o.get("refusal"):
            status = "waiting: " + o["refusal"].get("code", "")
        elif o.get("available") is False:
            status = "unavailable: " + ",".join(o.get("unavailable") or [])
    if n.get("controllerPause"):
        status += " (server: %s)" % n["controllerPause"]
    # 0.46.9 (protocol 7): the short platform column (ubuntu24.04-x86_64,
    # arch-x86_64, macos15.5-arm64); an older node: linux-x86_64 as before.
    os_arch = n.get("platform") or "%s-%s" % (n.get("os") or "?", n.get("arch") or "?")
    caps = o.get("caps") or {}
    free = o.get("free") or {}
    # free CPU = the cap minus what running K2 jobs reserved (not idle CPU);
    # free memory = the smaller of cap minus reserved and what the OS has.
    # 0.46.7 (CE-8): idle CPU (what the OS sees free) beside unreserved.
    idle = (" · idle ~%s" % cores(n["idleCpuMillis"])) if n.get("idleCpuMillis") is not None else ""
    res = ("cpu %s/%s unreserved%s  mem %.1f/%s free  disk %s" % (
        cores(free.get("cpu_millis")), cores(caps.get("cpu_millis")), idle,
        (free.get("mem_bytes") or 0) / (1 << 30), gb(caps.get("mem_bytes")), gb(free.get("disk_free_bytes")))) if o else "-"
    par = o.get("parallelMax")
    # 0.46.7 (CE-7): whose each count is. run/queued = jobs of this server
    # there (every workspace); yours = this workspace only; node runs = the
    # count of the node itself (other servers included).
    mine = n.get("mine") if isinstance(n.get("mine"), dict) else None
    run = "run %d/%s%s" % (n.get("running", 0), par if par is not None else "?", (" (yours %d)" % mine.get("running", 0)) if mine else "")
    queued = "queued %d%s" % (n.get("queued", 0), (" (yours %d)" % mine.get("queued", 0)) if mine else "")
    nr = n.get("nodeRunning")
    node_runs = (" · node runs %d" % nr) if nr is not None and nr != n.get("running", 0) else ""
    g = n.get("grant")
    grant = ("  grant %s/job %s parallel %s queued%s" % (dur(g.get("maxJobSecs")), g.get("maxParallel"), g.get("maxQueued"), " ci" if g.get("allowCi") else "")) if isinstance(g, dict) else ""
    reboot = "  reboot pending" if n.get("rebootPending") else ""
    return "%-14s %-19s %-24s %-64s %s · %s%s%s%s  seen %s%s" % (n["name"], os_arch, status[:24], res, run, queued, tier_counts(n.get("queueTiers")), node_runs, grant, ago(n.get("lastSeen")), reboot)
def tier_counts(t, bare=False):
    # Tiers (0.46.5): `CI 1 · P 0 · N 3`; nothing when none are queued.
    if not isinstance(t, dict) or not any(t.get(k) for k in ("ci", "priority", "normal", "batch")):
        return ""
    line = "CI %d · P %d · S %d" % (t.get("ci") or 0, t.get("priority") or 0, t.get("normal") or 0)
    if t.get("batch"):
        line += " · B %d" % t["batch"]
    return line if bare else " (%s)" % line
def tier_tag(t):
    return {"ci": "CI", "priority": "P", "batch": "B"}.get(t or "", "")
def dur(s):
    s = int(s or 0)
    if s >= 3600 and s % 3600 == 0: return "%dh" % (s // 3600)
    if s >= 60 and s % 60 == 0: return "%dm" % (s // 60)
    return "%ds" % s
def update_lines(n):
    # 0.46.9 (prd-compute-update-continuity-v1 §5.13): only while an update
    # is under way, or when a node needs a hand install; otherwise nothing.
    u = n.get("update") if isinstance(n.get("update"), dict) else {}
    r = n.get("rollout") if isinstance(n.get("rollout"), dict) else {}
    have = n.get("nodeVersion") or "?"
    phase = {"offered": "sent", "staged": "staged", "swapped": "health check"}.get(u.get("state") or "")
    if phase and u.get("version") and u.get("version") != have:
        return ["%-14s k2-node v%s -> %s (%s)" % ("", have, u["version"], phase)]
    if r.get("code") == "node_needs_manual_install":
        return ["%-14s k2-node v%s: needs one manual install (the updater fix is in 0.46.4): sudo k2-node setup at the machine" % ("", have)]
    if r.get("node") == "rolled_back":
        return ["%-14s k2-node %s rolled back here; the rollout paused (k2 compute node rollout)" % ("", r.get("toVersion") or "")]
    return []
def share_lines(n):
    # Shared nodes only (0.46.3); a solo node prints exactly as before.
    s = n.get("share")
    if not isinstance(s, dict):
        return []
    until = ""
    if s.get("expiresAt"):
        until = "   until " + time.strftime("%b %d", time.localtime(s["expiresAt"]))
    if s.get("role") == "tenant":
        head = "shared by %s (%s servers)%s" % (s.get("ownerLabel") or "?", s.get("servers") or "?", until)
    else:
        head = "shared with %d other server(s)" % max((s.get("servers") or 1) - 1, 0)
    if s.get("state") and s["state"] != "active":
        head += "   your share: " + s["state"]
    if s.get("skewSecs"):
        head += "   clock skew %+ds (node vs this server)" % s["skewSecs"]
    out = ["%-14s %s" % ("", head)]
    if s.get("line"):
        out.append("%-14s your share: %s" % ("", s["line"]))
    if s.get("priority") is False:
        out.append("%-14s your --tier priority runs as Normal here (its owner has not set priority=true for this server)" % "")
    return out
if verb == "nodes":
    rows = v.get("nodes", [])
    # Tiers (0.46.5): who is on the critical path on this server, so a
    # coordinator sees it before choosing (a courtesy, never enforced).
    for c in v.get("criticalPath") or []:
        print("critical path (Priority): %s (%s jobs)%s" % (c.get("submitter") or "?", c.get("jobs"), (" in %s" % c["workspace"]) if c.get("workspace") else ""))
    if not rows:
        print("(no compute nodes%s)" % ("" if v.get("waitingCodes") is None else "; add one with: k2 compute node add <name>"))
    for n in rows:
        # 0.46.8: a node off the node list of this workspace says so.
        print(node_line(n) + ("  [off the node list]" if n.get("allowed") is False else ""))
        for l in share_lines(n):
            print(l)
        for l in update_lines(n):
            print(l)
    for c in v.get("waitingCodes") or []:
        print("code waiting for %s (expires %s)" % (c["name"], time.strftime("%H:%M", time.localtime(c["expiresAt"]))))
    if isinstance(v.get("allowNodes"), list):
        print("node list (%s): jobs go only to %s (k2 compute allow-nodes)" % (v.get("allowNodesWorkspace") or "this workspace", ", ".join(v["allowNodes"]) or "nothing"))
elif verb == "info":
    print(node_line(v))
    for l in share_lines(v):
        print(l)
    # 0.46.9 (protocol 7): what the machine is, and its package manager.
    h = v.get("host")
    if isinstance(h, dict) and h.get("line"):
        print("host: " + h["line"])
    pk = v.get("packages")
    if isinstance(pk, dict):
        n_open = pk.get("openRequests") or 0
        bits = [pk.get("manager") or "?"]
        if pk.get("policy"):
            bits.append("policy for this server: " + pk["policy"])
        bits.append("%d open request%s" % (n_open, "" if n_open == 1 else "s"))
        print("packages: %s (k2 compute node packages %s)" % (" · ".join(bits), v.get("name")))
    o = v.get("offer") or {}
    nv = v.get("nodeVersion") or o.get("node_version")
    if nv or o.get("features") is not None:
        # 0.46.6: what the node supports follows its k2-node version
        # (Priority 0.46.5+; lines, turns, Batch, the per-submitter cap 0.46.6+).
        feats = o.get("features") or []
        if not v.get("shared"):
            what = "this server orders its queue: every line works"
        elif "tiers2" in feats:
            what = "shared; the node orders: lines, turns, Batch"
        elif "tiers" in feats:
            what = "shared; the node orders: Priority only (Batch waits for an idle node; no turns until k2-node 0.46.6)"
        else:
            what = "shared; the node orders without tiers (Priority has no effect there)"
        print("k2-node %s (protocol %s): %s" % (nv or "?", o.get("protocol", "?"), what))
    if o.get("tools"):
        print("tools: " + "  ".join("%s %s" % kv for kv in sorted(o["tools"].items())))
    if v.get("routes"):
        print("routes: " + " ".join(v["routes"]))
    for g in v.get("grants") or []:
        print("grant: %s  %s/job  %s parallel%s  %s queued  %s GB disk%s" % (g.get("workspace"), dur(g["maxJobSecs"]), g["maxParallel"], ("  %s per submitter" % g["submitterMaxRunning"]) if g.get("submitterMaxRunning") else "", g["maxQueued"], g["maxDiskGb"], "  CI allowed" if g.get("allowCi") else ""))
    for j in v.get("recentJobs") or []:
        print("job %s  %-9s exit %-4s %s" % (j["short"], j["state"], "-" if j.get("exitCode") is None else j["exitCode"], " ".join(j.get("argv") or [])[:60]))
elif verb == "jobs":
    rows = v.get("jobs", [])
    if not rows:
        print("(no jobs)")
    for j in rows:
        # 0.46.1: the full reason (it was cut to 10 characters).
        # Tiers (0.46.5): the TIER column (CI, P, blank for Normal).
        # 0.46.7 (CE-9): the --tag of the job and who sent it ("(owner token)"
        # when the passport of an agent was missing and the owner token went).
        who = j.get("submitter") or j.get("submittedBy") or "-"
        if j.get("via") == "owner_token":
            who += " (owner token)"
        # 0.46.8: a queued job says pinned (a node named; k2 compute move
        # <id> --auto releases it) or auto.
        state = j["state"]
        if state == "queued" and "pinned" in j:
            state += " (pin)" if j["pinned"] else " (auto)"
        print("%s  %-2s %-12s %-15s exit %-4s %-22s %-16s %-24s %s" % (j["short"], tier_tag(j.get("tier")), (j.get("node") or "?")[:12], state, "-" if j.get("exitCode") is None else j["exitCode"], j.get("reason") or "", (j.get("tag") or "-")[:16], who[:24], " ".join(j.get("argv") or [])[:60]))
elif verb == "get":
    j = v
    print("job %s on %s: %s%s" % (j["short"], j.get("node"), j["state"], (" (%s)" % j["reason"]) if j.get("reason") else ""))
    # 0.46.7: a long command (an inline base64 script) is summarized;
    # --full prints all of it.
    argv = " ".join(j.get("argv") or [])
    if os.environ.get("K2_CP_FULL") == "1" or len(argv) <= 160:
        print("  argv: %s" % argv)
    else:
        print("  argv: %s… (%d chars; --full for all of it)" % (argv[:120], len(argv)))
    # Tiers (0.46.5): shown when not Normal, or when the server lowered it.
    ctl = j.get("ctl") or {}
    if j.get("tierAsked") and j.get("tierAsked") != j.get("tier"):
        print("  tier %s (asked %s: downgraded)" % (j.get("tier") or "normal", j["tierAsked"]))
    elif j.get("tier") and j["tier"] != "normal":
        print("  tier %s" % j["tier"])
    if ctl.get("why"):
        print("  why (Priority): %s" % ctl["why"])
    if ctl.get("memDefaulted"):
        print("  memory: the default reservation (no --mem); set --mem next time")
    if j.get("tag"):
        print("  tag: %s" % j["tag"])
    # 0.46.7 (CE-9): who sent it; the owner token said so.
    by = j.get("submittedBy")
    if by or j.get("submitter"):
        print("  submitted by: %s%s%s" % (by or "-", (" as %s" % j["submitter"]) if j.get("submitter") else "", " (owner token: no passport in that shell)" if j.get("via") == "owner_token" else ""))
    if j.get("supersedeKey"):
        print("  supersede key: %s" % j["supersedeKey"])
    if j.get("secrets"):
        # Secret-env: the env names it gets from the vault, never values.
        print("  secrets (env names): %s" % ", ".join(j["secrets"]))
    pl = j.get("placement") or ctl.get("placement") or {}
    if pl.get("mode") == "auto":
        # Auto-placement (0.46.6): where K2 put it and why (a move says so).
        code = pl.get("code") or ""
        how = ("moved: " + code[len("moved_"):]) if code.startswith("moved_") else "auto"
        print("  placed on %s (%s, %s): %s%s" % (pl.get("node"), how, pl.get("policy"), pl.get("why"), (" · " + pl["fits"]) if pl.get("fits") else ""))
    print("  commit: %s  tree: %s" % (j.get("srcCommit") or j.get("submitCommit") or "-", j.get("srcTree") or "-"))
    print("  exit: %s  signal: %s  attempt %s (generation %s)" % (j.get("exitCode"), j.get("signal"), j.get("attempt"), j.get("generation")))
    if j.get("queue") and j["queue"].get("onNode"):
        # In the queue of a shared node: its place and ETA (0.46.3).
        q = j["queue"]
        print("  %s%s" % (q["message"], ("  [class %s]" % j["class"]) if j.get("class") else ""))
        if q.get("reason") and q["reason"] != "node_queued":
            print("  waiting for the node to answer: %s" % q["reason"])
    elif j.get("queue"):
        q = j["queue"]
        at = q.get("aheadTiers") or {}
        behind = ", ".join(x for x in [("%d CI" % at["ci"]) if at.get("ci") else "", ("%d Priority" % at["priority"]) if at.get("priority") else ""] if x)
        if q.get("line"):
            # 0.46.6 (L6): where it waits, N of M, why, and whether jobs
            # behind it may start first.
            print("  %s%s" % (q["line"], ("  [behind %s]" % behind) if behind else ""))
        else:
            print("  queued: position %s%s%s — %s (%s)" % (q["position"], (" of %s" % q["of"]) if q.get("of") else "", (", behind " + behind) if behind else "", q["reason"], q["message"]))
        if q.get("parallel"):
            print("  the node runs %s job(s) from this workspace at a time" % q["parallel"])
        for a in q.get("ahead") or []:
            age = a.get("elapsedSecs") if a.get("elapsedSecs") is not None else a.get("ageSecs")
            print("  ahead: %-9s %-9s %-14s %s  %s%s" % (
                a.get("short") or "-", a.get("state"), a.get("workspace") or "(other)",
                ("%dm%02ds" % (age // 60, age % 60)) if age is not None else "-",
                a.get("argv") or "", "  (" + a["reason"] + ")" if a.get("reason") else ""))
    wt = j.get("wait") or {}
    if wt and not j.get("queue") and wt.get("words"):
        # 0.46.7 (CE-7): every unfinished job says where it is.
        print("  %s%s" % (wt["words"], (", ~%s left (%s)" % (dur(wt["etaSecs"]), wt.get("etaBasis"))) if wt.get("etaSecs") is not None else ""))
    elif wt.get("etaSecs") is not None and j.get("queue") and not j["queue"].get("onNode"):
        print("  ETA ~%s (estimate from earlier runs)" % dur(wt["etaSecs"]))
    stl = j.get("stall") or {}
    if "stallSecs" in stl:
        # CE-10: the stall watch of a running job.
        print("  stall watch: %s" % (("ends after %s of silence (--stall-timeout)%s" % (dur(stl["stallSecs"]), "; warned" if stl.get("warned") else "")) if stl.get("stallSecs") else "off"))
    rl = j.get("reportedLate") or {}
    if rl.get("secs"):
        # CE-5: the end time of the node itself; this server heard of it late.
        hm = lambda t: time.strftime("%H:%M", time.localtime(t)) if t else "?"
        print("  the node finished at %s; this server heard at %s (%s late: it was asleep, or the node was cut off). Durations use the times of the node." % (hm(rl.get("nodeEndedAt")), hm(rl.get("heardAt")), ("%dh%02dm" % (rl["secs"] // 3600, (rl["secs"] % 3600) // 60)) if rl["secs"] >= 3600 else ("%dm" % (rl["secs"] // 60))))
    if j.get("hint"):
        print("  hint: %s" % j["hint"])
    cb = j.get("cancelledBy")
    if isinstance(cb, dict):
        print("  cancel asked by %s at %s" % (cb.get("by"), time.strftime("%H:%M:%S", time.localtime(cb.get("at") or 0))))
    r = j.get("result") or {}
    if j.get("reason") == "tenant_disk_full" and r.get("detail"):
        print("  " + r["detail"])
    for n in r.get("notes") or []:
        if n.startswith("evicted "):
            print("  " + n)
    t = r.get("tests")
    if t:
        print("  tests: %d passed, %d failed, %d errors, %d skipped" % (t["passed"], t["failed"], t["errors"], t["skipped"]))
    for a in r.get("artifacts") or []:
        print("  artifact: %s  %d bytes  sha256 %s…" % (a["name"], a["bytes"], a["sha256"][:12]))
    if r.get("receipt"):
        print("  receipt: %s (%s)" % (r["receipt"].get("id"), "verified" if r["receipt"].get("verified") else "NOT verified"))
    tl = r.get("tools") or {}
    if tl.get("versions"):
        # CE-4: exact versions (what the job used; node-probe = an older
        # node-wide probe of an older node, not necessarily what the job used).
        print("  tools%s: %s" % (" (node-wide probe, older k2-node)" if tl.get("basis") == "node-probe" else "", ", ".join("%s %s" % kv for kv in sorted(tl["versions"].items()))))
    rs = r.get("resources") or {}
    if rs:
        # CE-14: peak memory and CPU time.
        pk, lim = rs.get("peakMemBytes"), rs.get("memLimitBytes")
        print("  resources: peak memory %s%s (%s), CPU time %s (%s)" % (
            "-" if pk is None else "%.1fG" % (pk / (1 << 30)), "" if lim is None else " of %.1fG" % (lim / (1 << 30)), rs.get("memSource") or "-",
            "-" if rs.get("cpuSecs") is None else "%ss" % round(rs["cpuSecs"]), rs.get("cpuSource") or "-"))
        g = rs.get("gpu")
        if isinstance(g, dict):
            # GPU jobs S4: the GPU of the job, the whole device (host use included).
            gb = lambda b: "%.1fG" % (b / (1 << 30))
            bits = ["busy avg %s%% (max %s%%)" % (g.get("busyAvgPct"), g.get("busyMaxPct")) if g.get("busyAvgPct") is not None else "busy % not reported"]
            if g.get("vramPeakBytes") is not None: bits.append("VRAM +" + gb(g["vramPeakBytes"]))
            if g.get("gttPeakBytes") is not None: bits.append("GTT +" + gb(g["gttPeakBytes"]))
            print("  gpu: %s: %s (whole device, %s)" % (g.get("model") or g.get("pci"), ", ".join(bits), g.get("source") or "-"))
            # GPU jobs S6: the GPU memory of the job itself (DRM fdinfo) and its budget.
            if g.get("memLimitBytes") is not None:
                own = "peak " + gb(g["jobMemPeakBytes"]) if g.get("jobMemPeakBytes") is not None else "not read"
                print("  gpu memory (the job itself): %s of --gpu-mem %s" % (own, gb(g["memLimitBytes"])))
    hs = r.get("host") or {}
    if hs:
        # CE-19: the load of the node at the start and end (other K2 jobs, steal).
        def stamp(s):
            if not s: return "-"
            return "load %s of %s CPUs, %s other job(s)%s%s%s" % (s.get("load1"), s.get("cpus"), s.get("jobs"), (", steal %s%%" % s["steal"]) if s.get("steal") is not None else "",
                (", GPU %s%% busy" % s["gpuBusy"]) if s.get("gpuBusy") is not None else "",
                (", quiet" if s["quiet"] else ", not quiet") if s.get("quiet") is not None else "")
        print("  node at start: %s · at end: %s" % (stamp(hs.get("start")), stamp(hs.get("end"))))
    eff = r.get("effective")
    if isinstance(eff, dict) and eff:
        # CE-3: the effective config (each setting and where it came from).
        print("  effective: %s" % json.dumps(eff, separators=(",", ":"))[:300])
    for x in r.get("shards") or []:
        t = x.get("tests")
        print("  shard %s/%d: job %s on %s · %s%s%s" % (x["index"], len(r["shards"]), x["short"], x.get("node") or "?", x["state"],
              (" exit %s" % x["exit"]) if x.get("exit") is not None else "",
              (" · %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else ""))
elif verb == "receipt":
    print(json.dumps(v.get("receipt"), indent=2))
    print("verified: %s  (%s)" % ("yes" if v.get("verified") else "NO", v.get("proves")))
elif verb == "grants":
    rows = v.get("grants", [])
    if not rows:
        print("(no grants)")
    for g in rows:
        print("%-14s %-20s max %ss/job  %s parallel  %s queued  %s GB disk%s  by %s" % (g.get("node"), g.get("workspace"), g["maxJobSecs"], g["maxParallel"], g["maxQueued"], g["maxDiskGb"], "  ci" if g.get("allowCi") else "", g["grantedBy"]))
elif verb == "usage":
    rows = v.get("usage", [])
    if not rows:
        print("(no traffic in this window)")
    for u in rows:
        print("%s  %-14s %-9s in %8.1f MB  out %8.1f MB" % (u["day"], u.get("node") or u["nodeId"][:8], u["route"], u["bytesIn"] / 1e6, u["bytesOut"] / 1e6))
    print(v.get("metered", ""))
elif verb == "events":
    for e in v.get("events", []):
        print("%s  %-16s %-18s %s %s" % (time.strftime("%m-%d %H:%M:%S", time.localtime(e["at"])), e["kind"], e["actor"][:18], (e.get("jobId") or e.get("nodeId") or "")[:8], json.dumps(e.get("detail"))[:80]))
elif verb == "queue":
    rows = v.get("entries") or []
    print("%s%s" % (v.get("node"), "" if v.get("from") == "node" else "  (jobs from this server only: the node keeps no shared queue)"))
    if not rows:
        print("(nothing running or queued)")
    else:
        print("%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s" % ("#", "who", "tier", "class", "state", "cpus", "eta", "why"))
    for e in rows:
        if e.get("mine") and e.get("short"):
            # 0.46.8: a queued job of yours says pin (a node named) or auto.
            pin = ""
            if e.get("state") == "queued" and "pinned" in e:
                pin = "[pin] " if e["pinned"] else "[auto] "
            who = "you: %s%s %s" % (pin, e["short"], e.get("argv") or "")
            if e.get("workspace"):
                who += " (ws %s)" % e["workspace"]
        elif e.get("mine"):
            who = "this server (other workspace)"
        else:
            who = e.get("server") or "another server"
        if e.get("tierWhy"):
            # The Priority one-line reason (0.46.6): who holds the critical path.
            who_why = "P: %s%s" % (e["tierWhy"], (" (%s)" % e["submitter"]) if e.get("submitter") else "")
        else:
            who_why = ""
        eta = e.get("etaSecs")
        if eta is None:
            etas = "-"
        elif e.get("state") == "running":
            etas = "~%dm left" % max(1, (eta + 59) // 60)
        else:
            etas = "~%dm" % max(1, (eta + 59) // 60)
        # WHY (0.46.6, L6): the wait in words (an older daemon: the code).
        why = e.get("why") or e.get("blockedBy") or ""
        cpus = ("%g" % (e["cpuMillis"] / 1000)) if e.get("cpuMillis") else "-"
        print("%-3s %-34s %-4s %-12s %-8s %-5s %-10s %s" % (e.get("pos"), who[:34], tier_tag(e.get("tier")) or "-", e.get("class") or "-", e.get("state"), cpus, etas, why))
        if who_why:
            print("    %s" % who_why)
    if tier_counts(v.get("tiers")):
        print("queued: " + tier_counts(v.get("tiers"), bare=True))
    for t in v.get("totals") or []:
        print("server %-20s running %s  queued %s  today %.1f CPU-h" % (t["label"], t["running"], t["queued"], (t.get("cpuSecsToday") or 0) / 3600))
elif verb == "servers":
    if v.get("role") == "tenant":
        print("%s is shared with this server by %s" % (v["node"], v.get("owner") or "its owner"))
        if v.get("line"):
            print("  your share: %s" % v["line"])
        s = v.get("share") or {}
        if s.get("expires_in_secs"):
            print("  ends in %dh" % (s["expires_in_secs"] // 3600))
    else:
        print("%-4s %-24s %-9s %-5s %-6s %s" % ("slot", "server", "state", "trust", "weight", "share"))
        for s in v.get("servers") or []:
            l = s.get("limits") or {}
            share = ("run %s · queue %s · %s CPU-h/day · %s/job · %s CPUs · %s GB · net %s" % (
                l.get("max_running"), l.get("max_queued"), l.get("cpu_hours_day"), dur(l.get("max_job_secs")), l.get("max_cpus"), l.get("max_mem_gb"), l.get("network"))) if l else ("(this server; no other servers yet)" if s.get("solo") else "-")
            st = s.get("state")
            if s.get("sas"):
                st += " · SAS " + s["sas"]
            u = s.get("usage") or {}
            if u.get("refusal"):
                st = "refused: %s" % u["refusal"]
            print("%-4s %-24s %-9s %-5s %-6s %s" % (s.get("slot"), (s.get("label") or "?")[:24], st, s.get("trust"), l.get("weight", "-"), share))
            extra = []
            if u.get("running") is not None:
                extra.append("running %s, queued %s" % (u.get("running"), u.get("queued")))
            if u.get("cpu_secs_today") is not None:
                extra.append("today %.1f CPU-h" % (u["cpu_secs_today"] / 3600))
            if s.get("expiresAt"):
                extra.append("until " + time.strftime("%b %d %H:%M", time.localtime(s["expiresAt"])))
            # Tiers (0.46.5): does its --tier priority count here?
            if s.get("priority") is True:
                extra.append("Priority counts")
            elif s.get("priority") is False and s.get("slot") != 0:
                extra.append("Priority runs as Normal")
            if extra:
                print("%-4s %s" % ("", " · ".join(extra)))
        for r in v.get("requests") or []:
            print("waiting: %s  %s  %s" % (r["id"], r["state"], r["summary"]))
        if v.get("note"):
            print("note: %s" % v["note"])
elif verb == "enroll":
    print("Enroll code for %s (expires in 10 minutes):" % v["name"])
    print("  %s" % v["enroll"])
    print("Keep it off command lines (ps shows them to every user): put it in a file only you")
    print("can read, or in the vault (k2 vault set NAME --kind generic --delivery file), and pipe it in.")
    print("On the node machine, as an admin (code-file holds the code):")
    print("  Linux: %s" % v["install"]["linux"])
    print("  macOS: %s" % v["install"]["macos"])
    if v["install"].get("servers"):
        print("  Already a node for another K2 server: %s" % v["install"]["servers"])
    print("The node prints a 6-digit code. Check it, then here:")
    print("  %s" % v["confirm"])
    print("Scripts: k2 compute node add %s --json prints {code, expiresAt, name, controller}." % v["name"])
else:
    print(json.dumps(v, indent=2))
'
}

# Print a job's result object (the `result` of jobs/get) on stdout.
_compute_print_result() {
    _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$1")"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result"), indent=2))'
}

# Follow a job's logs until it ends; exit with its result.
# _compute_follow <job> <raw 0|1> [cursor|seq:N] [json 0|1]
# json=1: every log line goes to stderr and the result object is printed
# on stdout at the end (same exit code).
# Callers may set _CP_DEADLINE (epoch seconds: run --wait --max-wait, wait
# --follow --max-wait; past it: still_running, exit 3, the job keeps going)
# and _CP_FAILURES=1 (after a failing exit, the failing tests).
_compute_follow() {
    local job="$1" raw="${2:-0}" start="${3:-0}" as_json="${4:-0}"
    local cursor=0 query state_file fails=0 lost=0 gen=""
    state_file="$(mktemp -t k2-compute-XXXXXX)"
    # shellcheck disable=SC2064
    trap "rm -f '$state_file' '$state_file.err' '$state_file.q'; echo '[k2 compute] detached; the job keeps running. k2 compute attach ${job:0:8} | k2 compute cancel ${job:0:8} --force' >&2; exit 130" INT
    case "$start" in
        seq:*) query="since_seq=${start#seq:}" ;;
        *) cursor="$start"; query="" ;;
    esac
    while :; do
        local poll=20
        if [ -n "${_CP_DEADLINE:-}" ]; then
            local left=$(( _CP_DEADLINE - $(date +%s) ))
            if [ "$left" -le 0 ]; then
                rm -f "$state_file" "$state_file.err" "$state_file.q"
                trap - INT
                echo "{\"error\":\"still_running\",\"jobs\":[\"${job:0:8}\"],\"hint\":\"--max-wait passed; the job keeps going: k2 compute wait ${job:0:8} --follow\"}" >&2
                exit 3
            fi
            [ "$left" -lt "$poll" ] && poll="$left"
        fi
        local q="job=$(urlencode "$job")&wait=$poll"
        # Follow one attempt at a time: a retry is a new generation.
        [ -n "$gen" ] && q="$q&generation=$gen"
        if [ -n "$query" ]; then q="$q&$query"; query=""; else q="$q&cursor=$cursor"; fi
        local resp
        resp="$(_compute_send GET /cli/compute/jobs/logs "$q")"
        _compute_split "$resp"
        if [ "$_CP_STATUS" = "000" ]; then
            fails=$((fails + 1))
            if [ "$fails" -eq 1 ]; then
                echo "[k2 compute] lost the K2 server; retrying (the job keeps running on the node)" >&2
            fi
            if [ "$fails" -gt 60 ]; then
                rm -f "$state_file"
                echo "{\"error\":\"lost_contact\",\"hint\":\"no answer from the K2 server for 10 minutes; the job may still be running: k2 compute attach $job\"}" >&2
                exit 3
            fi
            lost=1
            sleep 10
            continue
        fi
        [ "$_CP_STATUS" = "200" ] || { rm -f "$state_file"; _compute_fail "$_CP_STATUS" "$_CP_BODY"; }
        fails=0
        printf '%s' "$_CP_BODY" | K2_CP_RAW="$raw" K2_CP_STATE="$state_file" K2_CP_LOST="$lost" K2_CP_JSON="$as_json" python3 -c '
import base64, json, os, re, sys
v = json.load(sys.stdin)
raw = os.environ.get("K2_CP_RAW") == "1"
to_err = os.environ.get("K2_CP_JSON") == "1"
esc = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")
chunks = v.get("chunks", [])
if os.environ.get("K2_CP_LOST") == "1" and chunks:
    sys.stderr.write("[k2 compute] reattached at seq %d\n" % chunks[0]["seq"])
for c in chunks:
    data = base64.b64decode(c["data"])
    if not raw:
        data = esc.sub(b"", data)
    if c["stream"] == "out" and not to_err:
        sys.stdout.buffer.write(data); sys.stdout.buffer.flush()
    else:
        sys.stderr.buffer.write(data); sys.stderr.buffer.flush()
j = v.get("job", {})
q = v.get("queue") or {}
def s(x):
    return re.sub(r"[^A-Za-z0-9_.:-]", "", str(x if x is not None else ""))
# "|", not a tab: bash `read` merges runs of IFS whitespace, so an empty
# field (no signal, no reason) shifted every later one (mini-1 smoke printed
# the start time as the job id). s() never lets "|" through.
fa = ((j.get("ctl") or {}).get("failedAttempts") or [{}])[-1]
open(os.environ["K2_CP_STATE"], "w").write("|".join([
    s(v.get("cursor", 0)), "1" if v.get("end") else "0", s(j.get("state")), s(j.get("exitCode")),
    s(j.get("signal")), s(j.get("reason")), s(j.get("short")), s(j.get("node")),
    s(j.get("startedAt")), s(j.get("endedAt")), s(q.get("position")), s(q.get("waitReason") or q.get("reason")),
    s(v.get("generation")), s(j.get("generation")), s(j.get("attempt")), s(fa.get("reason") or fa.get("state")),
]) + "\n")
# The wait line (0.46.6: where it waits, N of M, why) in its own file: it
# has spaces and punctuation the "|" fields drop.
open(os.environ["K2_CP_STATE"] + ".q", "w").write((q.get("line") or "").replace("\n", " ") + "\n")
# The teaching error line for a non-result end (JSON-safe here, not in bash).
err = {"error": j.get("state"), "reason": j.get("reason"), "detail": j.get("detail"),
       "hint": "k2 compute get %s --json; k2 compute retry %s" % (j.get("short"), j.get("short"))}
open(os.environ["K2_CP_STATE"] + ".err", "w").write(json.dumps(err) + "\n")
'
        local end st code sig reason short node started ended qpos qreason fgen jgen attempt lastfail
        IFS='|' read -r cursor end st code sig reason short node started ended qpos qreason fgen jgen attempt lastfail < "$state_file"
        lost=0
        [ -z "$gen" ] && gen="$fgen"
        # The attempt we followed ended in an infrastructure failure and a
        # new attempt is queued: keep following that one.
        if [ "$end" = "1" ] && [ -n "$jgen" ] && [ "$jgen" != "$gen" ] && [ "$st" != "done" ] && [ "$st" != "failed" ] && [ "$st" != "cancelled" ] && [ "$st" != "timeout" ] && [ "$st" != "interrupted" ] && [ "$st" != "unknown" ]; then
            echo "[k2 compute] attempt $((attempt - 1)) hit an infrastructure failure (${lastfail:-?}); retrying as attempt $attempt on $node" >&2
            gen="$jgen"
            cursor=0
            _CP_LAST_Q=""
            continue
        fi
        # A line on every position or reason change (k2 compute get <job>
        # shows the jobs ahead).
        if [ "$st" = "queued" ] && [ -n "$qreason" ] && [ "$qpos|$qreason" != "${_CP_LAST_Q:-}" ]; then
            local qline=""
            IFS= read -r qline < "$state_file.q" || true
            if [ -n "$qline" ]; then
                echo "[k2 compute] $qline" >&2
            else
                echo "[k2 compute] queued on $node: position $qpos ($qreason)" >&2
            fi
            _CP_LAST_Q="$qpos|$qreason"
        fi
        if [ "$end" = "1" ]; then
            rm -f "$state_file" "$state_file.q"
            trap - INT
            [ "$st" = "done" ] && rm -f "$state_file.err"
            [ "$as_json" = "1" ] && _compute_print_result "$job"
            local took=""
            if [ -n "$started" ] && [ -n "$ended" ]; then
                took=" after $(( (ended - started) / 60 ))m$(( (ended - started) % 60 ))s"
            fi
            case "$st" in
                done)
                    if [ -n "$code" ]; then
                        echo "[k2 compute] exit $code$took · k2 compute logs $short --failures" >&2
                        if [ "$code" != "0" ] && [ "${_CP_FAILURES:-0}" = "1" ]; then
                            echo "── failures in $short (k2 compute logs $short --failures --tail 60) ──" >&2
                            ( cmd_compute_logs "$job" --failures --tail 60 ) >&2 || true
                        fi
                        exit "$code"
                    fi
                    echo "[k2 compute] killed by signal $sig$took" >&2
                    exit $((128 + ${sig:-0}))
                    ;;
                *)
                    echo "[k2 compute] job $short $st${reason:+ ($reason)}$took" >&2
                    cat "$state_file.err" >&2
                    rm -f "$state_file.err"
                    exit 3
                    ;;
            esac
        fi
    done
}

cmd_compute_run() {
    local node="" any=0 os="" arch="" detach=0 dirty=0 sha="" nosrc=0 cpus="" mem="" timeout="" exclusive=0
    local retry="" raw=0 workspace="" wait=0 wait_failures=0 wait_quiet=0 max_wait="" as_json=0 qtimeout="" retries="" stall="" nshards="" plan=0
    local gate="" tier="" supersede="" submitter="" repo="" cache="" why=""
    local preset="" now=0 tag="" same_node="" distro="" receipt_v="" gpu="" gpu_mem=""
    local -a labels=() envs=() needs=() artifacts=() secrets=() withs=() avoids=() onlys=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; break ;;
            # --avoid NODE (0.46.8, repeatable): K2 never picks that node for
            # this job (submit, moves, retries). Not with a named node.
            --avoid) avoids+=("${2:-}"); shift 2
                [ -n "${avoids[${#avoids[@]}-1]}" ] || { echo "usage: --avoid <node> (a node K2 must not pick; repeatable)" >&2; exit 2; } ;;
            # --only NODE (0.46.9, repeatable): K2 picks only among these
            # nodes. Not with a named node.
            --only) onlys+=("${2:-}"); shift 2
                [ -n "${onlys[${#onlys[@]}-1]}" ] || { echo "usage: --only <node> (K2 picks only among the nodes you list; repeatable)" >&2; exit 2; } ;;
            # --preset NAME: [presets.NAME] from .k2/compute.toml (0.46.7;
            # the daemon reads the file, flags still win).
            --preset) preset="${2:-}"; shift 2
                if ! [[ "$preset" =~ ^[a-z0-9][a-z0-9._-]{0,39}$ ]]; then
                    echo "usage: --preset NAME (a [presets.NAME] in .k2/compute.toml: 1–40 of a-z 0-9 . _ -)" >&2; exit 2
                fi ;;
            # --now: start at once or fail (exit 3 not_now), nothing queued.
            --now) now=1; shift ;;
            # --tag NAME: your own label for the job (jobs --tag filters).
            --tag) tag="${2:-}"; shift 2
                if ! [[ "$tag" =~ ^[A-Za-z0-9._:/-]{1,64}$ ]]; then
                    echo "usage: --tag NAME (1–64 of A-Z a-z 0-9 . _ : / -, e.g. fuzz:p3)" >&2; exit 2
                fi ;;
            # --same-node-as JOB: run on the node that ran one of your jobs.
            --same-node-as) same_node="${2:-}"; shift 2
                [ -n "$same_node" ] || { echo "usage: --same-node-as <job> (one of this workspace's jobs)" >&2; exit 2; } ;;
            # --repo <folder>: a git repo (or a worktree of one) inside the
            # workspace, shipped instead of the workspace's own (0.46.6).
            --repo) repo="${2:-}"; shift 2
                [ -n "$repo" ] || { echo "usage: --repo <folder> (a git repo inside the workspace, e.g. k2-multiplayer-base)" >&2; exit 2; } ;;
            # --with DIR[@SHA]: a sibling repo shipped beside the main one,
            # laid out as in the workspace (0.46.6). Repeatable.
            --with) withs+=("${2:-}"); shift 2
                [ -n "${withs[${#withs[@]}-1]}" ] || { echo "usage: --with DIR[@SHA] (a git repo inside the workspace, or a worktree of one)" >&2; exit 2; } ;;
            # --cache NAME: a folder on the node that outlives the job
            # ($K2_JOB_CACHE), per workspace and name (0.46.6).
            --cache) cache="${2:-}"; shift 2
                if ! [[ "$cache" =~ ^[a-z0-9][a-z0-9._-]{0,63}$ ]]; then
                    echo "usage: --cache NAME (1–64 of a-z 0-9 . _ -, starting with a letter or digit)" >&2; exit 2
                fi ;;
            # --gate = --tier ci (0.46.5; an older daemon only knows gate,
            # so both go in the body).
            --gate) gate=1; tier="ci"; shift ;;
            # --priority (0.46.6; --critical kept) needs --why "<reason>".
            --priority|--critical) tier="priority"; shift ;;
            --why) why="${2:-}"; shift 2 ;;
            --tier) tier="${2:-}"; shift 2
                case "$tier" in
                    ci|priority|batch) ;;
                    normal|standard) tier="normal" ;;
                    *) echo "usage: --tier standard|priority|batch|ci (k2 study compute: TIERS)" >&2; exit 2 ;;
                esac
                [ "$tier" = "ci" ] && gate=1 ;;
            --supersede) supersede="${2:-}"; shift 2
                if ! printf '%s' "$supersede" | python3 -c '
import sys
k = sys.stdin.read()
ok = 1 <= len(k) <= 120 and k == k.strip() and not any(ord(c) < 32 or ord(c) == 127 for c in k)
sys.exit(0 if ok else 1)'; then
                    echo "usage: --supersede KEY (1–120 characters, no control characters, no leading/trailing space)" >&2; exit 2
                fi ;;
            --submitter) submitter="${2:-}"; shift 2
                if ! [[ "$submitter" =~ ^[a-z0-9._-]{1,40}$ ]]; then
                    echo "usage: --submitter NAME (1–40 of a-z 0-9 . _ -)" >&2; exit 2
                fi ;;
            --secret) secrets+=("${2:-}"); shift 2 ;;
            --wait) wait=1; shift ;;
            # --wait --quiet: the old exit-only wait (no log text; 0.46.6).
            --quiet|-q) wait_quiet=1; shift ;;
            --failures) wait_failures=1; shift ;;
            --max-wait) max_wait="${2:-}"; shift 2 ;;
            --any) any=1; shift ;;
            --os) os="${2:-}"; shift 2 ;;
            # --distro ubuntu|ubuntu@24.04|debian (0.46.9): the node's
            # /etc/os-release ID or ID_LIKE, the version on a dot prefix.
            --distro) distro="$(_compute_distro_arg "${2:-}")" || exit 2; shift 2 ;;
            --arch) arch="${2:-}"; shift 2 ;;
            --label) labels+=("${2:-}"); shift 2 ;;
            --detach|-d) detach=1; shift ;;
            --dirty) dirty=1; shift ;;
            --sha) sha="${2:-}"; shift 2 ;;
            --no-source) nosrc=1; shift ;;
            --env) envs+=("${2:-}"); shift 2 ;;
            --needs|--need) needs+=("${2:-}"); shift 2 ;;
            --cpus) cpus="${2:-}"; shift 2 ;;
            --mem) mem="$(_compute_bytes "${2:-}")"; shift 2 ;;
            --timeout) timeout="$(_compute_secs "${2:-}")"; shift 2 ;;
            --exclusive) exclusive=1; shift ;;
            # --gpu [any|amd|intel] (0.46.11): one GPU of the node, to this
            # job alone. Bare --gpu = any.
            --gpu=*) gpu="${1#--gpu=}"; shift
                _compute_gpu_arg "$gpu" || exit 2 ;;
            --gpu)
                case "${2:-}" in
                    ''|-*) gpu="any"; shift ;;
                    *) gpu="$2"; shift 2; _compute_gpu_arg "$gpu" || exit 2 ;;
                esac ;;
            # --gpu-mem SIZE (GPU jobs S6): the GPU job's GPU memory budget.
            --gpu-mem) gpu_mem="$(_compute_bytes "${2:-}")" || exit 2; shift 2 ;;
            --receipt-v2) receipt_v=2; shift ;;
            --retry-interrupted) retry=1; shift ;;
            --no-retry) retry=0; retries=0; shift ;;
            --retries) retries="${2:-}"; shift 2
                case "$retries" in ''|*[!0-9]*) echo "usage: --retries N (0–5)" >&2; exit 2 ;; esac ;;
            --queue-timeout)
                case "${2:-}" in
                    off|none|0) qtimeout=0 ;;
                    *) qtimeout="$(_compute_secs "${2:-}")" ;;
                esac
                shift 2 ;;
            --stall-timeout)
                case "${2:-}" in
                    off|none|0) stall=0 ;;
                    *) stall="$(_compute_secs "${2:-}")" ;;
                esac
                shift 2 ;;
            --raw) raw=1; shift ;;
            --artifact|--artifacts) artifacts+=("${2:-}"); shift 2 ;;
            --shards) nshards="${2:-}"; shift 2
                case "$nshards" in ''|*[!0-9]*) echo "usage: --shards N (2–16)" >&2; exit 2 ;; esac ;;
            --json) as_json=1; shift ;;
            --plan|--dry-run) plan=1; shift ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            -*) echo "usage: unknown flag '$1' (k2 compute --help)" >&2; exit 2 ;;
            *)
                if [ -z "$node" ] && [ "$any" = "0" ]; then node="$1"; shift
                else echo "usage: put the command after --: k2 compute run [<node>] -- <command…>" >&2; exit 2
                fi ;;
        esac
    done
    if [ $# -eq 0 ]; then
        echo "usage: k2 compute run [<node>] [flags] -- <command…>  (no node: K2 picks one)" >&2
        exit 2
    fi
    # No node named: K2 picks a granted node (auto-placement, 0.46.6).
    # `any` rides in the body so an older daemon still accepts it; --any
    # is the same thing, kept for scripts.
    [ -z "$node" ] && any=1
    if [ "$wait" = "1" ] && [ "$detach" = "1" ]; then
        echo "usage: --wait and --detach don't mix (--wait blocks until the end; --detach returns at once)" >&2
        exit 2
    fi
    # Tiers v2 (0.46.6): Priority needs a one-line reason; Batch waits for
    # an idle node (hours), so it is always detached.
    if [ "$tier" = "priority" ] && [ -z "$why" ]; then
        echo "usage: --priority needs a one-line reason: --priority --why \"release gate for 0.46.6\" (it shows in the queue and k2 compute status; Priority is for the critical path only)" >&2
        exit 2
    fi
    # 0.46.7: --exclusive holds a whole node, so it needs a reason too.
    if [ "$exclusive" = "1" ] && [ -z "$why" ]; then
        echo "usage: --exclusive needs a one-line reason: --exclusive --why \"benchmark needs the whole node\" (it shows in the queue; it also needs a grant with --ci)" >&2
        exit 2
    fi
    if [ "$now" = "1" ] && { [ "$tier" = "batch" ] || [ -n "$nshards" ]; }; then
        echo "usage: --now doesn't mix with --tier batch (it waits for an idle node) or --shards (each shard is placed itself)" >&2
        exit 2
    fi
    if [ "$tier" = "batch" ]; then
        if [ "$wait" = "1" ]; then
            echo "usage: --tier batch doesn't mix with --wait: a Batch job starts only when the node is idle (maybe hours). It is detached; k2 compute wait <id> later" >&2
            exit 2
        fi
        detach=1
    fi
    if { [ "$wait_failures" = "1" ] || [ -n "$max_wait" ] || [ "$wait_quiet" = "1" ]; } && [ "$wait" = "0" ]; then
        echo "usage: --failures, --max-wait and --quiet go with --wait (k2 compute run <node> --wait --failures -- …)" >&2
        exit 2
    fi
    # --wait: attached on the daemon (detach:false), so an agent in a K2 tab
    # gets no second [compute] line while `wait` follows it; one automatic
    # retry after an interruption, like --detach.
    [ "$wait" = "1" ] && [ -z "$retry" ] && retry=1
    local cwd="" repo_dir=""
    if _k2_git_usable; then
        cwd="$(git rev-parse --show-prefix 2>/dev/null || true)"
        cwd="${cwd%/}"
        # The checkout we're in: from a git worktree inside the workspace the
        # daemon takes HEAD (and --dirty) from it, not the main checkout.
        repo_dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"
        # (--repo naming another repo than the one we're in: the daemon
        # starts the job at that repo's root; our folder means nothing there.)
    fi
    local client
    client="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
    local argv_json labels_json env_json
    argv_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@")"
    labels_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --label key=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${labels[@]+"${labels[@]}"}")" || exit 2
    env_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --env NAME=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${envs[@]+"${envs[@]}"}")" || exit 2
    local needs_json=""
    if [ "${#needs[@]}" -gt 0 ]; then
        needs_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${needs[@]}")"
    fi
    local artifacts_json=""
    if [ "${#artifacts[@]}" -gt 0 ]; then
        artifacts_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${artifacts[@]}")"
    fi
    # --secret NAME[=ENVNAME]: names only (the value never leaves the vault
    # until the node asks for it at spawn, sealed).
    local secrets_json=""
    if [ "${#secrets[@]}" -gt 0 ]; then
        secrets_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    if not a or a.startswith("="):
        sys.stderr.write("usage: --secret NAME[=ENVNAME]\n"); sys.exit(2)
    n, _, e = a.partition("=")
    o = {"name": n}
    if e: o["env"] = e
    out.append(o)
print(json.dumps(out))' "${secrets[@]}")" || exit 2
    fi
    local with_json=""
    if [ "${#withs[@]}" -gt 0 ]; then
        with_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    repo, at, sha = a.rpartition("@")
    if not at:
        repo, sha = a, ""
    if not repo:
        sys.stderr.write("usage: --with DIR[@SHA]\n"); sys.exit(2)
    o = {"repo": repo}
    if sha: o["sha"] = sha
    out.append(o)
print(json.dumps(out))' "${withs[@]}")" || exit 2
    fi
    local avoid_json=""
    if [ "${#avoids[@]}" -gt 0 ]; then
        avoid_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${avoids[@]}")"
    fi
    local only_json=""
    if [ "${#onlys[@]}" -gt 0 ]; then
        only_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${onlys[@]}")"
    fi
    local ws_name
    ws_name="$(_compute_ws "$workspace")"
    # --gpu (0.46.11): one GPU of the vendor asked (count 1 in v1).
    local gpu_json=""
    [ -n "$gpu" ] && gpu_json="{\"vendor\": \"$gpu\", \"count\": 1}"
    # --gpu-mem (GPU jobs S6): only with --gpu.
    if [ -n "$gpu_mem" ] && [ -z "$gpu" ]; then
        echo "usage: --gpu-mem needs --gpu: it is the GPU memory budget of a GPU job (k2 compute run --gpu --gpu-mem 4G --mem 8G -- <command>)" >&2
        exit 2
    fi
    local body
    body="$(_compute_json K2_CJ_node="$node" K2_CJ_any_b="$any" K2_CJ_os="$os" K2_CJ_distro="$distro" K2_CJ_arch="$arch" K2_CJ_avoid_j="$avoid_json" K2_CJ_only_j="$only_json" \
        K2_CJ_labels_j="$labels_json" K2_CJ_env_j="$env_json" K2_CJ_argv_j="$argv_json" K2_CJ_needs_j="$needs_json" \
        K2_CJ_artifacts_j="$artifacts_json" K2_CJ_secrets_j="$secrets_json" K2_CJ_queueTimeoutSecs_n="$qtimeout" K2_CJ_retries_n="$retries" K2_CJ_stallSecs_n="$stall" K2_CJ_shards_n="$nshards" K2_CJ_plan_b="$plan" \
        K2_CJ_detach_b="$detach" K2_CJ_dirty_b="$dirty" K2_CJ_sha="$sha" K2_CJ_noSource_b="$nosrc" \
        K2_CJ_cpus_n="$cpus" K2_CJ_memBytes_n="$mem" K2_CJ_timeoutSecs_n="$timeout" \
        K2_CJ_exclusive_b="$exclusive" K2_CJ_retryInterrupted_b="$retry" K2_CJ_cwd="$cwd" \
        K2_CJ_repoDir="$repo_dir" K2_CJ_workspace="$ws_name" K2_CJ_clientJobId="$client" K2_CJ_gate_b="$gate" \
        K2_CJ_tier="$tier" K2_CJ_why="$why" K2_CJ_supersede="$supersede" K2_CJ_submitter="$submitter" \
        K2_CJ_repo="$repo" K2_CJ_cache="$cache" K2_CJ_with_j="$with_json" \
        K2_CJ_preset="$preset" K2_CJ_now_b="${now#0}" K2_CJ_tag="$tag" K2_CJ_sameNodeAs="$same_node" K2_CJ_receiptV_n="$receipt_v" \
        K2_CJ_gpu_j="$gpu_json" K2_CJ_gpuMemBytes_n="$gpu_mem")"
    [ "$any" = "0" ] && body="$(printf '%s' "$body" | python3 -c 'import json,sys; v=json.load(sys.stdin); v.pop("any", None); print(json.dumps(v))')"
    # The same clientJobId makes a retried POST idempotent.
    local raw_resp attempt=0
    while :; do
        raw_resp="$(_compute_send POST /cli/compute/run "$body")"
        _compute_split "$raw_resp"
        [ "$_CP_STATUS" != "000" ] && break
        attempt=$((attempt + 1))
        [ "$attempt" -ge 3 ] && break
        sleep 2
    done
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$plan" = "1" ]; then
        # Dry run: one JSON object on stdout, a one-line summary on stderr.
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
e = v.get("estimate")
def d(x):
    return "%dm%02ds" % (x // 60, x % 60)
line = "[k2 compute] plan: %s on %s" % ("starts now" if v.get("startsNow") else "waits (%s ahead%s)" % (v["queue"]["ahead"], (": " + v["queue"]["blockedBy"]["code"]) if v["queue"].get("blockedBy") else ""), v["node"])
if not v.get("online"): line += " (offline now)"
if v.get("warmSlot"): line += " · " + v["warmSlot"].split(" ")[0]
line += (" · ~%s (p90 %s, %d runs: %s)" % (d(e["medianSecs"]), d(e["p90Secs"]), e["samples"], e["basis"])) if e else " · no estimate yet"
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % w)
for n in v.get("notices") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % (n.get("message") or n.get("code")))
sys.stderr.write(line + "\n")
p = v.get("placement") or {}
if p.get("mode") == "auto":
    sys.stderr.write("[k2 compute] would place on %s (auto): %s%s\n" % (p.get("node"), p.get("why"), (" · " + p["fits"]) if p.get("fits") else ""))
elif p.get("why") and p.get("why") != "you named this node":
    sys.stderr.write("[k2 compute] pinned to %s: %s\n" % (p.get("node"), p.get("why")))
# 0.46.7: tools on that node, the limits that can end it, the settings
# .k2/compute.toml / --preset filled in, and the runners-up.
def dur(s):
    if s is None: return None
    s = int(s)
    return "%dh" % (s // 3600) if s >= 3600 and s % 3600 == 0 else ("%dm" % (s // 60) if s >= 60 and s % 60 == 0 else "%ds" % s)
src = v.get("source")
if isinstance(src, dict):
    where = src.get("checkout") or "workspace"
    if src.get("repo") and src.get("checkout") != "worktree": where = "repo"
    if src.get("path"): where += " " + src["path"]
    s = "[k2 compute] source: %s @ %s" % (where, (src.get("commit") or "")[:8])
    if isinstance(src.get("dirty"), dict):
        s += " +dirty (%s changed, %s untracked)" % (src["dirty"].get("changed"), src["dirty"].get("untracked"))
    sys.stderr.write(s + "\n")
if v.get("toolsLine"):
    sys.stderr.write("[k2 compute] tools on %s: %s\n" % (v["node"], v["toolsLine"]))
lim = v.get("limits") or {}
ends = []
if lim.get("stallSecs"):
    ends.append("ends after %s with no output (--stall-timeout)" % dur(lim["stallSecs"]))
if lim.get("queueTimeoutSecs"):
    ends.append("fails if not started within %s (--queue-timeout)" % dur(lim["queueTimeoutSecs"]))
if lim.get("timeoutSecs"):
    ends.append("stops after %s running (--timeout)" % dur(lim["timeoutSecs"]))
g = v.get("gpu")
if isinstance(g, dict):
    gs = g.get("gpus") or []
    if gs:
        sys.stderr.write("[k2 compute] gpu: %s, %s free now (one GPU, this job alone)\n" % (" · ".join(x.get("model") or x.get("pci") for x in gs), g.get("free")))
    else:
        sys.stderr.write("[k2 compute] gpu: none on %s for this server (it would fail there)\n" % v["node"])
    m = g.get("mem")
    if isinstance(m, dict):
        # GPU jobs S6: the GPU memory budget there and what it counts.
        gib = lambda b: "%.1fG" % (b / (1 << 30))
        if m.get("budgetBytes") is None:
            s = "gpu memory: not watched (a discrete GPU; --gpu-mem sets a budget)"
        else:
            s = "gpu memory: budget %s%s" % (gib(m["budgetBytes"]), "" if m.get("watched") else " (NOT watched: an older k2-node there)")
            if m.get("countedBytes"):
                s += ", counted with --mem (unified GPU: its memory is system RAM)"
        sys.stderr.write("[k2 compute] %s\n" % s)
if ends:
    sys.stderr.write("[k2 compute] limits: %s\n" % " · ".join(ends))
eff = v.get("effective") or {}
parts = []
for k in ("os", "arch", "mem", "cpus", "needs", "env", "timeout", "stall", "tier", "cache"):
    e = eff.get(k)
    if not isinstance(e, dict) or e.get("from") in (None, "flag"):
        continue
    val = e.get("value")
    if isinstance(val, list): val = ", ".join(str(x) for x in val)
    parts.append("%s %s (%s)" % (k, val, e["from"]))
if parts:
    src = eff.get("source") or ""
    if eff.get("preset"): src += (", " if src else "") + "preset " + eff["preset"]
    sys.stderr.write("[k2 compute] from %s: %s\n" % (src, "; ".join(parts)))
hints = ((eff.get("env") or {}).get("cpuHints")) or {}
if hints:
    sys.stderr.write("[k2 compute] CPU hints: %s (the job'"'"'s CPUs; --env NAME=… overrides)\n" % " ".join("%s=%s" % kv for kv in hints.items()))
rk = v.get("ranking") or []
if len(rk) > 1:
    def one(r):
        s = r["node"] + " (" + r["state"]
        if r.get("etaSecs") is not None: s += ", ~%s %s" % (dur(r["etaSecs"]), r.get("etaBasis"))
        return s + ")"
    sys.stderr.write("[k2 compute] ranking: %s\n" % " · ".join(one(r) for r in rk))
print(json.dumps(v, indent=2))'
        exit 0
    fi
    local info
    info="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
j = v["job"]
c = (j.get("srcCommit") or "")[:8]
q = v.get("queue") or {}
if v.get("shards"):
    line = "[k2 compute] job %s: %d shards → %s" % (j["short"], len(v["shards"]), ", ".join("%s:%s" % (x["index"], x.get("node") or "?") for x in v["shards"]))
else:
    line = "[k2 compute] job %s on %s" % (j["short"], j.get("node"))
    if c: line += " · commit " + c
    line += " · " + j["state"]
    if q: line += " (position %s: %s)" % (q.get("position"), q.get("reason"))
# 0.46.8: the pinned-node hint is its own line ("[k2 compute] hint: …").
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] %s\n" % (w if w.startswith("hint: ") else "note: " + w))
# Tiers (0.46.5): courtesy notes (the job is submitted) and --supersede.
for n in v.get("notices") or []:
    sys.stderr.write("note: %s\n" % (n.get("message") or n.get("code")))
rep = v.get("replaced") or []
if rep:
    sys.stderr.write("[k2 compute] replaced %d queued job(s) with the same --supersede key: %s\n" % (len(rep), ", ".join(str(r)[:8] for r in rep)))
if j.get("tier") and j.get("tier") != "normal":
    line += " · tier " + j["tier"]
if j.get("tierAsked") and j.get("tierAsked") != j.get("tier"):
    line += " (asked %s)" % j["tierAsked"]
src = v.get("source")
if isinstance(src, dict):
    where = src.get("checkout") or "workspace"
    if src.get("repo") and src.get("checkout") != "worktree": where = "repo"
    if src.get("path"): where += " " + src["path"]
    s = "[k2 compute] source: %s @ %s" % (where, (src.get("commit") or "")[:8])
    d = src.get("dirty")
    if isinstance(d, dict):
        s += " +dirty (%s changed, %s untracked)" % (d.get("changed"), d.get("untracked"))
    sys.stderr.write(s + "\n")
# Auto-placement (0.46.6): where K2 put it and why, one line. A pinned job
# (a node named) prints none.
p = v.get("placement") or j.get("placement") or {}
placed = ""
if p.get("mode") == "auto":
    placed = "[k2 compute] placed on %s (auto): %s" % (p.get("node"), p.get("why"))
    if p.get("fits"): placed += " · " + p["fits"]
clean = lambda x: x.replace("\t", " ").replace("\n", " ")
print(j["id"] + "\t" + ("set" if v.get("shards") else "one") + "\t" + clean(line) + "\t" + clean(placed))')"
    local job_id="${info%%$'\t'*}" rest="${info#*$'\t'}"
    local kind="${rest%%$'\t'*}" rest2="${rest#*$'\t'}"
    local line="${rest2%%$'\t'*}" placed="${rest2#*$'\t'}"
    echo "$line" >&2
    if [ -n "$placed" ]; then echo "$placed" >&2; fi
    if [ "$detach" = "1" ]; then
        if [ "$as_json" = "1" ]; then
            _compute_print_result "$job_id"
        else
            echo "${job_id:0:8}"
        fi
        echo "[k2 compute] detached. k2 compute wait ${job_id:0:8} blocks until it ends (agents in a K2 tab also get a [compute] message)" >&2
        exit 0
    fi
    if [ "$wait" = "1" ]; then
        # The id first, so an agent whose shell tool times out can resume
        # with `k2 compute wait <id> --follow`. Then (0.46.6, L8) the wait
        # lines and the job's output, streamed like an attached run, ending
        # with the job's exit code; --max-wait still bounds it. --quiet
        # (and --json, whose stdout is the wait JSON) keep the exit-only
        # wait. A split run's id waits like one job (no log stream: its
        # shards each have one).
        if [ "$as_json" = "1" ]; then echo "${job_id:0:8}" >&2; else echo "${job_id:0:8}"; fi
        if [ "$wait_quiet" = "0" ] && [ "$as_json" = "0" ] && [ "$kind" != "set" ]; then
            [ -n "$max_wait" ] && _CP_DEADLINE=$(( $(date +%s) + $(_compute_secs "$max_wait") ))
            _CP_FAILURES="$wait_failures"
            _compute_follow "$job_id" "$raw" 0 0
        fi
        local -a wargs=("$job_id")
        [ "$wait_failures" = "1" ] && wargs+=(--failures)
        [ -n "$max_wait" ] && wargs+=(--max-wait "$max_wait")
        [ "$as_json" = "1" ] && wargs+=(--json)
        cmd_compute_wait "${wargs[@]}"
    fi
    if [ "$kind" = "set" ]; then
        _compute_follow_set "$job_id" "$as_json"
    fi
    _compute_follow "$job_id" "$raw" 0 "$as_json"
}

# k2 compute cache [list|clear] — this server's job caches on its nodes
# (`--cache NAME`, 0.46.6).
cmd_compute_cache() {
    local verb="list" node="" ws="" name="" all=0 as_json=0
    case "${1:-}" in
        list|ls) shift ;;
        clear|rm) verb="clear"; shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --node) node="${2:-}"; shift 2 ;;
            --workspace) ws="${2:-}"; shift 2 ;;
            --name) name="${2:-}"; shift 2 ;;
            --all) all=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 compute cache [list] [--node N] [--workspace WS] [--name NAME] [--json] | clear (--workspace WS | --all) [--name NAME] [--node N]" >&2; exit 2 ;;
        esac
    done
    if [ "$verb" = "clear" ]; then
        if [ -z "$ws" ] && [ "$all" = "0" ] && ! _compute_in_session; then
            echo "usage: k2 compute cache clear --workspace WS [--name NAME] (or --all for every workspace's caches)" >&2; exit 2
        fi
        local body
        body="$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$ws" K2_CJ_name="$name" K2_CJ_all_b="$all")"
        _compute_call POST /cli/compute/cache/clear "$body"
    else
        local q=""
        [ -n "$node" ] && q="$q&node=$(urlencode "$node")"
        [ -n "$ws" ] && q="$q&workspace=$(urlencode "$ws")"
        [ -n "$name" ] && q="$q&name=$(urlencode "$name")"
        _compute_call GET /cli/compute/cache "${q#&}"
    fi
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
    printf '%s' "$_CP_BODY" | K2_CP_VERB="$verb" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
clear = os.environ.get("K2_CP_VERB") == "clear"
def size(b):
    for u in ("B", "KB", "MB", "GB", "TB"):
        if b < 1024 or u == "TB":
            return ("%d %s" % (b, u)) if u == "B" else ("%.1f %s" % (b, u))
        b /= 1024.0
def ago(t):
    if not t: return "-"
    d = max(0, int(time.time()) - int(t))
    return "%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400))
rows = []
for n in v.get("nodes") or []:
    if n.get("error") or n.get("note") and not n.get("caches"):
        sys.stderr.write("[k2 compute] %s: %s\n" % (n["node"], n.get("note") or n.get("error")))
    for e in (n.get("cleared") or []) if clear else []:
        print("cleared  %-14s %-20s %-24s %10s" % (n["node"], e.get("workspace") or "(another: %s)" % e["wsKey"][:8], e["name"], size(e["bytes"])))
    for e in n.get("caches") or []:
        rows.append((n["node"], e.get("workspace") or "(unknown %s)" % e["wsKey"][:8], e["name"], size(e["bytes"]), ago(e.get("usedAt")), "busy" if e.get("busy") else ""))
if rows and not clear:
    print("%-14s %-20s %-24s %10s  %-9s %s" % ("NODE", "WORKSPACE", "CACHE", "SIZE", "USED", ""))
    for r in rows:
        print("%-14s %-20s %-24s %10s  %-9s %s" % r)
elif clear:
    for r in rows:
        if r[5] == "busy":
            sys.stderr.write("[k2 compute] %s: %s/%s is held by a running job; not cleared\n" % (r[0], r[1], r[2]))
elif not rows:
    sys.stderr.write("[k2 compute] no job caches\n")
'
}

# Follow a split run (--shards): one line per shard change on stderr,
# then the combined result; exit like a single job (the first failing
# shard's exit code, 3 for infra).
_compute_follow_set() {
    local job="$1" as_json="${2:-0}" seen fails=0
    seen="$(mktemp -t k2-compute-set-XXXXXX)"
    echo "{}" > "$seen"
    # shellcheck disable=SC2064
    trap "rm -f '$seen' '$seen.err'; echo '[k2 compute] detached; the shards keep running. k2 compute get $job | k2 compute cancel $job' >&2; exit 130" INT
    while :; do
        local resp
        resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$job")&wait=20")"
        _compute_split "$resp"
        if [ "$_CP_STATUS" = "000" ]; then
            fails=$((fails + 1))
            [ "$fails" -eq 1 ] && echo "[k2 compute] lost the K2 server; retrying (the shards keep running)" >&2
            if [ "$fails" -gt 60 ]; then
                rm -f "$seen"
                echo "{\"error\":\"lost_contact\",\"hint\":\"no answer from the K2 server for 10 minutes: k2 compute get $job\"}" >&2
                exit 3
            fi
            sleep 10
            continue
        fi
        [ "$_CP_STATUS" = "200" ] || { rm -f "$seen"; _compute_fail "$_CP_STATUS" "$_CP_BODY"; }
        fails=0
        local st
        st="$(printf '%s' "$_CP_BODY" | K2_CP_SEEN="$seen" python3 -c '
import json, os, re, sys
v = json.load(sys.stdin)
r = v.get("result") or {}
seen_p = os.environ["K2_CP_SEEN"]
seen = json.load(open(seen_p))
def tl(t):
    return (" · %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else ""
for x in r.get("shards") or []:
    key = "%s|%s|%s|%s" % (x["state"], x.get("attempt"), x.get("node"), x.get("exit"))
    if seen.get(str(x["index"])) != key:
        seen[str(x["index"])] = key
        what = x["state"]
        if x["state"] == "done":
            what = "exit %s" % x.get("exit")
        elif x.get("reason") and x["state"] != "running":
            what += " (%s)" % x["reason"]
        att = (" attempt %s" % x["attempt"]) if (x.get("attempt") or 1) > 1 else ""
        sys.stderr.write("[k2 compute] shard %s/%d on %s%s: %s%s\n" % (x["index"], len(r["shards"]), x.get("node") or "?", att, what, tl(x.get("tests"))))
json.dump(seen, open(seen_p, "w"))
term = v.get("state") in ("done", "failed", "cancelled", "timeout", "interrupted", "unknown")
if term:
    t = r.get("tests")
    took = (" after %dm%02ds" % (r["durationSecs"] // 60, r["durationSecs"] % 60)) if r.get("durationSecs") is not None else ""
    if v["state"] == "done":
        sys.stderr.write("[k2 compute] %d shards: exit %s%s%s · k2 compute logs %s --failures\n" % (len(r.get("shards") or []), r.get("exit"), took, (" · tests %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else "", v["short"]))
    else:
        err = {"error": v["state"], "reason": v.get("reason"), "detail": v.get("detail"), "hint": "k2 compute get %s --json; k2 compute retry %s" % (v["short"], v["short"])}
        open(seen_p + ".err", "w").write(json.dumps(err) + "\n")
def s(x):
    return re.sub(r"[^A-Za-z0-9_.:-]", "", str(x if x is not None else ""))
print("|".join(["1" if term else "0", s(v.get("state")), s(r.get("exit")), s(r.get("signal"))]))')"
        local end state code sig
        IFS='|' read -r end state code sig <<<"$st"
        if [ "$end" = "1" ]; then
            trap - INT
            [ "$as_json" = "1" ] && _compute_print_result "$job"
            if [ "$state" = "done" ]; then
                rm -f "$seen" "$seen.err"
                [ -n "$code" ] && exit "$code"
                exit $((128 + ${sig:-0}))
            fi
            cat "$seen.err" >&2
            rm -f "$seen" "$seen.err"
            exit 3
        fi
    done
}

# k2 compute test [<node>] [--shards N] [--partition hash|count]
#   [--profile P] [--runner "cargo nextest run"] [run flags] [-- nextest args]
# One command for a cargo-nextest suite: a nextest partition per shard,
# JUnit kept and parsed, one combined pass/fail.
cmd_compute_test() {
    local node="" any=0 nshards=1 partition="hash" profile="" runner="cargo nextest run" has_nextest_need=0
    local -a pass=() targs=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; targs=("$@"); break ;;
            --any) any=1; shift ;;
            --shards) nshards="${2:-}"; shift 2 ;;
            --partition) partition="${2:-}"; shift 2 ;;
            --profile) profile="${2:-}"; shift 2 ;;
            --runner) runner="${2:-}"; shift 2 ;;
            --needs|--need)
                case "${2:-}" in *cargo-nextest*) has_nextest_need=1 ;; esac
                pass+=("$1" "${2:-}"); shift 2 ;;
            --os|--distro|--arch|--label|--env|--secret|--cpus|--mem|--timeout|--sha|--workspace|--artifact|--artifacts|--queue-timeout|--retries|--stall-timeout|--tier|--why|--supersede|--submitter|--repo|--cache|--with|--preset|--tag|--same-node-as|--avoid|--only)
                pass+=("$1" "${2:-}"); shift 2 ;;
            --detach|-d|--dirty|--exclusive|--receipt-v2|--raw|--json|--no-retry|--retry-interrupted|--no-source|--gate|--critical|--now|--plan) pass+=("$1"); shift ;;
            --gpu=*) pass+=("$1"); shift ;;
            --gpu)
                case "${2:-}" in
                    ''|-*) pass+=("$1"); shift ;;
                    *) pass+=("$1" "$2"); shift 2 ;;
                esac ;;
            --gpu-mem) pass+=("$1" "${2:-}"); shift 2 ;;
            -*) echo "usage: unknown flag '$1' (k2 compute test --help)" >&2; exit 2 ;;
            *) if [ -z "$node" ]; then node="$1"; shift; else echo "usage: put nextest's own arguments after --" >&2; exit 2; fi ;;
        esac
    done
    case "$nshards" in ''|*[!0-9]*) echo "usage: --shards N (1–16)" >&2; exit 2 ;; esac
    case "$partition" in hash|count) ;; *) echo "usage: --partition hash|count" >&2; exit 2 ;; esac
    if [ "$nshards" -gt 1 ] && [ -n "$node" ]; then
        echo "usage: --shards picks a node for each shard itself (no node: K2 picks); drop the node name" >&2
        exit 2
    fi
    [ -z "$node" ] && any=1
    local script
    script="$(K2_CT_RUNNER="$runner" K2_CT_PROFILE="$profile" K2_CT_SHARDS="$nshards" K2_CT_PART="$partition" python3 -c '
import os, shlex, sys
runner = shlex.split(os.environ["K2_CT_RUNNER"])
profile = os.environ["K2_CT_PROFILE"]
shards = int(os.environ["K2_CT_SHARDS"])
lines = [
    "set -u",
    # No K2 session variable reaches a test (the prod-isolation guard).
    "for v in $(env | sed -n \"s/^\\(K2[A-Z0-9_]*\\)=.*/\\1/p\"); do unset \"$v\"; done",
    "export XDG_RUNTIME_DIR=\"${TMPDIR:-/tmp}/k2-xdg\"; mkdir -p \"$XDG_RUNTIME_DIR\"; chmod 700 \"$XDG_RUNTIME_DIR\"",
]
cmd = [shlex.quote(w) for w in runner]
if profile:
    cmd += ["--profile", shlex.quote(profile)]
else:
    lines.append("cfg=\"${TMPDIR:-/tmp}/k2-nextest.toml\"")
    lines.append("printf \"%s\\n\" \"[profile.k2]\" \"fail-fast = false\" \"[profile.k2.junit]\" \"path = \\\"junit.xml\\\"\" > \"$cfg\"")
    cmd += ["--config-file", "\"$cfg\"", "--profile", "k2"]
if shards > 1:
    cmd += ["--partition", "%s:{shard}/{shards}" % os.environ["K2_CT_PART"], "--no-tests=pass"]
cmd += [shlex.quote(a) for a in sys.argv[1:]]
lines.append("exec " + " ".join(cmd))
print("\n".join(lines))' "${targs[@]+"${targs[@]}"}")" || exit 2
    local -a run_args=()
    if [ "$any" = "1" ]; then run_args+=(--any); else run_args+=("$node"); fi
    [ "$nshards" -gt 1 ] && run_args+=(--shards "$nshards")
    if [ "$has_nextest_need" = "0" ] && [ "$runner" = "cargo nextest run" ]; then
        run_args+=(--needs cargo-nextest)
    fi
    run_args+=(--artifact 'target/nextest/**/*.xml')
    cmd_compute_run "${run_args[@]}" "${pass[@]+"${pass[@]}"}" -- sh -c "$script"
}

# k2 compute prewarm [--preset P | --needs …] [--os …] [--arch …] [--label k=v]
# [--node N]... [--tag T] [--plan] [--wait] [--json] (0.46.7, CE-18): the
# daemon submits one Batch `sh -c true` job with the needs to each granted
# matching node where a need isn't ready, and reports the ready ones.
cmd_compute_prewarm() {
    local preset="" os="" arch="" tag="" workspace="" plan=0 wait=0 as_json=0 distro=""
    local -a needs=() nodes=() labels=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --preset) preset="${2:-}"; shift 2 ;;
            --needs|--need) needs+=("${2:-}"); shift 2 ;;
            --os) os="${2:-}"; shift 2 ;;
            --distro) distro="$(_compute_distro_arg "${2:-}")" || exit 2; shift 2 ;;
            --arch) arch="${2:-}"; shift 2 ;;
            --label) labels+=("${2:-}"); shift 2 ;;
            --node) nodes+=("${2:-}"); shift 2 ;;
            --tag) tag="${2:-}"; shift 2 ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            --plan|--dry-run) plan=1; shift ;;
            --wait) wait=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 compute prewarm [--preset NAME | --needs rust@stable,cmake@3] [--os linux|macos] [--distro ubuntu@24.04] [--arch A] [--label k=v] [--node N]... [--tag T] [--plan] [--wait] [--json] (unknown '$1')" >&2; exit 2 ;;
        esac
    done
    # No --preset or --needs: the needs come from .k2/compute.toml's
    # [defaults] (the daemon says so when there are none).
    if [ "$plan" = "1" ] && [ "$wait" = "1" ]; then
        echo "usage: --plan submits nothing, so there is nothing to --wait for" >&2; exit 2
    fi
    local needs_json="" nodes_json="" labels_json="" repo_dir=""
    if [ "${#needs[@]}" -gt 0 ]; then
        needs_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${needs[@]}")"
    fi
    [ "${#nodes[@]}" -gt 0 ] && nodes_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${nodes[@]}")"
    if [ "${#labels[@]}" -gt 0 ]; then
        labels_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --label key=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${labels[@]}")" || exit 2
    fi
    # The daemon reads .k2/compute.toml from this checkout (or the workspace).
    _k2_git_usable && repo_dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"
    local body
    body="$(_compute_json K2_CJ_preset="$preset" K2_CJ_needs_j="$needs_json" K2_CJ_os="$os" K2_CJ_distro="$distro" K2_CJ_arch="$arch" \
        K2_CJ_labels_j="$labels_json" K2_CJ_nodes_j="$nodes_json" K2_CJ_tag="$tag" K2_CJ_plan_b="${plan#0}" \
        K2_CJ_repoDir="$repo_dir" K2_CJ_workspace="$(_compute_ws "$workspace")")"
    _compute_call POST /cli/compute/prewarm "$body"
    if [ "$as_json" = "1" ] && [ "$wait" = "0" ]; then
        printf '%s\n' "$_CP_BODY"; exit 0
    fi
    local ids
    ids="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("nodes") or []:
    st = r.get("state")
    if st == "ready":
        s = "ready already"
    elif st == "installing":
        s = "installing %s (job %s)" % (", ".join(r.get("installs") or []), r.get("short"))
    elif st == "would_install":
        s = "would install %s" % ", ".join(r.get("installs") or [])
    elif st in ("asked", "would_ask"):
        # 0.46.9: system packages its owner must approve first.
        s = "%s its owner for %s" % ("asked" if st == "asked" else "would ask", ", ".join(r.get("ask") or []))
    elif st == "cant_install":
        s = "cannot install there"
    else:
        e = r.get("error") or {}
        s = "refused: %s: %s" % (e.get("code"), e.get("hint"))
    if r.get("note"): s += " · " + r["note"]
    sys.stderr.write("[k2 compute] %s: %s\n" % (r.get("node"), s))
if v.get("fits"): sys.stderr.write("[k2 compute] %s\n" % v["fits"])
rq = v.get("request")
if isinstance(rq, dict) and rq.get("id"):
    sys.stderr.write("[k2 compute] package request %s: %s. Prewarm again once it is installed (k2 compute node packages requests)\n" % (rq["id"], rq.get("summary") or ""))
elif isinstance(rq, dict) and isinstance(rq.get("error"), dict):
    sys.stderr.write("[k2 compute] package request not filed: %s: %s\n" % (rq["error"].get("code"), rq["error"].get("hint")))
print(" ".join(v.get("jobs") or []))')"
    if [ "$wait" = "1" ] && [ -n "$ids" ]; then
        local -a wargs=()
        read -r -a wargs <<< "$ids"
        [ "$as_json" = "1" ] && wargs+=(--json)
        cmd_compute_wait "${wargs[@]}"
    fi
    [ "$as_json" = "1" ] && printf '%s\n' "$_CP_BODY"
    # 0.46.9: a package request it filed for this server's owner is a hold.
    _compute_pkg_held_exit
    exit 0
}

# k2 compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay]:
# copy kept artifacts from the node (through this server) into DIR
# (default .), keeping their relative names; every file is checked
# against the sha256 the node reported. --over-relay: pull files over
# the 100 MB relay cap through the relay anyway (up to 500 MB). The
# server pulls a big file in the background; this asks again every
# 20 s until it's in.
cmd_compute_fetch() {
    local job="" out="." as_json=0 over_relay=0
    local -a names=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --out|-o) out="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            --over-relay) over_relay=1; shift ;;
            -*) echo "usage: k2 compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay]" >&2; exit 2 ;;
            *) if [ -z "$job" ]; then job="$1"; else names+=("$1"); fi; shift ;;
        esac
    done
    [ -n "$job" ] && [ -n "$out" ] || { echo "usage: k2 compute fetch <job> [NAME...] [--out DIR] [--json] [--over-relay]" >&2; exit 2; }
    _compute_call GET /cli/compute/jobs/artifact "job=$(urlencode "$job")"
    local list
    list="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
want = sys.argv[1:]
have = {a["name"]: a for a in v.get("artifacts") or []}
missing = [w for w in want if w not in have]
if missing:
    sys.stderr.write(json.dumps({"error": "artifact_not_found", "hint": "job %s kept: %s" % (v.get("short"), ", ".join(sorted(have)) or "(nothing)")}) + "\n")
    sys.exit(3)
for n in (want or sorted(have)):
    print(n)' "${names[@]+"${names[@]}"}")" || exit 3
    mkdir -p "$out" || { echo "{\"error\":\"local_write\",\"hint\":\"can't create $out\"}" >&2; exit 3; }
    local results="[]" name
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        local offset=0 eof=0 first=1
        while [ "$eof" != "1" ]; do
            _compute_call GET /cli/compute/jobs/artifact "job=$(urlencode "$job")&name=$(urlencode "$name")&offset=$offset&max=4194304&wait=20&over_relay=$over_relay"
            local step
            step="$(printf '%s' "$_CP_BODY" | K2_CP_OUT="$out" K2_CP_FIRST="$first" python3 -c '
import base64, json, os, sys
v = json.load(sys.stdin)
if v.get("pending"):
    # Still on its way from the node: ask again.
    sys.stderr.write("%s: %d of %d MB on the server so far\n" % (v["name"], v.get("got", 0) >> 20, v["bytes"] >> 20))
    print("pending")
    sys.exit(0)
name = v["name"]
parts = name.split("/")
if name.startswith("/") or any(p in ("", ".", "..") for p in parts):
    sys.stderr.write(json.dumps({"error": "artifact_name_unsafe", "hint": name}) + "\n"); sys.exit(3)
path = os.path.join(os.environ["K2_CP_OUT"], *parts)
os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
data = base64.b64decode(v["data"])
with open(path, "wb" if os.environ.get("K2_CP_FIRST") == "1" else "ab") as f:
    f.write(data)
print("%d %d %s %s %s" % (v["offset"] + len(data), 1 if v["eof"] else 0, v["sha256"], v["bytes"], path))')" || exit 3
            [ "$step" = "pending" ] && continue
            local sha bytes path
            read -r offset eof sha bytes path <<<"$step"
            first=0
        done
        results="$(K2_CP_PATH="$path" K2_CP_SHA="$sha" K2_CP_BYTES="$bytes" K2_CP_NAME="$name" python3 -c '
import hashlib, json, os, sys
acc = json.loads(sys.argv[1])
p = os.environ["K2_CP_PATH"]
h = hashlib.sha256(open(p, "rb").read()).hexdigest()
if h != os.environ["K2_CP_SHA"] or os.path.getsize(p) != int(os.environ["K2_CP_BYTES"]):
    sys.stderr.write(json.dumps({"error": "artifact_changed", "hint": "%s does not match the sha256 the node reported; fetch it again" % p}) + "\n")
    sys.exit(3)
acc.append({"name": os.environ["K2_CP_NAME"], "path": p, "bytes": int(os.environ["K2_CP_BYTES"]), "sha256": h})
print(json.dumps(acc))' "$results")" || exit 3
    done <<<"$list"
    if [ "$as_json" = "1" ]; then
        printf '%s' "$results" | K2_CP_JOB="$job" python3 -c 'import json,os,sys; print(json.dumps({"job": os.environ["K2_CP_JOB"], "files": json.load(sys.stdin)}, indent=2))'
    else
        printf '%s' "$results" | python3 -c '
import json, sys
fs = json.load(sys.stdin)
if not fs: print("(no artifacts kept for this job)")
for f in fs: print("%s  %d bytes  sha256 %s…" % (f["path"], f["bytes"], f["sha256"][:12]))'
    fi
}

cmd_compute_logs() {
    local job="" tail="" failures=0 raw=0 no_tests=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tail) tail="${2:-}"; shift 2 ;;
            --failures) failures=1; shift ;;
            --raw) raw=1; shift ;;
            --no-tests) no_tests=1; shift ;;
            -*) echo "usage: k2 compute logs <job> [--tail N] [--failures] [--raw]" >&2; exit 2 ;;
            *) job="$1"; shift ;;
        esac
    done
    [ -n "$job" ] || { echo "usage: k2 compute logs <job> [--tail N] [--failures] [--raw]" >&2; exit 2; }
    if [ "$failures" = "1" ] && [ "$no_tests" = "0" ]; then
        # The structured list (JUnit the node parsed) first; program
        # output, never instructions.
        _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$job")"
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin).get("result") or {}
t = r.get("tests")
if not t:
    sys.exit(0)
print("FAILING TESTS: %d failed, %d errors, %d passed, %d skipped (of %d)" % (t["failed"], t["errors"], t["passed"], t["skipped"], t["total"]))
for f in t.get("failing") or []:
    where = ("[shard %d] " % f["shard"]) if f.get("shard") else ""
    cls = (f.get("classname") + " ") if f.get("classname") else ""
    msg = (f.get("message") or "").splitlines()
    print("  %s%s%s%s" % (where, cls, f["name"], (" — " + msg[0][:160]) if msg else ""))
if t.get("truncated"):
    print("  … more failures than listed (k2 compute fetch the JUnit file)")
print("LOG LINES:")'
    fi
    # A split run has no log of its own: each shard's, with a header.
    _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$job")"
    local shard_lines
    shard_lines="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin).get("result") or {}
for x in r.get("shards") or []:
    print("%s\t%s/%d · job %s on %s · %s%s" % (x["job"], x["index"], len(r["shards"]), x["short"], x.get("node") or "?", x["state"], (" exit %s" % x["exit"]) if x.get("exit") is not None else ""))')"
    if [ -n "$shard_lines" ]; then
        local sid sdesc
        while IFS=$'\t' read -r sid sdesc; do
            echo "── shard $sdesc ──"
            local -a sub=("$sid" --no-tests)
            [ -n "$tail" ] && sub+=(--tail "$tail")
            [ "$failures" = "1" ] && sub+=(--failures)
            [ "$raw" = "1" ] && sub+=(--raw)
            ( cmd_compute_logs "${sub[@]}" )
        done <<<"$shard_lines"
        return 0
    fi
    local tmp cursor=0
    tmp="$(mktemp -t k2-compute-logs-XXXXXX)"
    : > "$tmp"
    while :; do
        _compute_call GET /cli/compute/jobs/logs "job=$(urlencode "$job")&cursor=$cursor&max=1048576"
        local next
        next="$(printf '%s' "$_CP_BODY" | K2_CP_OUT="$tmp" python3 -c '
import base64, json, os, sys
v = json.load(sys.stdin)
with open(os.environ["K2_CP_OUT"], "ab") as f:
    for c in v.get("chunks", []):
        f.write(base64.b64decode(c["data"]))
print("%s %d" % (v.get("cursor", 0), len(v.get("chunks", []))))')"
        local n="${next#* }"
        cursor="${next% *}"
        [ "$n" = "0" ] && break
    done
    K2_CP_RAW="$raw" K2_CP_TAIL="$tail" K2_CP_FAIL="$failures" python3 -c '
import os, re, sys
data = open(sys.argv[1], "rb").read()
if os.environ.get("K2_CP_RAW") != "1":
    data = re.sub(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", b"", data)
lines = data.decode("utf-8", "replace").splitlines()
if os.environ.get("K2_CP_FAIL") == "1":
    pat = re.compile(r"(FAIL|FAILED|panicked at|^error(\[E\d+\])?:|^failures:|test result: FAILED|assertion|--- STDERR)")
    lines = [l for l in lines if pat.search(l)]
t = os.environ.get("K2_CP_TAIL")
if t:
    lines = lines[-int(t):]
sys.stdout.write("\n".join(lines) + ("\n" if lines else ""))' "$tmp"
    rm -f "$tmp"
}

# k2 compute wait <job>... [--failures] [--max-wait D] [--quiet] [--json]
# Blocks until every job ends; exits like an attached run's end (the job's
# exit code, 128+signal, or 3 for cancelled/failed/timeout/interrupted/
# unknown). --max-wait: exit 3 + still_running when D passes first.
cmd_compute_wait() {
    local failures=0 quiet=0 max_wait="" as_json=0 follow=0 any=0
    local -a jobs=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --failures) failures=1; shift ;;
            --quiet|-q) quiet=1; shift ;;
            --json) as_json=1; shift ;;
            --follow|-f) follow=1; shift ;;
            --max-wait) max_wait="${2:-}"; shift 2 ;;
            --any) any=1; shift ;;
            -*) echo "usage: k2 compute wait <job>... [--any] [--follow] [--failures] [--max-wait 9m] [--quiet] [--json]" >&2; exit 2 ;;
            *) jobs+=("$1"); shift ;;
        esac
    done
    [ "${#jobs[@]}" -gt 0 ] || { echo "usage: k2 compute wait <job>... [--any] [--follow] [--failures] [--max-wait 9m] [--quiet] [--json]" >&2; exit 2; }
    local deadline=""
    if [ -n "$max_wait" ]; then
        deadline=$(( $(date +%s) + $(_compute_secs "$max_wait") ))
    fi
    # --follow (0.46.6, L8): stream one job's output from where it is now,
    # like run --wait (resuming after a shell timeout).
    if [ "$follow" = "1" ]; then
        if [ "${#jobs[@]}" -ne 1 ] || [ "$as_json" = "1" ] || [ "$quiet" = "1" ]; then
            echo "usage: k2 compute wait <job> --follow [--failures] [--max-wait 9m]   (one job; no --json or --quiet)" >&2; exit 2
        fi
        _CP_DEADLINE="$deadline"
        _CP_FAILURES="$failures"
        _compute_follow "${jobs[0]}" 0 0 0
    fi
    # --any (0.46.7, CE-21): return when the FIRST of the jobs ends, with
    # its exit code (for schedulers); the others keep running.
    if [ "$any" = "1" ]; then
        [ "$follow" = "1" ] && { echo "usage: k2 compute wait <job>... --any (not with --follow)" >&2; exit 2; }
        _compute_wait_any "$deadline" "$quiet" "$as_json" "$failures" "${jobs[@]}"
    fi
    local dir n=0 j
    dir="$(mktemp -d -t k2-compute-wait-XXXXXX)"
    for j in "${jobs[@]}"; do
        local fails=0 last="" w resp left info
        while :; do
            w=20
            if [ -n "$deadline" ]; then
                left=$(( deadline - $(date +%s) ))
                [ "$left" -lt 0 ] && left=0
                [ "$left" -lt "$w" ] && w="$left"
            fi
            # One throttled long poll (≤ 20 s) per round; it also tells the
            # daemon someone follows the job (no duplicate [compute] line).
            resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$j")&wait=$w")"
            _compute_split "$resp"
            if [ "$_CP_STATUS" = "000" ]; then
                fails=$((fails + 1))
                [ "$fails" -eq 1 ] && echo "[k2 compute] lost the K2 server; retrying (the job keeps running on the node)" >&2
                if [ "$fails" -gt 60 ]; then
                    rm -rf "$dir"
                    echo "{\"error\":\"lost_contact\",\"hint\":\"no answer from the K2 server for 10 minutes; the job may still be running: k2 compute wait $j\"}" >&2
                    exit 3
                fi
                sleep 10
                continue
            fi
            if [ "$_CP_STATUS" != "200" ]; then
                rm -rf "$dir"
                _CP_404_EXIT=4 _compute_fail "$_CP_STATUS" "$_CP_BODY"
            fi
            fails=0
            printf '%s' "$_CP_BODY" > "$dir/$n.json"
            info="$(python3 -c '
import json, sys
j = json.load(open(sys.argv[1]))
q = j.get("queue") or {}
wt = j.get("wait") or {}
st = j.get("state") or "?"
key = "%s|%s|%s" % (st, q.get("position"), q.get("waitReason") or q.get("reason"))
term = st in ("done", "failed", "cancelled", "timeout", "interrupted", "unknown")
if st == "queued" and q.get("line"):
    line = "%s %s" % (j.get("short"), q["line"])
elif st == "queued" and wt.get("words"):
    line = "%s queued: %s" % (j.get("short"), wt["words"])
elif st == "queued" and q:
    line = "%s queued on %s: position %s%s (%s)" % (j.get("short"), j.get("node"), q.get("position"),
        (" of %s" % q["of"]) if q.get("of") else "", q.get("reason"))
else:
    line = "%s %s on %s" % (j.get("short"), st, j.get("node"))
print("%d\t%s\t%s" % (1 if term else 0, key, line))' "$dir/$n.json")"
            local term="${info%%$'\t'*}" rest="${info#*$'\t'}"
            local key="${rest%%$'\t'*}" line="${rest#*$'\t'}"
            if [ "$term" = "1" ]; then break; fi
            if [ "$key" != "$last" ] && [ "$quiet" = "0" ]; then
                echo "[k2 compute] $line" >&2
            fi
            last="$key"
            if [ -n "$deadline" ] && [ "$(date +%s)" -ge "$deadline" ]; then break; fi
        done
        n=$((n + 1))
    done
    local out rc
    out="$(K2_CP_JSON="$as_json" K2_CP_N="$n" python3 -c '
import json, os, sys
d = sys.argv[1]
n = int(os.environ["K2_CP_N"])
bodies = [json.load(open(os.path.join(d, "%d.json" % i))) for i in range(n)]
term = ("done", "failed", "cancelled", "timeout", "interrupted", "unknown")
rc = None
unfinished = []
errs = []
failed_ids = []
for j in bodies:
    st = j.get("state")
    if st not in term:
        unfinished.append(j.get("short"))
        continue
    a, b = j.get("startedAt") or j.get("assignedAt"), j.get("endedAt")
    took = (" after %dm%02ds" % ((b - a) // 60, (b - a) % 60)) if a and b else ""
    rl = j.get("reportedLate") or {}
    if rl.get("secs"):
        # CE-5: the times of the node itself; this server heard of the end late.
        took += " (node time; this server heard %dm late)" % (rl["secs"] // 60)
    if st == "done" and j.get("exitCode") is not None:
        code, what = int(j["exitCode"]), "exit %s" % j["exitCode"]
    elif st == "done":
        code, what = 128 + int(j.get("signal") or 0), "killed by signal %s" % j.get("signal")
    else:
        code, what = 3, st + ((" (%s)" % j["reason"]) if j.get("reason") else "")
        errs.append(json.dumps({"error": st, "reason": j.get("reason") or "", "hint": "k2 compute get %s; k2 compute retry %s" % (j.get("short"), j.get("short"))}))
    sys.stderr.write("[k2 compute] job %s on %s: %s%s\n" % (j.get("short"), j.get("node"), what, took))
    if code != 0:
        failed_ids.append(j.get("id"))
        if rc is None:
            rc = code
for e in errs:
    sys.stderr.write(e + "\n")
if unfinished:
    sys.stderr.write(json.dumps({"error": "still_running", "jobs": unfinished, "hint": "k2 compute wait " + " ".join(unfinished)}) + "\n")
    rc = 3
if os.environ.get("K2_CP_JSON") == "1":
    print(json.dumps(bodies))
print("RC %d %s" % (rc or 0, " ".join(i for i in failed_ids if i)))' "$dir")"
    rm -rf "$dir"
    local tail_line="${out##*$'\n'}"
    [ "$tail_line" = "$out" ] && out="" || out="${out%$'\n'*}"
    [ -n "$out" ] && printf '%s\n' "$out"
    rc="$(printf '%s' "$tail_line" | cut -d' ' -f2)"
    if [ "$failures" = "1" ]; then
        local fid
        for fid in $(printf '%s' "$tail_line" | cut -d' ' -f3-); do
            echo "── failures in ${fid:0:8} (k2 compute logs ${fid:0:8} --failures --tail 60) ──"
            ( cmd_compute_logs "$fid" --failures --tail 60 ) || true
        done
    fi
    exit "${rc:-3}"
}

# wait --any (CE-21): one short poll per job per round (wait=1, so the
# daemon counts each as followed: no duplicate [compute] line), until the
# first ends. Prints that job's line (JSON: its get body) and exits with
# its code; --max-wait past: still_running, exit 3.
_compute_wait_any() {
    local deadline="$1" quiet="$2" as_json="$3" failures="$4"
    shift 4
    local j resp fails=0 last=""
    while :; do
        for j in "$@"; do
            resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$j")&wait=1")"
            _compute_split "$resp"
            if [ "$_CP_STATUS" = "000" ]; then
                fails=$((fails + 1))
                [ "$fails" -eq 1 ] && echo "[k2 compute] lost the K2 server; retrying (the jobs keep running on the nodes)" >&2
                [ "$fails" -gt 60 ] && { echo '{"error":"lost_contact","hint":"no answer from the K2 server for 10 minutes; the jobs may still be running"}' >&2; exit 3; }
                sleep 10
                continue 2
            fi
            [ "$_CP_STATUS" = "200" ] || _CP_404_EXIT=4 _compute_fail "$_CP_STATUS" "$_CP_BODY"
            fails=0
            local out
            out="$(printf '%s' "$_CP_BODY" | K2_CP_JSON="$as_json" python3 -c '
import json, os, sys
j = json.load(sys.stdin)
st = j.get("state")
if st not in ("done", "failed", "cancelled", "timeout", "interrupted", "unknown"):
    print("RUNNING"); sys.exit(0)
a, b = j.get("startedAt") or j.get("assignedAt"), j.get("endedAt")
took = (" after %dm%02ds" % ((b - a) // 60, (b - a) % 60)) if a and b else ""
if st == "done" and j.get("exitCode") is not None:
    code, what = int(j["exitCode"]), "exit %s" % j["exitCode"]
elif st == "done":
    code, what = 128 + int(j.get("signal") or 0), "killed by signal %s" % j.get("signal")
else:
    code, what = 3, st + ((" (%s)" % j["reason"]) if j.get("reason") else "")
sys.stderr.write("[k2 compute] job %s on %s ended first: %s%s\n" % (j.get("short"), j.get("node"), what, took))
if os.environ.get("K2_CP_JSON") == "1":
    print(json.dumps(j))
print("RC %d %s" % (code, j.get("id") or ""))')"
            [ "$out" = "RUNNING" ] && continue
            local tail_line="${out##*$'\n'}"
            [ "$tail_line" != "$out" ] && printf '%s\n' "${out%$'\n'*}"
            local rc fid
            rc="$(printf '%s' "$tail_line" | cut -d' ' -f2)"
            fid="$(printf '%s' "$tail_line" | cut -d' ' -f3)"
            local others=()
            local o
            for o in "$@"; do [ "$o" = "$j" ] || others+=("$o"); done
            [ "${#others[@]}" -gt 0 ] && [ "$quiet" = "0" ] && echo "[k2 compute] still running: ${others[*]} (k2 compute wait ${others[*]} --any)" >&2
            if [ "$failures" = "1" ] && [ "${rc:-0}" != "0" ] && [ -n "$fid" ]; then
                echo "── failures in ${fid:0:8} (k2 compute logs ${fid:0:8} --failures --tail 60) ──"
                ( cmd_compute_logs "$fid" --failures --tail 60 ) || true
            fi
            exit "${rc:-3}"
        done
        if [ -n "$deadline" ] && [ "$(date +%s)" -ge "$deadline" ]; then
            echo "{\"error\":\"still_running\",\"jobs\":\"$*\",\"hint\":\"none ended yet: k2 compute wait $* --any\"}" >&2
            exit 3
        fi
        [ "$quiet" = "0" ] && [ -z "$last" ] && { echo "[k2 compute] waiting for the first of $# job(s) to end" >&2; last=1; }
    done
}

# k2 compute status [--json]: one block per node (state, waits such as a
# foreign lock, run/queue, free CPU/memory under the caps, load, disk free
# against the floor, the jobs running and queued) plus this controller
# machine (disk, Time Machine local snapshots). Only what the controller
# already knows; hand-run lock files, cache sizes and pinned long-runners
# need node reporting, which nodes don't send yet.
cmd_compute_status() {
    local as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            *) echo "usage: k2 compute status [--json]   (k2 compute status <job> = get)" >&2; exit 2 ;;
        esac
    done
    local q="detail=1" sws
    sws="$(_compute_ws "")"
    [ -n "$sws" ] && q="$q&workspace=$(urlencode "$sws")"
    _compute_call GET /cli/compute/nodes "$q"
    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
def gb(b):
    return "-" if b is None else "%.0f GB" % (b / (1 << 30))
def cores(m):
    return "%g" % round((m or 0) / 1000, 1)
def mmss(s):
    return "-" if s is None else ("%dm%02ds" % (s // 60, s % 60) if s < 3600 else "%dh%02dm" % (s // 3600, (s % 3600) // 60))
rows = v.get("nodes") or []
if not rows:
    print("(no compute nodes)")
for n in rows:
    st = n.get("status") or {}
    parts = ["%-14s" % n.get("name"), "%s-%s" % (n.get("os") or "?", n.get("arch") or "?"), st.get("state") or "?"]
    if st.get("controllerPause"):
        parts.append("(server: %s)" % st["controllerPause"])
    w = st.get("waiting")
    if w:
        parts.append("held: %s (%s)" % (w.get("code"), w.get("message")))
    if st.get("unavailable"):
        parts.append("unavailable: " + ",".join(st["unavailable"]))
    # 0.46.7 (CE-7): the same counts as `nodes`, and whose they are.
    mine = n.get("mine") if isinstance(n.get("mine"), dict) else None
    parts.append("run %d/%s%s" % (len(st.get("running") or []), st.get("parallelMax") if st.get("parallelMax") is not None else "?", (" (yours %d)" % mine.get("running", 0)) if mine else ""))
    parts.append("queued %d%s" % (len(st.get("queued") or []), (" (yours %d)" % mine.get("queued", 0)) if mine else ""))
    if n.get("nodeRunning") is not None and n["nodeRunning"] != len(st.get("running") or []):
        parts.append("node runs %d" % n["nodeRunning"])
    f, c = st.get("free") or {}, st.get("caps") or {}
    if f:
        # CE-8: unreserved (not reserved by K2 jobs) beside idle.
        idle = (" · idle ~%s" % cores(st["idleCpuMillis"])) if st.get("idleCpuMillis") is not None else ""
        parts.append("cpu %s/%s unreserved%s" % (cores(f.get("cpuMillis")), cores(c.get("cpuMillis")), idle))
        parts.append("mem %.1f/%s free" % ((f.get("memBytes") or 0) / (1 << 30), gb(c.get("memBytes"))))
    if st.get("load"):
        parts.append("load " + " ".join("%.1f" % x for x in st["load"]))
    d = st.get("disk") or {}
    if d.get("freeBytes") is not None:
        s = "disk %s free" % gb(d["freeBytes"])
        if d.get("floorBytes") is not None:
            s += " (floor %s%s)" % (gb(d["floorBytes"]), ", default" if d.get("floorFrom") == "default" else "")
        if d.get("belowFloor"):
            s += " BELOW FLOOR"
        parts.append(s)
    print("  ".join(parts))
    if (n.get("share") or {}).get("line"):
        print("    your share: " + n["share"]["line"])
    # 0.46.6: the line tag (CI, P, B) and the --why of a Priority job.
    tag = lambda j: ({"ci": "CI ", "priority": "P  ", "batch": "B  "}.get(j.get("tier") or "", ""))
    pwhy = lambda j: ("\n      why: %s%s" % (j["tierWhy"], (" (%s)" % j["submitter"]) if j.get("submitter") else "")) if j.get("tierWhy") else ""
    # 0.46.8: a running job with no --cpus holds the node CPUs ÷ its slots.
    capc, parm = (st.get("caps") or {}).get("cpuMillis") or 0, st.get("parallelMax") or 0
    def held(j):
        if "cpuMillis" not in j or j["cpuMillis"] is not None or not capc or parm < 2:
            return ""
        return "  [no --cpus: reserves %s of %s CPUs]" % (cores(capc // parm), cores(capc))
    for j in st.get("running") or []:
        print("    running %s%-9s %-8s %-12s %s%s%s" % (tag(j), j.get("short") or "-", mmss(j.get("elapsedSecs")), j.get("workspace") or "(other)", j.get("argv") or "", held(j), pwhy(j)))
    # 0.46.8: pinned (a node was named: it never moves; k2 compute move
    # <id> --auto releases it) or auto (K2 moves it if another node frees).
    pin = lambda j: ("pinned" if j["pinned"] else "auto") if "pinned" in j else ""
    for j in st.get("queued") or []:
        print("    queued  %s%-9s #%-3s %-8s %-6s %-12s %s%s%s" % (tag(j), j.get("short") or "-", j.get("position") or "?", mmss(j.get("ageSecs")), pin(j), j.get("workspace") or "(other)",
            j.get("argv") or "", "  (" + j["reason"] + ")" if j.get("reason") else "", pwhy(j)))
    if any(j.get("pinned") for j in st.get("queued") or []):
        print("      pinned jobs wait for this node only: k2 compute move <id> --auto lets K2 move them")
m = v.get("controllerMachine") or {}
if m:
    s = "controller %s: disk %s free of %s" % (m.get("name"), gb(m.get("diskFreeBytes")), gb(m.get("diskTotalBytes")))
    if m.get("tmLocalSnapshots") is not None:
        s += " · Time Machine local snapshots: %d" % m["tmLocalSnapshots"]
    elif m.get("tmLocalSnapshotsError"):
        s += " · Time Machine local snapshots: unknown (%s)" % m["tmLocalSnapshotsError"]
    print(s)
if v.get("notYet"):
    print("not yet: " + v["notYet"])'
    exit 0
}

cmd_compute_node() {
    local sub="${1:-}"
    shift 2>/dev/null || true
    local body
    case "$sub" in
        add)
            local name="" url="" add_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --url) url="${2:-}"; shift 2 ;;
                    --json) add_json=1; shift ;;
                    *) name="$1"; shift ;;
                esac
            done
            [ -n "$name" ] || { echo "usage: k2 compute node add <name> [--url URL] [--json]" >&2; exit 2; }
            body="$(_compute_json K2_CJ_name="$name" K2_CJ_url="$url")"
            _compute_call POST /cli/compute/nodes/enroll-code "$body" tcp
            if [ "$add_json" = "1" ]; then
                # One stable object for scripts (2026-10-28): the CODE.FP
                # string k2-node takes, its expiry, the node and controller.
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print(json.dumps({"code": v["enroll"], "expiresAt": v["expiresAt"], "name": v["name"], "controller": v["controllerUrl"]}))'
            else
                _compute_render enroll
            fi
            ;;
        confirm)
            [ $# -ge 2 ] || { echo "usage: k2 compute node confirm <name> <6-digit code>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_sas="${*:2}")"
            _compute_call POST /cli/compute/nodes/confirm "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s is active. %s" % (v["node"], v["next"]))'
            ;;
        remove|rm)
            [ $# -ge 1 ] || { echo "usage: k2 compute node remove <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1")"
            _compute_call POST /cli/compute/nodes/remove "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("removed %s (%d jobs cancelled)" % (v["node"], v["jobsCancelled"]))'
            ;;
        retire)
            [ $# -ge 1 ] || { echo "usage: k2 compute node retire <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1")"
            _compute_call POST /cli/compute/nodes/retire "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], "removed" if v["state"] == "revoked" else "draining; removed once its %d running job(s) end" % v.get("running", 0)))'
            ;;
        rename)
            [ $# -ge 2 ] || { echo "usage: k2 compute node rename <old> <new>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_name="$2")"
            _compute_call POST /cli/compute/nodes/rename "$body" tcp
            echo "renamed to $2"
            ;;
        pause|drain|resume)
            [ $# -ge 1 ] || { echo "usage: k2 compute node $sub <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_action="$sub")"
            _compute_call POST /cli/compute/nodes/pause "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: server-side %s. %s" % (v["node"], v.get("controllerPause") or "active", v["note"]))'
            ;;
        route|routes)
            local op="${1:-}"
            [ $# -ge 3 ] || { echo "usage: k2 compute node route add|rm <name> <url>" >&2; exit 2; }
            case "$op" in
                add) body="$(_compute_json K2_CJ_node="$2" K2_CJ_add="$3")" ;;
                rm|remove) body="$(_compute_json K2_CJ_node="$2" K2_CJ_remove="$3")" ;;
                *) echo "usage: k2 compute node route add|rm <name> <url>" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/compute/nodes/routes "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("routes for %s: %s" % (v["node"], " ".join(v["routes"]) or "(none)"))'
            ;;
        admin) cmd_compute_node_admin "$@" ;;
        servers) cmd_compute_node_servers "$@" ;;
        packages|pkg) cmd_compute_node_packages "$@" ;;
        take-owner)
            # MT36 (0.46.4): a node that says this server owns it now (an
            # owner change on it) is taken over only by the owner, in person.
            # The node's word is only a claim: the person sees the full key,
            # the old owner and what moves here, and types the node's name.
            [ $# -eq 1 ] && [ "${1#-}" = "$1" ] || { echo "usage: k2 compute node take-owner <node>" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/take-owner "$(_compute_json K2_CJ_node="$1" K2_CJ_preview_j=true)" tcp
            local tname tfp
            tname="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["node"])')"
            tfp="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["fingerprint"])')"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("Take over %s?" % v["node"])
print("  node key (full): %s" % v["fingerprint"])
print("  owner until now: %s" % (v.get("from") or "another server"))
print("  The node only SAYS this server owns it. Go on only if you changed its owner yourself and that key is the node you know.")
print("  This server becomes its admin (root admin on that machine: updates, policy, servers, approvals).")
print("  Every compute:<workspace>/* secret grant starts sending its secrets to it:")
if not v.get("grantsKnown"):
    print("    " + v["secretGrantsText"])
elif not v.get("secretGrants"):
    print("    none today")
else:
    for g in v["secretGrants"]:
        print("    %s: %s" % (g["workspace"], g["secret"]))' >&2
            [ -r /dev/tty ] || { echo '{"error":"needs_a_person","hint":"take-owner asks you to type the node name: run it in a terminal"}' >&2; exit 3; }
            printf 'Type the node name (%s) to take it over: ' "$tname" >&2
            local typed=""
            IFS= read -r typed </dev/tty || typed=""
            [ "$typed" = "$tname" ] || { echo '{"error":"not_confirmed","hint":"the name did not match; nothing changed"}' >&2; exit 3; }
            local tid
            _compute_call GET /cli/compute/nodes/info "node=$(urlencode "$tname")"
            tid="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
            local tok
            tok="$(_vault_confirm_token node-admin "take-owner:$tid" "take over $tname ($tfp)")" || exit 3
            _compute_call POST /cli/compute/nodes/take-owner "$(_compute_json K2_CJ_node="$tid" K2_CJ_confirmToken="$tok")" tcp
            tok=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); print("%s is this server'"'"'s now (taken over from %s): its admin, updates, servers and approvals are here." % (v["node"], v.get("from") or "its owner"))'
            ;;
        policy)
            local node="${1:-}" act="${2:-show}"
            [ -n "$node" ] || { echo "usage: k2 compute node policy <node> show|set key=value..." >&2; exit 2; }
            shift 2 2>/dev/null || shift $#
            case "$act" in
                show)
                    _compute_call GET /cli/compute/nodes/policy "node=$(urlencode "$node")"
                    for a in "$@"; do [ "$a" = "--json" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }; done
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("%s (%s, remote admin %s)" % (v["node"], "online" if v["online"] else "offline", v.get("remoteAdmin") or "unknown"))
p=v.get("policy")
if not p: print("  " + v.get("hint",""))
else:
    for k in sorted(p): print("  %-18s %s" % (k, json.dumps(p[k])))'
                    ;;
                set)
                    local reason="" pairs=()
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --reason) reason="${2:-}"; shift 2 ;;
                            *=*) pairs+=("$1"); shift ;;
                            *) echo "usage: k2 compute node policy <node> set key=value... [--reason …]" >&2; exit 2 ;;
                        esac
                    done
                    [ ${#pairs[@]} -gt 0 ] || { echo "usage: k2 compute node policy <node> set key=value..." >&2; exit 2; }
                    local set_json
                    set_json="$(_compute_kv_json "${pairs[@]}")" || exit 2
                    _compute_call POST /cli/compute/nodes/policy/set "$(_compute_json K2_CJ_node="$node" K2_CJ_set_j="$set_json" K2_CJ_reason="$reason")"
                    if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                        _compute_admin_held_or_approve
                    fi
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], v.get("note","ok")))'
                    ;;
                *) echo "usage: k2 compute node policy <node> show|set key=value..." >&2; exit 2 ;;
            esac
            ;;
        health)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node health <node> [--json]" >&2; exit 2; }
            _compute_call GET /cli/compute/nodes/health "node=$(urlencode "$1")"
            for a in "$@"; do [ "$a" = "--json" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }; done
            _compute_render_health
            ;;
        update|rollback)
            local node="" canary=0 now="" all="" batch=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --canary) canary=1; shift ;;
                    --now) now=1; shift ;;
                    --all) all=1; shift ;;
                    --batch)
                        batch="${2:-}"
                        case "$batch" in ''|*[!0-9]*) echo "usage: --batch N (a number of nodes; 0 = all at once)" >&2; exit 2 ;; esac
                        shift 2 ;;
                    -*) echo "usage: k2 compute node $sub <node> [--now]   (update also: --all [--batch N])" >&2; exit 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            if [ "$all" = "1" ]; then
                [ "$sub" = "update" ] && [ -z "$node" ] || { echo "usage: k2 compute node update --all [--batch N] [--now]   (no node name with --all)" >&2; exit 2; }
                _compute_call POST /cli/compute/nodes/update "$(_compute_json K2_CJ_all_b=1 K2_CJ_now_b="$now" K2_CJ_batch_j="${batch:-0}")"
                _compute_render_rollout 0
                return 0
            fi
            [ -n "$batch" ] && { echo "usage: --batch goes with --all" >&2; exit 2; }
            [ -n "$node" ] || { echo "usage: k2 compute node $sub <node> [--now]" >&2; exit 2; }
            _compute_call POST "/cli/compute/nodes/$sub" "$(_compute_json K2_CJ_node="$node" K2_CJ_canary_b="$canary" K2_CJ_now_b="$now")"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); u=v.get("update") or {}
print("%s: %s%s. %s" % (v["node"], u.get("state"), (" " + u["version"]) if u.get("version") else "", v.get("note","")))'
            ;;
        rollout)
            local verb="show" as_json=0 id=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    show|pause|resume|stop) verb="$1"; shift ;;
                    --json) as_json=1; shift ;;
                    --id) id="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute node rollout [show|pause|resume|stop] [--id ID] [--json]" >&2; exit 2 ;;
                esac
            done
            if [ "$verb" = "show" ]; then
                _compute_call GET /cli/compute/nodes/rollout ""
            else
                _compute_call POST "/cli/compute/nodes/rollout/$verb" "$(_compute_json K2_CJ_id="$id")"
            fi
            _compute_render_rollout "$as_json"
            ;;
        wall)
            # Q5 (0.46.4): turn the Linux job wall on; operate on 0.46.4+ nodes.
            local node="" restart=1 reason=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --no-restart) restart=0; shift ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    -*) echo "usage: k2 compute node wall <node> [--no-restart] [--reason \"…\"]" >&2; exit 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] || { echo "usage: k2 compute node wall <node> [--no-restart] [--reason \"…\"]" >&2; exit 2; }
            local restart_j=true
            [ "$restart" = "0" ] && restart_j=false
            _compute_call POST /cli/compute/nodes/wall "$(_compute_json K2_CJ_node="$node" K2_CJ_restart_j="$restart_j" K2_CJ_reason="$reason")"
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], v.get("note","ok")))'
            ;;
        alerts)
            local node="" after="" to="" have_to=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --after) after="${2:-}"; shift 2 ;;
                    --to) to="${2:-}"; have_to=1; shift 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] || [ "$have_to" = "1" ] || { echo "usage: k2 compute node alerts <node> [--after 10m|off] | --to <workspace>|none" >&2; exit 2; }
            local after_j=""
            if [ -n "$node" ]; then
                case "$after" in
                    "") after_j="" ;;
                    off|none) after_j='"off"' ;;
                    *) after_j="$(_compute_secs "$after")" ;;
                esac
            fi
            local to_val=""
            [ "$have_to" = "1" ] && to_val="${to:-none}"
            _compute_call POST /cli/compute/nodes/alerts "$(_compute_json K2_CJ_node="$node" K2_CJ_after_j="$after_j" K2_CJ_to="$to_val")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if "alertsTo" in v: print("alerts go to: %s" % (v["alertsTo"] or "nobody (unset)"))
if "node" in v:
    a=v.get("alertAfterSecs")
    print("%s: %s" % (v["node"], ("offline alert after %dm" % (a // 60)) if a else "no offline alert"))'
            ;;
        *)
            echo "usage: k2 compute node add|confirm|remove|rename|pause|drain|resume|route|health|policy|update|rollout|rollback|wall|alerts|admin|servers|packages …" >&2
            exit 2
            ;;
    esac
}

# A k2-node rollout (prd-compute-update-continuity-v1 §5.10): one line, then
# each node's place.
_compute_render_rollout() {
    if [ "${1:-0}" = "1" ]; then
        printf '%s\n' "$_CP_BODY"
        return 0
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v.get("rollout")
if not r:
    print("no rollout yet (k2 compute node update --all starts one)%s" % ("" if v.get("follow", True) else "; auto-follow (compute.nodes.follow) is off"))
    sys.exit(0)
p=r.get("position") or {}
how="all at once" if not r.get("batchSize") else ("%d at a time" % r["batchSize"])
print("rollout %s: k2-node %s, %s, %s (%s of %s settled; %s)" % (r["id"][:8], r["toVersion"], r["state"], r["trigger"], p.get("settled"), p.get("total"), how))
if r.get("note"): print("  " + r["note"])
for n in r.get("nodes") or []:
    why=(" " + n["code"]) if n.get("code") else ""
    print("  %-14s %s%s%s" % (n["node"], n["state"], why, (": " + n["detail"]) if n.get("detail") and n["state"] in ("skipped","failed","refused","timed_out") else ""))
if v.get("note"): print(v["note"])'
}

# A package request held for this server's owner (`pkgHeld`): say so and
# exit 2, like every held change. With permission cards on, the daemon's
# Ask with: line (k2 permission request compute.packages request=<id> …)
# prints last, at exit, and brings the request to the owner as a card.
_compute_pkg_held_exit() {
    printf '%s' "$_CP_BODY" | grep -q '"pkgHeld": *true' || return 0
    local id
    id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("request") or {}).get("id") or "")')"
    echo "held for the owner: package request $id" >&2
    echo "  the owner approves it on the card, in Settings → Compute → Approvals, or outside K2: k2 compute node packages approve $id" >&2
    if _permission_in_body "$_CP_BODY"; then
        echo "  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card." >&2
    fi
    echo "  you get a [compute] message when it's decided; then run your job again" >&2
    exit 2
}

# `k2 compute node packages …` (0.46.9, prd-compute-self-serve-environments-v1
# §6.3): what a node can install for this server, and asking its owner for
# more. Thin: every rule lives in the daemon (compute_pkg.rs).
_compute_node_packages_usage() {
    cat <<'EOF'
k2 compute node packages — system packages on a compute node (0.46.9)

  k2 compute node packages <node> [--json]
        the node's host line, its package manager, the policy for this
        server there, the approved and installed names, and your recent
        requests
  k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why "…" [--job ID] [--json]
        ask the node's owner to install them: one request for every name
        on every node named. A name is a Debian name (libssl-dev; K2 maps
        it for each distro), a native one (pacman:openssl, dnf:openssl-devel)
        or a K2 profile (profile:tauri-linux). A job whose --needs aren't
        covered files the same request by itself. Held for this server's
        owner: exit 2, and the Ask with: line puts it on their card. You
        get a [compute] message when the owner decides; then run your job
        again
  k2 compute node packages requests [--node N] [--state S] [--limit N] [--json]
        your requests (the owner or an Admin outside K2: every request).
        S: pending, approved, installed, denied, failed, expired or all.
        installed = run your job again; denied, failed, expired = it won't
  k2 compute node packages approve|deny <request> [--add] [--note "…"]
        the owner or an Admin, from a terminal outside K2 (an agent never
        decides). <request> is the cpr_… id that requests shows (a start of
        it that fits one is enough). The card in chat and Settings →
        Compute → Approvals are the main path. approve --add also puts the
        names on that server's approved list, so the next ask for them
        needs no card
  k2 compute node packages policy <node> [--json]
        what this server may install there without asking
  k2 compute node packages policy <node> [<server>] [level=list|profiles|official]
        [profiles=a,b] [max_install_mb=N] [--reason "…"]
        change it for <server>: own (this server, the default), or another
        server's label as k2 compute node servers <node> shows it. The
        node's owner only. profiles= takes bare K2 profile names
        (build-base, tauri-linux, rust-gui-ci, k2-workspace, gpu-headless) and replaces
        the server's list. Narrowing applies at once; widening is held for
        the owner's approval (exit 2). Every server's policy, at the
        machine: k2-node packages list
EOF
}

cmd_compute_node_packages() {
    local first="${1:-}"
    case "$first" in
        ""|help|--help|-h) _compute_node_packages_usage; return 0 ;;
    esac
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$first" in
        request)
            shift
            local nodes="" why="" job="" ask_usage='usage: k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why "…" [--job ID]'
            local -a names=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) why="${2:-}"; shift 2 ;;
                    --job) job="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$ask_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *) if [ -z "$nodes" ]; then nodes="$1"; else names+=("$1"); fi; shift ;;
                esac
            done
            [ -n "$nodes" ] && [ "${#names[@]}" -gt 0 ] || { echo "$ask_usage" >&2; exit 2; }
            [ -n "$why" ] || { echo "$ask_usage (--why: one line the owner reads on the card, e.g. \"tauri build needs webkit2gtk\")" >&2; exit 2; }
            local nodes_json names_json
            nodes_json="$(python3 -c 'import json,sys; print(json.dumps([x.strip() for x in sys.argv[1].split(",") if x.strip()]))' "$nodes")"
            names_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${names[@]}")"
            _compute_call POST /cli/compute/nodes/packages/request "$(_compute_json K2_CJ_nodes_j="$nodes_json" K2_CJ_names_j="$names_json" K2_CJ_why="$why" K2_CJ_job="$job" K2_CJ_workspace="$(_compute_ws "")")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; _compute_pkg_held_exit; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("request") or {}
print("request %s: %s" % (r.get("id"), r.get("summary") or ""))
for row in r.get("rows") or []:
    d = row.get("dry") or {}
    extra = ""
    if d.get("installs"):
        extra = " · installs %d package(s)%s" % (len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else "")
    if d.get("refusal"):
        extra += " · would be refused: " + d["refusal"]
    print("  %-14s %-9s %s%s" % (row.get("node"), row.get("state"), " ".join(row.get("names") or []), extra))
for c in v.get("cant") or []:
    print("  not asked: " + c)
if v.get("hint"):
    print(v["hint"])'
            # Held for this server's owner: exit 2 (sent to another
            # server's owner only: exit 0, you hear back by message).
            _compute_pkg_held_exit
            ;;
        requests|list|ls)
            shift
            local q=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node packages requests [--node N] [--state pending|approved|installed|denied|failed|expired|all] [--limit N] [--json]" >&2; exit 2 ;;
                esac
            done
            # The owner's view needs no passport: outside K2 it goes over TCP.
            if _compute_in_session; then
                _compute_call GET /cli/compute/nodes/packages/requests "${q#&}"
            else
                _compute_call GET /cli/compute/nodes/packages/requests "${q#&}" tcp
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("requests") or []
if not rows:
    print("no package requests")
def ago(t):
    d = max(int(time.time()) - int(t or 0), 0)
    return "now" if d < 60 else ("%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400)))
seen = set()
for r in rows:
    origin = {"own": "", "sent": "asked %s" % r.get("server"), "fwd": "from %s" % r.get("server")}.get(r.get("origin"), r.get("origin") or "")
    print("%-21s %-9s %-14s %-8s %s%s" % (r.get("batchId"), r.get("state"), r.get("node"), ago(r.get("createdAt")), " ".join(r.get("names") or []), ("  (" + origin + ")") if origin else ""))
    who = r.get("askedBy") or "?"
    print("%21s asked by %s%s%s" % ("", who, (" for job " + r["jobId"][:8]) if r.get("jobId") else "", (": " + r["why"]) if r.get("why") else ""))
    d = r.get("dry") or {}
    if d.get("installs"):
        print("%21s installs %d package(s)%s" % ("", len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else ""))
    if d.get("refusal"):
        print("%21s would be refused: %s" % ("", d["refusal"]))
    if r.get("note") or r.get("decidedBy"):
        print("%21s %s%s" % ("", ("by " + r["decidedBy"] + " ") if r.get("decidedBy") else "", r.get("note") or ""))
if v.get("canDecide") and any(r.get("state") == "pending" and r.get("origin") != "sent" for r in rows):
    print("decide: k2 compute node packages approve|deny <request> [--add] [--note \"…\"]")'
            ;;
        approve|deny)
            shift
            local id="" note="" add=0 dec_usage="usage: k2 compute node packages $first <request> [--note \"…\"]"
            [ "$first" = "approve" ] && dec_usage="usage: k2 compute node packages approve <request> [--add] [--note \"…\"]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --note|--why) note="${2:-}"; shift 2 ;;
                    --add) add=1; shift ;;
                    --json) shift ;;
                    -*) echo "$dec_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *) [ -z "$id" ] || { echo "$dec_usage (one request)" >&2; exit 2; }; id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "$dec_usage" >&2; exit 2; }
            [ "$first" = "deny" ] && [ "$add" = "1" ] && { echo "usage: --add goes with approve (it puts the names on the server's approved list)" >&2; exit 2; }
            if _compute_in_session; then
                echo '{"error":"owner_only","hint":"only the owner or an Admin decides package requests, in person: the card in chat, Settings → Compute → Approvals, or this command from a terminal outside K2. An agent can never approve."}' >&2
                exit 3
            fi
            # Show what is decided first: the names, nodes and install set.
            _compute_call GET /cli/compute/nodes/packages/requests "state=pending&limit=500" tcp
            local full
            full="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin).get("requests") or [] if r.get("origin") != "sent"]
ids = sorted({r["batchId"] for r in rows if r["batchId"] == want or r["id"] == want or r["batchId"].startswith(want)})
if len(ids) != 1:
    sys.exit(1)
b = ids[0]
print(b)
for r in rows:
    if r["batchId"] != b:
        continue
    d = r.get("dry") or {}
    sys.stderr.write("  %s on %s (%s), for %s%s\n" % (" ".join(r.get("names") or []), r.get("node"), r.get("host") or "?", r.get("askedBy") or "?", (" via " + r["server"]) if r.get("origin") == "fwd" else ""))
    if r.get("why"):
        sys.stderr.write("    why: %s\n" % r["why"])
    if d.get("installs"):
        sys.stderr.write("    installs %d package(s)%s: %s\n" % (len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else "", " ".join(d["installs"])))
' "$id")" || full="$id"
            local approve_j=false
            [ "$first" = "approve" ] && approve_j=true
            local add_j=""
            [ "$add" = "1" ] && add_j=true
            _compute_call POST /cli/compute/nodes/packages/decide "$(_compute_json K2_CJ_request="$full" K2_CJ_approve_j="$approve_j" K2_CJ_add_j="$add_j" K2_CJ_note="$note")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("rows") or []:
    print("%s %s on %s%s" % (r.get("state"), r.get("request"), r.get("node"), (": " + r["note"]) if r.get("note") else ""))'
            ;;
        policy)
            shift
            local node="" server="" reason="" pol_usage='usage: k2 compute node packages policy <node> [<server>] [level=list|profiles|official] [profiles=a,b] [max_install_mb=N] [--reason "…"]'
            local -a pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$pol_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *=*) pairs+=("$1"); shift ;;
                    *)
                        if [ -z "$node" ]; then node="$1"
                        elif [ -z "$server" ]; then server="$1"
                        else echo "$pol_usage" >&2; exit 2
                        fi
                        shift ;;
                esac
            done
            [ -n "$node" ] || { echo "$pol_usage" >&2; exit 2; }
            if [ "${#pairs[@]}" -eq 0 ]; then
                # Show: this server's policy there (the node reports only
                # the caller's; every server's is at the machine).
                _compute_call GET /cli/compute/nodes/packages "node=$(urlencode "$node")"
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                printf '%s' "$_CP_BODY" | K2_CP_SERVER="$server" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
p = v.get("packages")
if not isinstance(p, dict):
    print("%s: %s" % (v.get("node"), v.get("note") or "no package status"))
    sys.exit(0)
print("%s: %s for this server" % (v.get("node"), p.get("policy")))
if os.environ.get("K2_CP_SERVER") not in ("", "own"):
    print("  (a node reports only the asking server'"'"'s policy; every server'"'"'s, at the machine: k2-node packages list)")'
                exit 0
            fi
            local body
            body="$(K2_PP_NODE="$node" K2_PP_SERVER="${server:-own}" K2_PP_REASON="$reason" python3 -c '
import json, os, sys
out = {"node": os.environ["K2_PP_NODE"], "server": os.environ["K2_PP_SERVER"]}
if os.environ.get("K2_PP_REASON"):
    out["reason"] = os.environ["K2_PP_REASON"]
for kv in sys.argv[1:]:
    k, _, v = kv.partition("=")
    if k == "level":
        if v not in ("list", "profiles", "official"):
            sys.stderr.write("usage: level=list|profiles|official\n"); sys.exit(2)
        out["level"] = v
    elif k == "profiles":
        out["profiles"] = [x.strip() for x in v.split(",") if x.strip()]
    elif k == "max_install_mb":
        if not v.isdigit():
            sys.stderr.write("usage: max_install_mb=N (megabytes per request, 1 to 65536)\n"); sys.exit(2)
        out["maxInstallMb"] = int(v)
    else:
        sys.stderr.write("usage: %s: the keys are level, profiles and max_install_mb\n" % k); sys.exit(2)
print(json.dumps(out))' "${pairs[@]}")" || exit 2
            _compute_call POST /cli/compute/nodes/packages/policy "$body"
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v.get("node"), v.get("note") or "ok"))'
            ;;
        -*)
            echo "usage: k2 compute node packages <node> | request | requests | approve | deny | policy … (k2 compute node packages --help)" >&2
            exit 2
            ;;
        *)
            # k2 compute node packages <node> [--json]
            local node="$first"
            shift
            for a in "$@"; do
                [ "$a" = "--json" ] || { echo "usage: k2 compute node packages <node> [--json]" >&2; exit 2; }
            done
            _compute_call GET /cli/compute/nodes/packages "node=$(urlencode "$node")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
print("%s%s" % (v.get("node"), ("  host: " + v["hostLine"]) if v.get("hostLine") else ""))
p = v.get("packages")
if not isinstance(p, dict):
    print("  " + (v.get("note") or "no package status from this node"))
else:
    print("  manager    %s" % (p.get("manager") or "?"))
    print("  policy     %s (for this server; set by %s)" % (p.get("policy"), v.get("owner") or "its owner"))
    ap = p.get("approved") or []
    print("  approved   %s" % (" ".join(ap) if ap else "(none)"))
    ins = p.get("installed") or []
    print("  installed  %s" % (" ".join(ins) if ins else "(none of those reported)"))
    age = p.get("indexAgeSecs")
    if isinstance(age, (int, float)) and age > 86400:
        print("  index      %dd old (K2 refreshes apt/zypper before an install; an Arch node needs its owner to upgrade first)" % (age // 86400))
    if p.get("openRequests"):
        print("  open       %s request(s) wait for its owner" % p["openRequests"])
if not v.get("takesRequests"):
    print("  requests   this node takes none from K2 (older k2-node, or a Mac): its owner installs at the machine")
rows = v.get("requests") or []
for r in rows:
    print("  %-21s %-9s %s" % (r.get("batchId"), r.get("state"), " ".join(r.get("names") or [])))
print("ask for more: k2 compute node packages request %s <name>[,<name>…] --why \"…\"" % v.get("node"))'
            ;;
    esac
}

# `k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …]`
# (node sharing, 0.46.3: prd-compute-shared-queue-v1 §9; owner, 0.46.4:
# prd-macos-tenant-compute-v1 §6).
cmd_compute_node_servers() {
    local node="${1:-}"
    [ -n "$node" ] && [ "${node#-}" = "$node" ] || { echo "usage: k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …] (k2 compute --help)" >&2; exit 2; }
    shift
    local act="${1:-list}"
    [ "$act" = "--json" ] && act=list
    [ $# -gt 0 ] && [ "$act" != "list" ] && shift
    [ "$act" = "list" ] && [ "${1:-}" = "list" ] && shift
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$act" in
        list|ls)
            local q="node=$(urlencode "$node")"
            for a in "$@"; do [ "$a" = "--refresh" ] && q="$q&refresh=1"; done
            _compute_call GET /cli/compute/nodes/servers "$q" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render servers
            ;;
        add)
            local url="" code="" trust="own" forv="" reason="" pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --url) url="${2:-}"; shift 2 ;;
                    --code) code="${2:-}"; shift 2 ;;
                    --trust) trust="${2:-}"; shift 2 ;;
                    --for) forv="${2:-}"; shift 2 ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    --weight) pairs+=("weight=${2:-}"); shift 2 ;;
                    --max-running) pairs+=("max_running=${2:-}"); shift 2 ;;
                    --max-queued) pairs+=("max_queued=${2:-}"); shift 2 ;;
                    --cpu-hours) pairs+=("cpu_hours_day=${2:-}"); shift 2 ;;
                    --max-time) pairs+=("max_job_secs=$(_compute_secs "${2:-}")"); shift 2 ;;
                    --cpus) pairs+=("max_cpus=${2:-}"); shift 2 ;;
                    --mem) local m="${2:-}"; pairs+=("max_mem_gb=${m%[Gg]}"); shift 2 ;;
                    --disk) local d="${2:-}"; pairs+=("disk_gb=${d%[Gg]}"); shift 2 ;;
                    --network) pairs+=("network=\"${2:-}\""); shift 2 ;;
                    --gate) pairs+=("gate=true" "top_class=\"gate\""); shift ;;
                    --exclusive) pairs+=("exclusive=true"); shift ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node servers <node> add --url URL --code -|CODE.FP [--trust own|team] [--for 7d|none] [limits…] (unknown '$1')" >&2; exit 2 ;;
                esac
            done
            # The code off argv (2026-10-28): --code - reads one line from
            # stdin; no --code at all takes $K2_NODE_ENROLL_CODE.
            if [ "$code" = "-" ]; then
                IFS= read -r code || true
                code="${code%$'\r'}"
            elif [ -z "$code" ] && [ -n "${K2_NODE_ENROLL_CODE:-}" ]; then
                code="$K2_NODE_ENROLL_CODE"
            fi
            [ -n "$url" ] && [ -n "$code" ] || { echo "usage: k2 compute node servers <node> add --url URL --code - < code-file (or K2_NODE_ENROLL_CODE) [--trust own|team] [--for 7d|none]" >&2; exit 2; }
            local for_n=""
            case "$forv" in
                "") for_n="" ;;
                none|never|0) for_n=0 ;;
                *d) for_n=$(( ${forv%d} * 86400 )) ;;
                *) for_n="$(_compute_secs "$forv")" ;;
            esac
            local set_json
            set_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
            _compute_call POST /cli/compute/nodes/servers/add "$(_compute_json K2_CJ_node="$node" K2_CJ_url="$url" K2_CJ_code="$code" \
                K2_CJ_trust="$trust" K2_CJ_forSecs_n="$for_n" K2_CJ_set_j="$set_json" K2_CJ_reason="$reason")" tcp
            code=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if v.get("warning"): print("note: " + v["warning"], file=sys.stderr)'
            _compute_admin_held_or_approve
            ;;
        approve) [ -n "${1:-}" ] || { echo "usage: k2 compute node servers <node> approve <request>" >&2; exit 2; }; _compute_admin_approve "$1" ;;
        deny) cmd_compute_node_admin deny "$@" ;;
        limits)
            local server="${1:-}" reason="" forv="" prio="" pairs=()
            [ -n "$server" ] && [ "${server#-}" = "$server" ] || { echo "usage: k2 compute node servers <node> limits <server> [--weight N] key=value…" >&2; exit 2; }
            shift
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    # Tiers (0.46.5): does this server's --tier priority count
                    # on the node? Its own body key (kept in the node's
                    # queue.toml, never in the share); a 0.46.5 node only.
                    priority=true|priority=yes|priority=1) prio=1; shift ;;
                    priority=false|priority=no|priority=0) prio=0; shift ;;
                    priority=*) echo "usage: priority=true|false" >&2; exit 2 ;;
                    --weight) pairs+=("weight=${2:-}"); shift 2 ;;
                    for=*) forv="${1#for=}"; shift ;;
                    --for) forv="${2:-}"; shift 2 ;;
                    max_job_secs=*h|max_job_secs=*m|max_job_secs=*s) pairs+=("max_job_secs=$(_compute_secs "${1#max_job_secs=}")"); shift ;;
                    network=*|top_class=*) pairs+=("${1%%=*}=\"${1#*=}\""); shift ;;
                    *=*) pairs+=("$1"); shift ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node servers <node> limits <server> key=value… (unknown '$1')" >&2; exit 2 ;;
                esac
            done
            local for_n=""
            case "$forv" in
                "") for_n="" ;;
                none|never|0) for_n=0 ;;
                *d) for_n=$(( ${forv%d} * 86400 )) ;;
                *) for_n="$(_compute_secs "$forv")" ;;
            esac
            [ ${#pairs[@]} -gt 0 ] || [ -n "$for_n" ] || [ -n "$prio" ] || { echo "usage: k2 compute node servers <node> limits <server> key=value… (at least one)" >&2; exit 2; }
            local set_json
            set_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
            _compute_call POST /cli/compute/nodes/servers/limits "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_set_j="$set_json" K2_CJ_forSecs_n="$for_n" K2_CJ_reason="$reason" K2_CJ_priority_b="$prio")" tcp
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s on %s: %s" % (v.get("server"), v["node"], v.get("note","ok")))'
            ;;
        pause|resume)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node servers <node> $act <server>" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/servers/pause "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$1" K2_CJ_action="$act")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s on %s: %s. %s" % (v["server"], v["node"], v["action"].split(".")[-1] + "d", v["note"]))'
            ;;
        revoke)
            local server="" now=0 wipe=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --now) now=1; shift ;;
                    --wipe) wipe=1; shift ;;
                    --json) shift ;;
                    -*) echo "usage: k2 compute node servers <node> revoke <server> [--now] [--wipe]" >&2; exit 2 ;;
                    *) server="$1"; shift ;;
                esac
            done
            [ -n "$server" ] || { echo "usage: k2 compute node servers <node> revoke <server> [--now] [--wipe]" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/servers/revoke "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_now_b="$now" K2_CJ_wipe_b="$wipe")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("revoked %s on %s: %s" % (v["server"], v["node"], v["note"]))'
            ;;
        owner)
            local server="" old="keep" caches="move" now=0 reason=""
            local usage="usage: k2 compute node servers <node> owner <server> [--old keep|revoke] [--caches move|wipe] [--now] [--reason TEXT]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --old) old="${2:-}"; shift 2 ;;
                    --caches) caches="${2:-}"; shift 2 ;;
                    --now) now=1; shift ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$usage" >&2; exit 2 ;;
                    *) server="$1"; shift ;;
                esac
            done
            [ -n "$server" ] || { echo "$usage" >&2; exit 2; }
            case "$old" in keep|revoke) ;; *) echo "$usage (--old is keep or revoke)" >&2; exit 2 ;; esac
            case "$caches" in move|wipe) ;; *) echo "$usage (--caches is move or wipe)" >&2; exit 2 ;; esac
            _compute_call POST /cli/compute/nodes/servers/owner "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_old="$old" \
                K2_CJ_caches="$caches" K2_CJ_now_b="$now" K2_CJ_reason="$reason")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if v.get("note"): print("note: " + v["note"], file=sys.stderr)'
            _compute_admin_held_or_approve
            ;;
        *)
            echo "usage: k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …]" >&2
            exit 2
            ;;
    esac
}

# key=value… → a JSON object. Values parse as JSON when they can (numbers,
# true/false, [lists]); a,b,c for names/labels becomes a list; else string.
_compute_kv_json() {
    python3 -c '
import json, sys
out = {}
for a in sys.argv[1:]:
    k, _, v = a.partition("=")
    k = k.strip()
    if not k:
        print("usage: key=value", file=sys.stderr); sys.exit(2)
    try:
        val = json.loads(v)
    except Exception:
        val = v
    if k in ("names", "labels") and isinstance(val, str):
        val = [x for x in val.split(",") if x]
    out[k] = val
print(json.dumps(out))' "$@"
}

_compute_render_health() {
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
f = v.get("facts") or {}
def gb(b): return "%d GB" % (b // (1 << 30)) if isinstance(b, int) else "?"
seen = v.get("lastSeen")
print("%s  %s%s" % (v["node"], "online" if v["online"] else "OFFLINE", "" if v["online"] or not seen else " since " + time.strftime("%H:%M", time.localtime(seen))))
print("  k2-node %s, remote admin %s, pending approvals %s" % (v.get("nodeVersion") or "?", v.get("remoteAdmin") or "unknown", v.get("pendingApprovals", 0)))
if v.get("needsPin"):
    print("  NEEDS ONE-TIME PIN: admin commands are refused until someone runs, at that machine:")
    print("      %s" % (v.get("pinCommand") or "sudo k2-node admin pin"))
if not f:
    print("  no health report yet (needs a k2-node with health)")
else:
    if f.get("uptime_secs") is not None: print("  uptime        %dd %dh" % (f["uptime_secs"] // 86400, f["uptime_secs"] % 86400 // 3600))
    for d in f.get("disks") or []: print("  disk %-8s %s free of %s" % (d.get("path") or d.get("name"), gb(d["free_bytes"]), gb(d["total_bytes"])))
    if f.get("mem_total_bytes"): print("  memory        %s available of %s, pressure %s" % (gb(f.get("mem_available_bytes")), gb(f["mem_total_bytes"]), f.get("memory_pressure") or "?"))
    if f.get("disk_encryption"): print("  FileVault     %s%s" % (f["disk_encryption"], " (a power cut needs someone at the machine)" if f["disk_encryption"] == "on" else ""))
    ar = (f.get("sleep") or {}).get("autorestart")
    if ar is not None: print("  autorestart   %s" % ("on" if ar == 1 else "off"))
    up = f.get("updates") or {}
    if up.get("pending") is not None: print("  updates       %s pending (%s security)" % (up["pending"], up.get("security") or 0))
    if up.get("reboot_required"): print("  reboot        required")
    ex = f.get("executor") or {}
    if ex and not ex.get("reachable"): print("  executor      not answering")
    if ex.get("wall_restart_pending"): print("  wall restart  owed: a restart onto the job wall is owed; run wall.enable again (or approve the held restart): k2 compute node wall %s" % v["node"])
    for s in f.get("stuck_jobs") or []: print("  stuck job     %s (quiet %dm)" % (s["job_id"][:8], s.get("quiet_secs", 0) // 60))
    sec = f.get("security")
    if sec:
        print("  job wall      %s%s" % (sec.get("wall"), (" (jobs run as %s)" % sec["job_user"]) if sec.get("wall") == "job_user" and sec.get("job_user") else ""))
        print("  network none  %s" % ("enforced" if sec.get("network_none") else "not enforced: network = none takes no jobs here"))
        for h in sec.get("open_homes") or []: print("  open home     %s" % h)
        for n in sec.get("notes") or []: print("  WARNING       %s" % n)
    else:
        print("  job wall      none reported (k2-node before 0.46.3: jobs run as the node user and can read its key)")
u = v.get("update")
if u: print("  update        %s %s%s" % (u.get("state"), u.get("version") or "", (" (" + u["reason"] + ")") if u.get("reason") else ""))
a = v.get("alertAfterSecs")
print("  alerts        %s, to %s" % (("offline after %dm" % (a // 60)) if a else "no offline alert", v.get("alertsTo") or "nobody (k2 compute node alerts --to <ws>)"))'
}

# A held request: the owner (outside K2) confirms right away; anyone else
# gets the request id and exit 2.
_compute_admin_held_or_approve() {
    local id
    id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v["request"]
print("held for the owner: %s\n  %s" % (r["id"], r["summary"]))' >&2
    if ! _compute_in_session && [ -t 0 ] && [ "${K2_NO_APPROVE:-}" != "1" ]; then
        _compute_admin_approve "$id"
        exit 0
    fi
    echo "  the owner approves it in Settings → Compute → Approvals (or: k2 compute node admin approve $id, outside K2)" >&2
    # 0.46.9: with permission cards on, the daemon's Ask with: line (printed
    # last, at exit) brings this same request to the owner's chat as a card.
    if _permission_in_body "$_CP_BODY"; then
        echo "  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card." >&2
        echo "  Allow there sends it to the node, as Settings would (don't run your command again); your [permission:] turn says how it went" >&2
    fi
    exit 2
}

# Owner confirm (Touch ID / login or vault password) bound to the request,
# then approve. Never works for an agent: the daemon refuses passports, and
# the confirm needs a person.
_compute_admin_approve() {
    local id="$1"
    if _compute_in_session; then
        echo '{"error":"owner_only","hint":"approve in Settings → Compute → Approvals, or from a terminal outside K2. An agent can never approve."}' >&2
        exit 3
    fi
    _compute_call GET /cli/compute/admin/requests "state=all&limit=500" tcp
    local found full label
    found="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin)["requests"] if r["id"] == want or r["id"].startswith(want)]
if len(rows) != 1:
    sys.exit(1)
r = rows[0]
print(r["id"]); print(r["state"]); print(r["summary"][:64])
print("---"); print(r["summary"])
if r.get("reason"): print("reason: " + r["reason"])
for s in (r.get("body") or {}).get("steps") or [r.get("body") or {}]:
    if s.get("verb") == "script":
        print("--- script (%s) ---" % (s.get("args") or {}).get("interpreter", "/bin/sh"))
        print((s.get("args") or {}).get("text", ""))
'  "$id")" || { echo "{\"error\":\"request_not_found\",\"hint\":\"no single request matches '$id' (k2 compute node admin list)\"}" >&2; exit 3; }
    local state
    full="$(printf '%s\n' "$found" | sed -n 1p)"
    state="$(printf '%s\n' "$found" | sed -n 2p)"
    label="$(printf '%s\n' "$found" | sed -n 3p)"
    if [ "$state" != "pending" ]; then
        echo "{\"error\":\"not_pending\",\"hint\":\"$full is $state\"}" >&2
        exit 3
    fi
    printf '%s\n' "$found" | sed -n '5,$p' >&2
    local tok
    tok="$(_vault_confirm_token node-admin "$full" "$label")" || exit 3
    _compute_call POST /cli/compute/admin/approve "$(_compute_json K2_CJ_request="$full" K2_CJ_confirmToken="$tok")" tcp
    tok=""
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v["request"]
print("approved %s: %s\n  %s" % (r["id"], r["summary"], v.get("note","")))'
}

cmd_compute_node_admin() {
    local sub="${1:-}"
    case "$sub" in
        propose|list|ls|approve|deny|withdraw) shift ;;
        ""|help|--help|-h) _compute_usage; return 0 ;;
        *) sub="propose" ;;  # PRD shape: k2 compute node admin <node> <verb> …
    esac
    case "$sub" in
        propose)
            local node="" verb="" reason="" file="" interp="" timeout="" plan="" pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    --file) file="${2:-}"; shift 2 ;;
                    --interpreter) interp="${2:-}"; shift 2 ;;
                    --timeout) timeout="$(_compute_secs "${2:-}")"; shift 2 ;;
                    --plan) plan="${2:-}"; shift 2 ;;
                    *=*) pairs+=("$1"); shift ;;
                    *)
                        if [ -z "$node" ]; then node="$1"
                        elif [ -z "$verb" ]; then verb="$1"
                        else echo "usage: k2 compute node admin propose <node> <verb> [key=value…]" >&2; exit 2
                        fi
                        shift ;;
                esac
            done
            [ -n "$node" ] && { [ -n "$verb" ] || [ -n "$plan" ]; } || { echo "usage: k2 compute node admin propose <node> <verb> [key=value…] --reason \"…\" (or --plan plan.json)" >&2; exit 2; }
            local body
            if [ -n "$plan" ]; then
                [ -f "$plan" ] || { echo "usage: --plan needs a JSON file: {\"steps\": [{\"verb\": …, \"args\": {…}}]}" >&2; exit 2; }
                body="$(python3 -c '
import json, sys
p = json.load(open(sys.argv[1]))
steps = p["steps"] if isinstance(p, dict) else p
print(json.dumps({"node": sys.argv[2], "kind": "plan", "steps": steps, "reason": sys.argv[3] or None}))' "$plan" "$node" "$reason")" || exit 2
            else
                local args_json
                args_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
                if [ "$verb" = "script" ]; then
                    [ -n "$file" ] && [ -f "$file" ] || { echo "usage: k2 compute node admin propose <node> script --file fix.sh --reason …" >&2; exit 2; }
                    args_json="$(python3 -c '
import json, sys
a = json.loads(sys.argv[1]); a["text"] = open(sys.argv[2]).read()
if sys.argv[3]: a["interpreter"] = sys.argv[3]
if sys.argv[4]: a["timeout_secs"] = int(sys.argv[4])
print(json.dumps(a))' "$args_json" "$file" "$interp" "$timeout")" || exit 2
                fi
                local kind=command
                case "$verb" in svc.*) kind=service ;; esac
                body="$(_compute_json K2_CJ_node="$node" K2_CJ_kind="$kind" K2_CJ_verb="$verb" K2_CJ_args_j="$args_json" K2_CJ_reason="$reason")"
            fi
            _compute_call POST /cli/compute/admin/propose "$body"
            _compute_admin_held_or_approve
            ;;
        list|ls)
            local q="" as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --json) as_json=1; shift ;;
                    *) echo "usage: k2 compute node admin list [--node N] [--state open|pending|all]" >&2; exit 2 ;;
                esac
            done
            _compute_call GET /cli/compute/admin/requests "${q#&}"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys,time
v=json.load(sys.stdin)
rows=v["requests"]
if not rows: print("no admin requests")
for r in rows:
    print("%-36s %-9s %-10s %s" % (r["id"], r["state"], r.get("node") or "?", r["summary"]))
    if r.get("reason"): print("%36s reason: %s" % ("", r["reason"]))
    if r.get("denyReason"): print("%36s denied: %s" % ("", r["denyReason"]))'
            ;;
        approve)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node admin approve <id>" >&2; exit 2; }
            _compute_admin_approve "$1"
            ;;
        deny)
            local id="" why=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) why="${2:-}"; shift 2 ;;
                    *) id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "usage: k2 compute node admin deny <id> [--why …]" >&2; exit 2; }
            _compute_call POST /cli/compute/admin/deny "$(_compute_json K2_CJ_request="$id" K2_CJ_why="$why")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin)["request"]; print("denied %s: %s" % (r["id"], r["summary"]))'
            ;;
        withdraw)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node admin withdraw <id>" >&2; exit 2; }
            _compute_call POST /cli/compute/admin/withdraw "$(_compute_json K2_CJ_request="$1")"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin)["request"]; print("withdrew %s" % r["id"])'
            ;;
    esac
}

cmd_compute_grant() {
    local node="" workspace="" maxt="" maxd="" maxp="" maxq="" allow_ci="" maxsub="" allow_gpu=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) workspace="${2:-}"; shift 2 ;;
            # Per-submitter running cap on this node (0.46.6; 0 = off,
            # absent keeps what the grant had).
            --submitter-max-running) maxsub="${2:-}"; shift 2
                case "$maxsub" in ''|*[!0-9]*) echo "usage: --submitter-max-running N (0 = off)" >&2; exit 2 ;; esac ;;
            # CI permission (0.46.5): owner or an Admin login only; absent
            # keeps what the grant had.
            --ci) allow_ci=1; shift ;;
            --no-ci) allow_ci=0; shift ;;
            # GPU jobs (0.46.11): owner or an Admin login only; absent keeps
            # what the grant had.
            --gpu) allow_gpu=1; shift ;;
            --no-gpu) allow_gpu=0; shift ;;
            --max-time) maxt="$(_compute_secs "${2:-}")"; shift 2 ;;
            --max-disk) maxd="${2:-}"; maxd="${maxd%[Gg]}"; shift 2 ;;
            --max-parallel) maxp="${2:-}"; shift 2 ;;
            --max-queued) maxq="${2:-}"; shift 2 ;;
            -*) echo "usage: unknown flag '$1'" >&2; exit 2 ;;
            *) node="$1"; shift ;;
        esac
    done
    [ -n "$node" ] && [ -n "$workspace" ] || { echo "usage: k2 compute grant <node> --workspace WS [--max-time 2h] [--max-disk 60G] [--max-parallel 1] [--max-queued 10] [--submitter-max-running N] [--ci|--no-ci] [--gpu|--no-gpu]" >&2; exit 2; }
    local body
    body="$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$workspace" K2_CJ_maxJobSecs_n="$maxt" \
        K2_CJ_maxDiskGb_n="$maxd" K2_CJ_maxParallel_n="$maxp" K2_CJ_maxQueued_n="$maxq" K2_CJ_submitterMaxRunning_n="$maxsub")"
    if [ -n "$allow_ci" ]; then
        body="$(printf '%s' "$body" | K2_CG_CI="$allow_ci" python3 -c 'import json,os,sys; v=json.load(sys.stdin); v["allowCi"] = os.environ["K2_CG_CI"] == "1"; print(json.dumps(v))')"
    fi
    if [ -n "$allow_gpu" ]; then
        body="$(printf '%s' "$body" | K2_CG_GPU="$allow_gpu" python3 -c 'import json,os,sys; v=json.load(sys.stdin); v["allowGpu"] = os.environ["K2_CG_GPU"] == "1"; print(json.dumps(v))')"
    fi
    _compute_call POST /cli/compute/grants/set "$body" tcp
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
g = v["grant"]
sub = (", %s running per submitter" % g["submitterMaxRunning"]) if g.get("submitterMaxRunning") else ""
print("granted %s to %s (max %ss/job, %s parallel%s, %s queued, %s GB disk%s%s). %s" % (v["node"], v["workspace"], g["maxJobSecs"], g["maxParallel"], sub, g["maxQueued"], g["maxDiskGb"], ", CI allowed" if g.get("allowCi") else "", ", GPU jobs allowed" if g.get("allowGpu") else "", v["note"]))
if v.get("resent"):
    print("re-sent %d queued job(s) of %s on %s with the new caps (they keep their order)" % (v["resent"], v["workspace"], v["node"]))'
}

# k2 compute allow-nodes (0.46.8): this workspace's compute node list. No
# argument shows it; nodes (or --add/--remove/--clear) change it. The
# daemon decides who may: the owner or an Admin any way; an agent only
# narrows its own (adding or --clear: allow_nodes_needs_person, exit 3).
cmd_compute_allow_nodes() {
    local nodes="" add="" remove="" clear="" workspace="" as_json=0
    local usage="usage: k2 compute allow-nodes [<node>[,<node>…] | --add NODE[,NODE…] | --remove NODE[,NODE…] | --clear] [--workspace WS] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --add|--remove|--workspace)
                [ $# -ge 2 ] && [ -n "$2" ] || { echo "$usage ($1 needs a value)" >&2; exit 2; }
                case "$1" in
                    --add) add="${add:+$add,}$2" ;;
                    --remove) remove="${remove:+$remove,}$2" ;;
                    --workspace) workspace="$2" ;;
                esac
                shift 2 ;;
            --clear) clear=1; shift ;;
            --json) as_json=1; shift ;;
            -*) echo "$usage (unknown flag '$1')" >&2; exit 2 ;;
            *) nodes="${nodes:+$nodes,}$1"; shift ;;
        esac
    done
    local ws
    ws="$(_compute_ws "$workspace")"
    if [ -z "$nodes$add$remove$clear" ]; then
        local q=""
        [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
        _compute_call GET /cli/compute/allow-nodes "$q"
    else
        _compute_call POST /cli/compute/allow-nodes/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_nodes="$nodes" \
            K2_CJ_add="$add" K2_CJ_remove="$remove" K2_CJ_clear_b="$clear")"
    fi
    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("change") == "same":
    print("no change")
if v.get("list"):
    names = []
    for n in v.get("nodes") or []:
        if not n.get("name"):
            names.append("%s (removed)" % n.get("id"))
            continue
        name = n["name"]
        if n.get("state") not in (None, "active"):
            name += " (%s)" % n["state"]
        if not n.get("granted"):
            name += " (no grant)"
        names.append(name)
    print("%s: jobs go only to %s" % (v.get("workspace"), ", ".join(names)))
    print("  a node off the list is refused (node_not_allowed); --any, moves and retries stay on the list")
else:
    print("%s: no node list (its jobs may go to any node granted to it)" % v.get("workspace"))
if v.get("failedQueued"):
    print("  ended %d queued job(s) on nodes that left the list (failed/node_not_allowed): %s" % (len(v["failedQueued"]), " ".join(j[:8] for j in v["failedQueued"])))
if v.get("withdrawn"):
    print("  asked %d job(s) back from a shared node'"'"'s queue (they never start there)" % v["withdrawn"])'
}

# k2 compute git-access (0.46.9, GH#78): short-lived, read-only git tokens
# for a workspace's compute jobs, for the GitHub repos a person granted.
# The daemon decides who may: setup is the owner's or an Admin's (outside
# K2); grant is a person's (an agent gets git_access_needs_person, exit 3,
# and asks on a card); an agent may list and revoke its own workspace's.
_compute_git_access_usage() {
    cat <<'EOF'
k2 compute git-access — let compute jobs read private GitHub repos

  k2 compute git-access [--workspace WS] [--json]
        the workspace's grants and this server's GitHub credential (its
        kind and App id; never a key or token). WS defaults to the
        workspace of the folder you run from; an agent always gets its own.

  k2 compute git-access setup --github-app <app id> --key-file <app.pem>
  k2 compute git-access setup --github-token-file <file> | --github-token-stdin
  k2 compute git-access setup --clear
        the server's GitHub credential, one per server. The owner or an
        Admin, from a terminal outside K2 (a plain shell on this machine,
        where k2 uses the owner's token; a K2 tab sends the tab's passport
        and is refused). Preferred: a GitHub App you create (Repository
        permissions: Contents read-only; install it on every repo you'll
        grant; download a private key). The key goes into this server's
        vault: delete the .pem afterwards. Each ask gets a 1-hour token that
        reads one repo. A fine-grained token (github_pat_…) also works, but
        GitHub can't mint short-lived tokens from it, so nodes get the
        token itself: make it read-only, for just these repos, with an
        expiry. Your gh login and classic tokens (ghp_…) are refused: they
        reach every repo you can, with write access.
        An agent can't set it (nor see it). It asks a person on a card:
          k2 permission request compute.git-access-setup --why "…"
        The owner or an Admin then picks App or token in K2's own form on
        the card; the value goes straight into this server's vault, the
        same as this command.

  k2 compute git-access grant --github <owner>/<repo>[,<owner>/<repo>…]
        [--nodes all|<node>[,<node>…]] [--shared] [--workspace WS]
        let the workspace's jobs read these repos. Grant the job's own repo
        too when it's a private GitHub repo: the node then fetches it
        itself, so the 50 MB relay cap no longer applies. --nodes all (the
        default) is every node this server owns, now and enrolled later
        (k2 compute nodes lists them; a node another server shares with you
        says "shared by …"). Such a node needs its name and --shared: the
        token then reaches that machine (its admin can read it while a job
        uses it); App tokens only. Access is read-only; there's no write
        (--read-only is accepted and changes nothing). The grant checks each
        repo with GitHub first. It reaches jobs placed after it.
        A person grants. An agent gets git_access_needs_person (exit 3) and
        asks: k2 permission request compute.git-access repos=<owner>/<repo> --why "…"
        When a person approves that card, the grant is made (every node this
        server owns); run the job again. With no credential on the server
        yet, the agent gets git_access_needs_setup (exit 3) instead and
        asks for the setup card first (above); then it runs the grant
        again, which asks for the grant card.

  k2 compute git-access revoke --github <owner>/<repo>[,…] [--nodes …] | --all [--workspace WS]
        new asks are refused at once; an App's tokens already handed out are
        deleted at GitHub. Running jobs keep running; their next git call
        for the repo fails. A fine-grained token stays valid at GitHub until
        you rotate it there. An agent may revoke its own workspace's.

What a job sees: plain `git clone/fetch https://github.com/<owner>/<repo>`
works (submodules too; ssh URLs to github.com are read as https), through
git's own credential helper. Cargo git dependencies: add
--env CARGO_NET_GIT_FETCH_WITH_CLI=true so cargo uses git. Inside the job
this needs a Linux node with the job wall (sudo k2-node setup); on a Mac or
an unwalled node only the node's own fetch of your source uses the grant.
The token is never in the job's env, so the job keeps --cache, its warm
slot and sccache. A job can read a token it's handed while it runs.
More: k2 study compute (GIT ACCESS).
EOF
}

cmd_compute_git_access() {
    local verb="list"
    case "${1:-}" in
        setup|grant|revoke|list) verb="$1"; shift ;;
    esac
    local github="" nodes="" workspace="" shared="" all="" clear="" app_id="" key_file="" token_file="" token_stdin="" as_json=0
    local usage="usage: k2 compute git-access [setup|grant|revoke] … (k2 compute git-access --help)"
    while [ $# -gt 0 ]; do
        case "$1" in
            --github|--nodes|--workspace|--github-app|--key-file|--github-token-file)
                [ $# -ge 2 ] && [ -n "$2" ] || { echo "$usage ($1 needs a value)" >&2; exit 2; }
                case "$1" in
                    --github) github="${github:+$github,}$2" ;;
                    --nodes) nodes="${nodes:+$nodes,}$2" ;;
                    --workspace) workspace="$2" ;;
                    --github-app) app_id="$2" ;;
                    --key-file) key_file="$2" ;;
                    --github-token-file) token_file="$2" ;;
                esac
                shift 2 ;;
            --github-token-stdin) token_stdin=1; shift ;;
            --shared) shared=1; shift ;;
            --read-only) shift ;;
            --all) all=1; shift ;;
            --clear) clear=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "$usage (unknown argument '$1')" >&2; exit 2 ;;
        esac
    done
    local ws
    ws="$(_compute_ws "$workspace")"
    case "$verb" in
        list)
            local q=""
            [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
            _compute_call GET /cli/compute/git-access "$q"
            ;;
        setup)
            local body
            if [ -n "$clear" ]; then
                body='{"clear":true}'
            elif [ -n "$app_id" ]; then
                [ -n "$key_file" ] || { echo "usage: k2 compute git-access setup --github-app <app id> --key-file <app.pem>" >&2; exit 2; }
                [ -r "$key_file" ] || { echo "k2 compute git-access: can't read $key_file" >&2; exit 2; }
                body="$(K2_GA_ID="$app_id" K2_GA_FILE="$key_file" python3 -c '
import json, os
print(json.dumps({"app_id": os.environ["K2_GA_ID"], "key_pem": open(os.environ["K2_GA_FILE"]).read()}))')"
            elif [ -n "$token_file" ] || [ -n "$token_stdin" ]; then
                if [ -n "$token_file" ]; then
                    [ -r "$token_file" ] || { echo "k2 compute git-access: can't read $token_file" >&2; exit 2; }
                fi
                body="$(K2_GA_FILE="$token_file" python3 -c '
import json, os, sys
f = os.environ.get("K2_GA_FILE")
t = (open(f).read() if f else sys.stdin.read()).strip()
print(json.dumps({"token": t}))')"
            else
                echo "usage: k2 compute git-access setup --github-app <app id> --key-file <app.pem> | --github-token-file <file> | --github-token-stdin | --clear" >&2
                echo "  (a person's, from a terminal outside K2; an agent asks: k2 permission request compute.git-access-setup --why \"…\")" >&2
                exit 2
            fi
            _compute_call POST /cli/compute/git-access/setup "$body"
            ;;
        grant)
            [ -n "$github" ] || { echo "usage: k2 compute git-access grant --github <owner>/<repo>[,…] [--nodes all|<node>,…] [--shared] [--workspace WS]" >&2; exit 2; }
            _compute_call POST /cli/compute/git-access/grant "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_github="$github" K2_CJ_nodes="$nodes" K2_CJ_shared_b="$shared")"
            ;;
        revoke)
            [ -n "$github$all" ] || { echo "usage: k2 compute git-access revoke --github <owner>/<repo>[,…] [--nodes …] | --all [--workspace WS]" >&2; exit 2; }
            _compute_call POST /cli/compute/git-access/revoke "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_github="$github" K2_CJ_nodes="$nodes" K2_CJ_all_b="$all")"
            ;;
    esac
    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "workspace" not in v:
    c = v.get("credential")
    if c is None:
        print("GitHub credential cleared: every later ask for a git token is refused; grants stay")
    else:
        print("GitHub credential set: %s%s" % ("App " + str(c.get("appId")) + (" (" + c["app"] + ")" if c.get("app") else "") if c.get("kind") == "app" else "fine-grained token", ""))
    if v.get("note"):
        print("  " + v["note"])
    sys.exit(0)
c = v.get("credential")
if c:
    print("credential: %s" % ("GitHub App " + str(c.get("appId")) if c.get("kind") == "app" else "a fine-grained token (nodes get the token itself)"))
else:
    print("credential: none (a person runs: k2 compute git-access setup --github-app <id> --key-file <app.pem>;")
    print("  an agent asks: k2 permission request compute.git-access-setup --why \"<what you need it for>\")")
for w in v.get("warnings") or []:
    print("warning: " + w)
if v.get("granted"):
    print("granted: %s" % ", ".join(v["granted"]))
if v.get("revoked"):
    print("revoked: %s (%d grant%s removed)" % (", ".join(v["revoked"]), v.get("removed", 0), "" if v.get("removed") == 1 else "s"))
    if v.get("tokensDeleted"):
        print("  %d token(s) already handed out were deleted at GitHub" % v["tokensDeleted"])
    for j in v.get("runningJobs") or []:
        print("  running job %s had a token for %s: it keeps running; its next git call for it fails" % (j["job"], j["repo"]))
    if v.get("note"):
        print("  " + v["note"])
gs = v.get("grants") or []
if gs:
    print("%s: jobs may read" % v["workspace"])
    for g in gs:
        print("  %-40s nodes: %s%s" % (g["repo"], g["nodes"], " (shared: another server owns it)" if g.get("shared") else ""))
else:
    print("%s: no git access (k2 compute git-access grant --github <owner>/<repo>)" % v["workspace"])
if v.get("tokenNote"):
    print("note: " + v["tokenNote"])'
}

# k2 compute retier <job> --tier T [--why "…"] [--json] (0.46.6, prd
# job tiers v2 L2): a queued job's tier. The job's own workspace, or the
# owner / an Admin for any job.
cmd_compute_retier() {
    local job="" tier="" why="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tier) tier="${2:-}"; shift 2 ;;
            --priority|--critical) tier="priority"; shift ;;
            --standard|--normal) tier="normal"; shift ;;
            --batch) tier="batch"; shift ;;
            --why) why="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"])" >&2; exit 2 ;;
            *) job="$1"; shift ;;
        esac
    done
    case "$tier" in
        ci|priority|batch) ;;
        normal|standard) tier="normal" ;;
        *) echo "usage: k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"]   (--priority needs --why)" >&2; exit 2 ;;
    esac
    [ -n "$job" ] || { echo "usage: k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"]" >&2; exit 2; }
    if [ "$tier" = "priority" ] && [ -z "$why" ]; then
        echo "usage: --priority needs a one-line reason: k2 compute retier $job --priority --why \"release gate for 0.46.6\"" >&2
        exit 2
    fi
    local body
    body="$(_compute_json K2_CJ_job="$job" K2_CJ_tier="$tier" K2_CJ_why="$why")"
    _compute_call POST /cli/compute/jobs/tier "$body"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
w = {"ci": "CI", "priority": "Priority", "normal": "Standard", "batch": "Batch"}
line = "job %s: %s → %s on %s" % (v["job"], w.get(v["from"], v["from"]), w.get(v["tier"], v["tier"]), v["node"])
if v.get("asked") and v["asked"] != v["tier"]:
    line += " (asked %s: this server'"'"'s Priority doesn'"'"'t count there)" % w.get(v["asked"], v["asked"])
print(line)
print("  " + v["note"])'
}

# k2 compute move <job>... --auto [--avoid N]... [--os O|any] [--arch A|any]
#   [--label k=v]... | --to <node>   (0.46.8): a QUEUED job to auto-placement
# (K2 moves it to a node that can start it) or to one named node, without a
# cancel. --all-mine / --workspace-all [--tag T] [--node N] pick the jobs as
# cancel does. The daemon checks everything; this only builds the body.
cmd_compute_move() {
    local to="" auto=0 os="" arch="" scope="" mtag="" mnode="" mws="" as_json=0
    local -a jobs=() avoids=() labels=()
    local musage="usage: k2 compute move <job>... --auto [--avoid <node>]... [--os O|any] [--arch A|any] [--label k=v]  |  k2 compute move <job>... --to <node>  |  k2 compute move --all-mine|--workspace-all [--tag T] [--node N] (--auto … | --to <node>)"
    while [ $# -gt 0 ]; do
        case "$1" in
            --auto) auto=1; shift ;;
            --to) to="${2:-}"; shift 2
                [ -n "$to" ] || { echo "$musage (--to needs a node)" >&2; exit 2; } ;;
            --avoid) avoids+=("${2:-}"); shift 2
                [ -n "${avoids[${#avoids[@]}-1]}" ] || { echo "$musage (--avoid needs a node)" >&2; exit 2; } ;;
            --os) os="${2:-}"; shift 2 ;;
            --arch) arch="${2:-}"; shift 2 ;;
            --label) labels+=("${2:-}"); shift 2 ;;
            --all-mine) scope="session"; shift ;;
            --workspace-all) scope="workspace"; shift ;;
            --tag) mtag="${2:-}"; shift 2 ;;
            --node) mnode="${2:-}"; shift 2 ;;
            --workspace) mws="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "$musage (unknown flag '$1')" >&2; exit 2 ;;
            *) jobs+=("$1"); shift ;;
        esac
    done
    if [ "$auto" = "1" ] && [ -n "$to" ]; then echo "$musage (--auto or --to, not both)" >&2; exit 2; fi
    if [ "$auto" = "0" ] && [ -z "$to" ]; then echo "$musage (say where: --auto or --to <node>)" >&2; exit 2; fi
    if [ -n "$scope" ] && [ "${#jobs[@]}" -gt 0 ]; then echo "$musage (job ids or --all-mine, not both)" >&2; exit 2; fi
    if [ -z "$scope" ] && [ "${#jobs[@]}" -eq 0 ]; then echo "$musage (name a job, or --all-mine)" >&2; exit 2; fi
    if [ -z "$scope" ] && { [ -n "$mtag" ] || [ -n "$mnode" ]; }; then echo "$musage (--tag and --node pick jobs with --all-mine or --workspace-all)" >&2; exit 2; fi
    local avoid_json="" labels_json=""
    if [ "${#avoids[@]}" -gt 0 ]; then
        avoid_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${avoids[@]}")"
    fi
    if [ "${#labels[@]}" -gt 0 ]; then
        labels_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --label key=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${labels[@]}")" || exit 2
    fi
    local ab=""
    [ "$auto" = "1" ] && ab=1
    local render='
import json, sys
v = json.load(sys.stdin)
def one(r):
    f = r.get("filter") or {}
    extra = []
    if f.get("os") or f.get("arch"):
        extra.append("%s-%s" % (f.get("os") or "any", f.get("arch") or "any"))
    if f.get("avoid"):
        extra.append("avoids " + ", ".join(f["avoid"]))
    tail = (" (" + "; ".join(extra) + ")") if extra else ""
    if r.get("moved"):
        return "job %s: moved %s -> %s (%s)%s. %s" % (r["job"], r["from"], r["node"], r["mode"], tail, r.get("note") or "")
    if r.get("mode") == "auto":
        return "job %s: auto-placed now%s. %s" % (r["job"], tail, r.get("note") or "")
    return "job %s: %s" % (r["job"], r.get("note") or "pinned to " + str(r.get("node")))
# 0.46.12: an --avoid name that is not one of your nodes is a note.
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % w)
if "matched" in v:
    m = v.get("moved") or []
    print("moved %d of %d matching queued job(s)" % (len(m), v.get("matched", 0)))
    for r in m:
        print("  " + one(r))
    for s in v.get("skipped") or []:
        print("  skipped %s: %s (%s)" % (s.get("job"), s.get("code"), s.get("hint")))
else:
    print(one(v))'
    if [ -n "$scope" ]; then
        local filt
        filt="$(K2_CF_SCOPE="$scope" K2_CF_TAG="$mtag" K2_CF_NODE="$mnode" K2_CF_WS="$(_compute_ws "$mws")" python3 -c '
import json, os
f = {"scope": os.environ["K2_CF_SCOPE"]}
for k, e in (("tag", "K2_CF_TAG"), ("node", "K2_CF_NODE"), ("workspace", "K2_CF_WS")):
    if os.environ.get(e):
        f[k] = os.environ[e]
print(json.dumps(f))')"
        _compute_call POST /cli/compute/jobs/move "$(_compute_json K2_CJ_filter_j="$filt" K2_CJ_auto_b="$ab" K2_CJ_to="$to" \
            K2_CJ_avoid_j="$avoid_json" K2_CJ_os="$os" K2_CJ_arch="$arch" K2_CJ_labels_j="$labels_json")"
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
        printf '%s' "$_CP_BODY" | python3 -c "$render"
        exit 0
    fi
    # One job: refusals exit as for every compute verb (2 usage, 3 refused).
    # Several: each is tried; the worst exit wins, every refusal is printed.
    local j rc worst=0
    for j in "${jobs[@]}"; do
        (
            _compute_call POST /cli/compute/jobs/move "$(_compute_json K2_CJ_job="$j" K2_CJ_auto_b="$ab" K2_CJ_to="$to" \
                K2_CJ_avoid_j="$avoid_json" K2_CJ_os="$os" K2_CJ_arch="$arch" K2_CJ_labels_j="$labels_json")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | python3 -c "$render"; fi
        )
        rc=$?
        [ "$rc" -gt "$worst" ] && worst="$rc"
    done
    exit "$worst"
}

cmd_compute_local() {
    local sub="${1:-status}"
    shift 2>/dev/null || true
    local body
    case "$sub" in
        status)
            _compute_call GET /cli/compute/local/status ""
            if [ "${1:-}" = "--json" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v.get("binary") or {}
hc = v.get("homeCheck") or {}
if not v.get("installed"):
    print("This computer is not a compute node.")
    print("  k2-node: %s" % (("%s (%s, %s)" % (b.get("path"), b.get("source"), b.get("version") or "?")) if b else "not on this machine (Linux: downloaded and signature-checked on enable)"))
    print("  root: %s" % {"helper": "the root helper installs it (no password)", "prompt": "an admin prompt or one sudo command", "command": "one sudo command (or passwordless sudo, tried on enable)"}.get(v.get("elevation"), "one sudo command"))
    if hc:
        print("  home: %s is %s (mode %s)" % (hc.get("home"), "private" if hc.get("private") else "readable by other users", hc.get("mode")))
    print("Enable: k2 compute local enable [--controller URL --enroll CODE.FP --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]")
    sys.exit(0)
n = v.get("node") or {}
c = v.get("control") or {}
print("node %s: %s%s" % (n.get("name", "?"), n.get("state", "unknown"), (" via " + n["route"]) if n.get("route") else ""))
print("control: %s%s" % (c.get("state", "active"), (" by %s" % c["by"]) if c.get("by") else ""))
if n.get("running"): print("running: " + " ".join(n["running"]))
if n.get("last_error"): print("last error: " + str(n["last_error"]))
if n.get("sas"): print("waiting for confirmation, code " + str(n["sas"]))
r = v.get("enrollResult") or {}
if r and not r.get("ok"): print("last enroll request failed: " + str(r.get("message")))
p = v.get("packages") or {}
if p.get("approved"): print("approved packages: " + " ".join(p["approved"]))
if hc and not hc.get("private"): print("note: %s is readable by other users (mode %s); jobs can read files there that are readable by everyone" % (hc.get("home"), hc.get("mode")))
ra = v.get("remoteAdmin")
print("remote admin: %s" % (ra or "not set up"))
if v.get("remoteAdminCommand"): print("  K2 can\x27t update this node. Turn it on: k2 compute local remote-admin policy (or: %s)" % v["remoteAdminCommand"])
'
            ;;
        pause)
            local now=0
            [ "${1:-}" = "--now" ] && now=1
            body="$(_compute_json K2_CJ_now_b="$now")"
            _compute_call POST /cli/compute/local/pause "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["control"], v["note"]))'
            ;;
        drain|resume)
            _compute_call POST "/cli/compute/local/$sub" "{}" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["control"], v["note"]))'
            ;;
        policy)
            if [ $# -eq 0 ]; then
                _compute_call GET /cli/compute/local/status ""
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("policy"), indent=2))'
                exit 0
            fi
            body="$(python3 -c '
import json, sys
s = {}
for a in sys.argv[1:]:
    if "=" not in a:
        sys.stderr.write("usage: k2 compute local policy key=value...\n"); sys.exit(2)
    k, v = a.split("=", 1); s[k.replace("-", "_")] = v
print(json.dumps({"set": s}))' "$@")" || exit 2
            _compute_call POST /cli/compute/local/policy "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["policy"], indent=2))'
            ;;
        enable)
            local controller="" enroll="" ticket="" name="" private="" remote_admin=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --controller) controller="${2:-}"; shift 2 ;;
                    --enroll|--code) enroll="${2:-}"; shift 2 ;;
                    --ticket) ticket="${2:-}"; shift 2 ;;
                    --name) name="${2:-}"; shift 2 ;;
                    --private-home) private=1; shift ;;
                    --no-private-home) private=0; shift ;;
                    --remote-admin)
                        remote_admin="${2:-}"; shift 2
                        case "$remote_admin" in off|policy|full) ;; *) echo "--remote-admin is off, policy or full" >&2; exit 2 ;; esac
                        ;;
                    *) echo "usage: k2 compute local enable [--controller URL --enroll CODE.FP --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]" >&2; exit 2 ;;
                esac
            done
            body="$(_compute_json K2_CJ_controller="$controller" K2_CJ_enroll="$enroll" K2_CJ_ticket="$ticket" K2_CJ_name="$name" K2_CJ_privateHome_b="$private" K2_CJ_remoteAdmin="$remote_admin")"
            _compute_call POST /cli/compute/local/enable "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
if v.get("sas"):
    print("Code: %s — confirm it on the controller: k2 compute node confirm <name> <code>" % v["sas"])
'
            ;;
        remote-admin)
            case "${1:-}" in
                off|policy|full) ;;
                *) echo "usage: k2 compute local remote-admin off|policy|full" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/compute/local/remote-admin "$(_compute_json K2_CJ_level="$1")" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
'
            ;;
        disable)
            # Q2 (Rosson 2026-10-24): an owner stop stops the jobs running
            # here; name them first, and ask (or need --yes) when any run.
            local yes=0
            [ "${1:-}" = "--yes" ] && yes=1
            _compute_call GET /cli/compute/local/status ""
            local running
            running="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(" ".join((v.get("node") or {}).get("running") or []))')"
            if [ -n "$running" ] && [ "$yes" = "0" ]; then
                echo "These jobs are running here and stop now: $running" >&2
                if [ -t 0 ]; then
                    printf "Stop being a compute node? [y/N] " >&2
                    local answer=""
                    read -r answer || answer=""
                    case "$answer" in y|Y|yes) ;; *) echo "not stopped" >&2; exit 2 ;; esac
                else
                    echo "pass --yes to stop them (k2 compute local disable --yes)" >&2
                    exit 2
                fi
            fi
            _compute_call POST /cli/compute/local/disable "{}" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
'
            ;;
        *)
            echo "usage: k2 compute local status|pause [--now]|drain|resume|policy|enable|remote-admin|disable" >&2
            exit 2
            ;;
    esac
}

# --gpu VALUE (0.46.11): any, amd or intel. NVIDIA isn't supported yet.
_compute_gpu_arg() {
    case "${1:-}" in
        any|amd|intel) return 0 ;;
        nvidia) echo "usage: --gpu nvidia: NVIDIA GPU jobs aren't supported yet; use --gpu any, amd or intel" >&2; return 2 ;;
        *) echo "usage: --gpu [any|amd|intel] (one GPU of the node, to this job alone; bare --gpu = any)" >&2; return 2 ;;
    esac
}

# --distro VALUE (0.46.9): ubuntu, ubuntu@24.04, debian, arch, fedora…
# (the node's ID or ID_LIKE; the version matches on a dot prefix).
_compute_distro_arg() {
    if [[ "${1:-}" =~ ^[a-z0-9][a-z0-9._-]{0,39}(@[0-9A-Za-z][0-9A-Za-z._-]{0,19})?$ ]]; then
        printf '%s' "$1"
    else
        echo "usage: --distro ID[@VERSION] (ubuntu, ubuntu@24.04, debian, arch, fedora: the ID or ID_LIKE in the node's /etc/os-release; k2 compute nodes shows each node's)" >&2
        return 2
    fi
}

# Pretty JSON of the last answer (the trust verbs print what the daemon said).
_compute_trust_print() {
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), indent=2))'
}

# A person's confirm for a trust request (bound to its id), then approve.
_compute_trust_approve() {
    local id="${1:-}"
    [ -n "$id" ] || { echo "usage: k2 compute trust approve <ctr_…>" >&2; exit 2; }
    if _compute_in_session; then
        echo '{"error":"person_only","hint":"a person approves this, from a terminal outside K2 (Touch ID / vault password). An agent never approves."}' >&2
        exit 3
    fi
    _compute_call GET /cli/compute/trust/requests "state=all" tcp
    local found full state label digest
    found="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin)["requests"] if r["id"] == want or r["id"].startswith(want)]
if len(rows) != 1:
    sys.exit(1)
r = rows[0]
print(r["id"]); print(r["state"]); print(r["summary"][:64]); print(r["digest"]); print("---"); print(r["summary"])
if r.get("reason"): print("reason: " + r["reason"])
' "$id")" || { echo "{\"error\":\"request_not_found\",\"hint\":\"no single request matches '$id' (k2 compute trust)\"}" >&2; exit 3; }
    full="$(printf '%s\n' "$found" | sed -n 1p)"
    state="$(printf '%s\n' "$found" | sed -n 2p)"
    label="$(printf '%s\n' "$found" | sed -n 3p)"
    # The digest of what is printed here: the daemon refuses the approval
    # when the request changed since (request_changed). The Touch ID
    # sheet's words are the daemon's own.
    digest="$(printf '%s\n' "$found" | sed -n 4p)"
    if [ "$state" != "pending" ]; then
        echo "{\"error\":\"not_pending\",\"hint\":\"$full is $state\"}" >&2
        exit 3
    fi
    printf '%s\n' "$found" | sed -n '6,$p' >&2
    local tok
    tok="$(_vault_confirm_token compute-trust "$full" "$label")" || exit 3
    K2_CP_MAX_TIME=300 _compute_call POST /cli/compute/trust/approve "$(_compute_json K2_CJ_request="$full" K2_CJ_expectedDigest="$digest" K2_CJ_confirmToken="$tok")" tcp
    tok=""
    _compute_trust_print
    # A seat that asked for secrets: the seat's approval didn't give them.
    # The consent is its own yes, with its own words and confirm.
    local next
    next="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(((json.load(sys.stdin).get("result") or {}).get("consentNext")) or "")')"
    if [ -n "$next" ]; then
        echo "The seat asked to send vault secrets to that owner's nodes. That is a separate yes: $next" >&2
    fi
}

# k2 compute roster|lanes|seats|trust|owners (0.46.10 C1/C3a).
cmd_compute_trust() {
    local noun="$1"; shift
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$noun" in
        roster)
            local sub="${1:-show}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                show|--json)
                    _compute_call GET /cli/compute/roster ""
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
l = v.get("latest")
if l:
    print("node roster seq %s (key %s…), issued %s%s" % (l["seq"], l["keyFingerprint"][:16], l["issuedAt"], ", STALE: k2 compute roster publish --resign" if v.get("stale") else ""))
    for e in l["entries"]:
        print("  %-16s %-8s %-15s host %s  key %s…" % (e["name"], e["status"], e.get("taintBasis") or "-", e["host"], e["nodeFp"][:12]))
else:
    print("no node roster yet: k2 compute roster set <node> …, then a person approves it")
for n in v.get("nodes", []):
    st = n.get("statement") or {}
    print("node %-16s statement %s, taint %s, on roster: %s" % (n["name"], "yes" if st else "none (k2-node older than protocol 9)", st.get("taint", "-"), n.get("roster") or "no"))
for r in v.get("pending", []):
    print("waiting: %s  %s" % (r["id"], r["summary"]))
'
                    ;;
                key)
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"the roster key is minted and shown to a person: run it from a terminal outside K2"}' >&2
                        exit 3
                    fi
                    _compute_call POST /cli/compute/roster/key "{}" tcp
                    _compute_trust_print
                    ;;
                set|attest)
                    local node="${1:-}"; [ -n "$node" ] && shift
                    [ -n "$node" ] || { echo "usage: k2 compute roster set <node> [--host H] [--provider P] [--site S] [--admin-reach a,b] [--attest TEXT]" >&2; exit 2; }
                    local host="" prov="" site="" reach="" att="" why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --host) host="${2:-}"; shift 2 ;;
                            --provider) prov="${2:-}"; shift 2 ;;
                            --site) site="${2:-}"; shift 2 ;;
                            --admin-reach) reach="${2:-}"; shift 2 ;;
                            --attest) att="${2:-}"; shift 2 ;;
                            --why|--reason) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute roster set <node> [--host H] [--provider P] [--site S] [--admin-reach a,b] [--attest TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/roster/propose "$(_compute_json K2_CJ_op=set K2_CJ_node="$node" K2_CJ_host="$host" K2_CJ_provider="$prov" K2_CJ_site="$site" K2_CJ_adminReach="$reach" K2_CJ_attest="$att" K2_CJ_reason="$why")"
                    _compute_trust_print
                    ;;
                retire|revoke)
                    local node="${1:-}"; [ -n "$node" ] && shift
                    [ -n "$node" ] || { echo "usage: k2 compute roster $sub <node>" >&2; exit 2; }
                    local reason=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --reason) reason="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute roster $sub <node> [--reason compromised|key_lost|other]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/roster/propose "$(_compute_json K2_CJ_op="$sub" K2_CJ_node="$node" K2_CJ_reason="$reason")"
                    _compute_trust_print
                    ;;
                publish)
                    local resign=""
                    for a in "$@"; do [ "$a" = "--resign" ] && resign=1; done
                    K2_CP_MAX_TIME=300 _compute_call POST /cli/compute/roster/publish "$(_compute_json K2_CJ_resign_b="$resign")"
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute roster [show|key|set|retire|revoke|publish] (k2 compute --help)" >&2; exit 2 ;;
            esac
            ;;
        lanes)
            if [ "${1:-}" = "set" ]; then
                shift
                local name="${1:-}"; [ -n "$name" ] && shift
                [ -n "$name" ] || { echo "usage: k2 compute lanes set <name> [--from ci-standard] [key=value …]" >&2; exit 2; }
                local from="" sets="{}" why=""
                while [ $# -gt 0 ]; do
                    case "$1" in
                        --from) from="${2:-}"; shift 2 ;;
                        --why|--reason) why="${2:-}"; shift 2 ;;
                        --json) shift ;;
                        *=*)
                            sets="$(printf '%s' "$sets" | K2_LS_KV="$1" python3 -c '
import json, os, sys
d = json.load(sys.stdin)
k, v = os.environ["K2_LS_KV"].split("=", 1)
try:
    d[k] = json.loads(v)
except Exception:
    d[k] = v.split(",") if k in ("isolation", "profiles") else v
print(json.dumps(d))')"
                            shift ;;
                        *) echo "usage: k2 compute lanes set <name> [--from ci-standard] [key=value …]" >&2; exit 2 ;;
                    esac
                done
                _compute_call POST /cli/compute/lanes/set "$(_compute_json K2_CJ_name="$name" K2_CJ_from="$from" K2_CJ_set_j="$sets" K2_CJ_reason="$why")"
                _compute_trust_print
                return 0
            fi
            _compute_call GET /cli/compute/lanes ""
            if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v["lanes"]:
    print("no approved lanes: k2 compute lanes set ci-standard --from ci-standard")
for l in v["lanes"]:
    print("%s: %s (approved by %s)" % (l["name"], l["summary"], l["approvedBy"]))
'
            ;;
        seats)
            local sub="${1:-list}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                list|--json)
                    _compute_call GET /cli/compute/seats ""
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for s in v["offered"]:
    print("offered %-24s to %-24s lane %-12s %-9s nodes %s" % (s["id"], s["clientLabel"], s["lane"], s["state"], ",".join(s["nodes"])))
for s in v["held"]:
    print("held    %-24s on %-24s lane %-12s %-9s nodes %s%s" % (s["id"], s["ownerId"], s["lane"], s["state"], ",".join(s["nodes"]) or "-", ("  (" + s["note"] + ")") if s.get("note") else ""))
if not v["offered"] and not v["held"]:
    print("no seats")
'
                    ;;
                request)
                    local owner="${1:-}"; [ -n "$owner" ] && shift
                    [ -n "$owner" ] || { echo "usage: k2 compute seats request <owner-host> [--lane L] [--seat ID] [--secrets] [--why TEXT]" >&2; exit 2; }
                    # Secrets are off unless asked for (--secrets); even then
                    # the seat's approval never gives them: a person agrees
                    # on their own confirm (k2 compute owners consent … on).
                    local lane="" seat="" secrets="0" why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --lane) lane="${2:-}"; shift 2 ;;
                            --seat) seat="${2:-}"; shift 2 ;;
                            --secrets) secrets="1"; shift ;;
                            --no-secrets) secrets="0"; shift ;;
                            --why) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats request <owner-host> [--lane L] [--seat ID] [--secrets] [--why TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/seats/request "$(_compute_json K2_CJ_owner="$owner" K2_CJ_lane="$lane" K2_CJ_seat="$seat" K2_CJ_secrets_b="$secrets" K2_CJ_why="$why")"
                    _compute_trust_print
                    ;;
                offer)
                    local req="${1:-}"; [ -n "$req" ] && shift
                    local nodes=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --nodes) nodes="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats offer <request> --nodes a,b,c" >&2; exit 2 ;;
                        esac
                    done
                    { [ -n "$req" ] && [ -n "$nodes" ]; } || { echo "usage: k2 compute seats offer <request> --nodes a,b,c" >&2; exit 2; }
                    _compute_call POST /cli/compute/seats/offer "$(_compute_json K2_CJ_request="$req" K2_CJ_nodes="$nodes")"
                    _compute_trust_print
                    ;;
                end|suspend|resume)
                    # (owner) The owner, in person: the seat's statement is
                    # signed again and its nodes' plans use the owner key.
                    local seat="${1:-}"; [ -n "$seat" ] && shift
                    local why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --why) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats $sub <seat> [--why TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    [ -n "$seat" ] || { echo "usage: k2 compute seats $sub <seat> [--why TEXT]" >&2; exit 2; }
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"the node owner ends, suspends or resumes a seat in person, from a terminal outside K2 (Touch ID / vault password)"}' >&2
                        exit 3
                    fi
                    local tok
                    tok="$(_vault_confirm_token compute-trust "seat-$sub:$seat" "")" || exit 3
                    K2_CP_MAX_TIME=300 _compute_call POST "/cli/compute/seats/$sub" "$(_compute_json K2_CJ_seat="$seat" K2_CJ_why="$why" K2_CJ_confirmToken="$tok")" tcp
                    tok=""
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute seats [list|request|offer|end|suspend|resume] (k2 compute --help)" >&2; exit 2 ;;
            esac
            ;;
        trust)
            local sub="${1:-requests}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                requests|list|ls|--json|--all)
                    local st="open"
                    for a in "$sub" "$@"; do [ "$a" = "--all" ] && st="all"; done
                    _compute_call GET /cli/compute/trust/requests "state=$st"
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v["requests"]:
    print("%s  %-9s %-12s %s" % (r["id"], r["state"], r["kind"], r["summary"]))
if not v["requests"]:
    print("nothing waits for a person")
'
                    ;;
                approve) _compute_trust_approve "${1:-}" ;;
                deny)
                    local id="${1:-}"; [ -n "$id" ] && shift
                    local why=""
                    [ "${1:-}" = "--why" ] && why="${2:-}"
                    [ -n "$id" ] || { echo "usage: k2 compute trust deny <id> [--why TEXT]" >&2; exit 2; }
                    _compute_call POST /cli/compute/trust/deny "$(_compute_json K2_CJ_request="$id" K2_CJ_why="$why")"
                    _compute_trust_print
                    ;;
                withdraw)
                    [ -n "${1:-}" ] || { echo "usage: k2 compute trust withdraw <id>" >&2; exit 2; }
                    _compute_call POST /cli/compute/trust/withdraw "$(_compute_json K2_CJ_request="$1")"
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute trust [requests|approve|deny|withdraw]" >&2; exit 2 ;;
            esac
            ;;
        owners)
            if [ "${1:-}" = "consent" ]; then
                local owner="${2:-}" onoff="${3:-}"
                case "$onoff" in on|off) ;; *) echo "usage: k2 compute owners consent <owner> on|off" >&2; exit 2 ;; esac
                if [ "$onoff" = "on" ]; then
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"sending vault secrets to another owner'"'"'s nodes is a person'"'"'s choice: a terminal outside K2"}' >&2
                        exit 3
                    fi
                    _compute_call GET /cli/compute/owners "" tcp
                    local kfp
                    kfp="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
o=sys.argv[1]
r=[t for t in json.load(sys.stdin)["trusted"] if t["ownerId"]==o or t["keyFp"]==o]
print(r[0]["keyFp"] if len(r)==1 else "")' "$owner")"
                    [ -n "$kfp" ] || { echo "{\"error\":\"node_owner_not_trusted\",\"hint\":\"no pinned node owner $owner (k2 compute owners)\"}" >&2; exit 3; }
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=1 K2_CJ_preview_j=true)" tcp
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print(v["text"])
print("  %s'"'"'s node-roster key: %s" % (v["owner"], v["keyFp"]))' >&2
                    local tok
                    tok="$(_vault_confirm_token compute-trust "consent:$kfp" "Send vault secrets to $owner's nodes")" || exit 3
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=1 K2_CJ_confirmToken="$tok")" tcp
                else
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=0)"
                fi
                _compute_trust_print
                return 0
            fi
            _compute_call GET /cli/compute/owners ""
            if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("self")
if s:
    print("this server (%s): node-roster key %s" % (s["ownerId"], s["key"]["fingerprint"]))
for t in v["trusted"]:
    l = t.get("latest") or {}
    print("%s: key %s, roster seq %s, secrets %s" % (t["ownerId"], t["keyFp"], l.get("seq", "-"), "on" if t["secretsConsent"] else "off"))
if not s and not v["trusted"]:
    print("no node owners pinned, and this server has no node-roster key yet")
'
            ;;
    esac
}

cmd_compute() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _compute_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        # k2 compute node packages --help: its own page (0.46.9).
        if [ "$sub" = "node" ] && { [ "${1:-}" = "packages" ] || [ "${1:-}" = "pkg" ]; }; then
            _compute_node_packages_usage
            return 0
        fi
        # k2 compute git-access --help: its own page (0.46.9).
        if [ "$sub" = "git-access" ]; then
            _compute_git_access_usage
            return 0
        fi
        _compute_usage
        return 0
    fi
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$sub" in
        nodes|ls)
            # Outside a K2 tab, name this checkout's workspace so each row
            # shows its grant (a passport's comes from the passport).
            local nq="" nws
            nws="$(_compute_ws "")"
            [ -n "$nws" ] && nq="workspace=$(urlencode "$nws")"
            _compute_call GET /cli/compute/nodes "$nq"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render nodes
            ;;
        info)
            [ -n "${1:-}" ] || { echo "usage: k2 compute info <node>" >&2; exit 2; }
            _compute_call GET /cli/compute/nodes/info "node=$(urlencode "$1")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render info
            ;;
        run) cmd_compute_run "$@" ;;
        # CI seats on shared nodes (0.46.10 C1/C3a).
        roster|lanes|seats|trust|owners) cmd_compute_trust "$sub" "$@" ;;
        retier|tier) cmd_compute_retier "$@" ;;
        move) cmd_compute_move "$@" ;;
        allow-nodes) cmd_compute_allow_nodes "$@" ;;
        git-access) cmd_compute_git_access "$@" ;;
        cache) cmd_compute_cache "$@" ;;
        queue)
            [ -n "${1:-}" ] && [ "${1#-}" = "$1" ] || { echo "usage: k2 compute queue <node> [--json]" >&2; exit 2; }
            _compute_call GET /cli/compute/queue "node=$(urlencode "$1")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render queue
            ;;
        jobs)
            local q="" n=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --workspace) q="$q&workspace=$(urlencode "${2:-}")"; shift 2 ;;
                    --mine) q="$q&mine=1"; shift ;;
                    # 0.46.7 (CE-9): the job's own --tag.
                    --tag) q="$q&tag=$(urlencode "${2:-}")"; shift 2 ;;
                    --tier)
                        case "${2:-}" in
                            ci|priority|normal|batch) q="$q&tier=$(urlencode "${2:-}")" ;;
                            standard) q="$q&tier=normal" ;;
                            *) echo "usage: --tier ci|priority|standard|batch" >&2; exit 2 ;;
                        esac
                        shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute jobs [--node N] [--mine] [--state S] [--tier T] [--tag T] [--limit N]" >&2; exit 2 ;;
                esac
            done
            n="${q#&}"
            _compute_call GET /cli/compute/jobs "$n"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render jobs
            ;;
        get|status)
            # `status` with no job: every node at a glance; `status <job>` = get.
            if [ "$sub" = "status" ] && { [ -z "${1:-}" ] || [ "${1#-}" != "$1" ]; }; then
                cmd_compute_status "$@"
            fi
            [ -n "${1:-}" ] || { echo "usage: k2 compute get <job> [--full] [--json]" >&2; exit 2; }
            local gfull=0 ga
            for ga in "$@"; do [ "$ga" = "--full" ] && gfull=1; done
            _CP_404_EXIT=4 _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$1")"
            # --json: the one result object (compute B), as run --json prints.
            [ "$as_json" = "1" ] && { printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result"), indent=2))'; exit 0; }
            K2_CP_FULL="$gfull" _compute_render get
            ;;
        fetch) cmd_compute_fetch "$@" ;;
        test) cmd_compute_test "$@" ;;
        prewarm) cmd_compute_prewarm "$@" ;;
        attach)
            local job="" since="" raw=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --since) since="${2:-}"; shift 2 ;;
                    --raw) raw=1; shift ;;
                    *) job="$1"; shift ;;
                esac
            done
            [ -n "$job" ] || { echo "usage: k2 compute attach <job> [--since SEQ]" >&2; exit 2; }
            _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$job")"
            job="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
            _compute_follow "$job" "$raw" "seq:${since:-0}"
            ;;
        logs) cmd_compute_logs "$@" ;;
        wait) cmd_compute_wait "$@" ;;
        cancel)
            # 0.46.1: a job that already left the queue is stopped only with
            # --force (the CLI sends ifState: queued without it; 409
            # job_started → exit 3). --force goes before or after the id.
            # 0.46.7 (CE-20): --all-mine cancels this session's jobs that
            # match (--workspace-all: every job of the workspace); queued
            # only unless --force.
            local cjob="" force=0 scope="" ctag="" cstate="" cnode="" cws="" ifq=0
            local cusage="usage: k2 compute cancel <job> [--force|--if-queued]  |  k2 compute cancel --all-mine|--workspace-all [--tag T] [--state queued|running|any] [--node N] [--force]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --force|-f) force=1; shift ;;
                    # 0.46.7: cancel only while queued; a started job is
                    # reported and left running, exit 0 (for scripts).
                    --if-queued) ifq=1; shift ;;
                    --all-mine) scope="session"; shift ;;
                    --workspace-all) scope="workspace"; shift ;;
                    --tag) ctag="${2:-}"; shift 2 ;;
                    --state) cstate="${2:-}"; shift 2 ;;
                    --node) cnode="${2:-}"; shift 2 ;;
                    --workspace) cws="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$cusage (unknown flag '$1')" >&2; exit 2 ;;
                    *)
                        [ -z "$cjob" ] || { echo "$cusage (one job)" >&2; exit 2; }
                        cjob="$1"; shift ;;
                esac
            done
            if [ -n "$scope" ]; then
                [ -z "$cjob" ] || { echo "$cusage (a job id or --all-mine, not both)" >&2; exit 2; }
                case "${cstate:-queued}" in queued|running|any) ;; *) echo "$cusage (--state is queued, running or any)" >&2; exit 2 ;; esac
                if [ "${cstate:-queued}" != "queued" ] && [ "$force" != "1" ]; then
                    echo "[k2 compute] --state ${cstate} stops started jobs only with --force; without it they are listed as skipped" >&2
                fi
                local filt
                filt="$(K2_CF_SCOPE="$scope" K2_CF_TAG="$ctag" K2_CF_STATE="$cstate" K2_CF_NODE="$cnode" K2_CF_WS="$(_compute_ws "$cws")" python3 -c '
import json, os
f = {"scope": os.environ["K2_CF_SCOPE"]}
for k, e in (("tag", "K2_CF_TAG"), ("state", "K2_CF_STATE"), ("node", "K2_CF_NODE"), ("workspace", "K2_CF_WS")):
    if os.environ.get(e):
        f[k] = os.environ[e]
print(json.dumps(f))')"
                local fb=0
                [ "$force" = "1" ] && fb=1
                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_filter_j="$filt" K2_CJ_force_b="$fb")"
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
c = v.get("cancelled") or []
print("cancelled %d of %d matching job(s)%s" % (len(c), v.get("matched", 0), (": " + " ".join(x["job"] for x in c)) if c else ""))
for s in v.get("skipped") or []:
    print("  skipped %s: %s (%s)" % (s.get("job"), s.get("code"), s.get("hint")))'
                exit 0
            fi
            [ -n "$cjob" ] || { echo "$cusage" >&2; exit 2; }
            [ "$ifq" = "1" ] && [ "$force" = "1" ] && { echo "$cusage (--if-queued or --force, not both)" >&2; exit 2; }
            local if_state="queued"
            [ "$force" = "1" ] && if_state=""
            if [ "$ifq" = "1" ]; then
                local craw
                craw="$(_compute_send POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState=queued)")"
                _compute_split "$craw"
                if [ "$_CP_STATUS" = "409" ] && printf '%s' "$_CP_BODY" | grep -Eq '"code":"(job_started|already_ended)"'; then
                    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                    printf '%s' "$_CP_BODY" | K2_CP_JOB="$cjob" python3 -c '
import json, os, sys
e = (json.load(sys.stdin).get("error") or {})
job = os.environ["K2_CP_JOB"][:8]
if e.get("code") == "already_ended":
    print("not cancelled: %s already ended (%s)" % (job, e.get("hint")))
else:
    print("not cancelled: %s already started; left running (k2 compute cancel %s --force stops it)" % (job, job))'
                    exit 0
                fi
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            else
                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState="$if_state")"
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | K2_CP_JOB="$cjob" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
job = os.environ["K2_CP_JOB"][:8]
node = v.get("node") or "?"
def took(s):
    if s is None: return ""
    s = int(s)
    return " for %dm%02ds" % (s // 60, s % 60)
if v.get("state") == "cancelled" and v.get("was") == "queued":
    print("cancelled %s (was queued on %s)" % (job, node))
elif v.get("state") == "cancel_requested" and v.get("onNode"):
    print("cancel asked for %s (queued on %s; %s): %s" % (job, node, v.get("cancel"), v.get("note") or ""))
elif "cancel" in v:
    print("stopping %s (was %s on %s%s; cancel %s)" % (job, v.get("was") or v.get("state"), node, took(v.get("ranSecs")), v["cancel"]))
else:
    print(v.get("state", "ok"))'
            ;;
        retry)
            [ -n "${1:-}" ] || { echo "usage: k2 compute retry <job>" >&2; exit 2; }
            _compute_call POST /cli/compute/jobs/retry "$(_compute_json K2_CJ_job="$1")"
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "job" in v: print("retrying as attempt %s: k2 compute attach %s" % (v["job"]["attempt"], v["job"]["short"]))
else: print(v.get("state", "ok"))'
            ;;
        receipt)
            [ -n "${1:-}" ] || { echo "usage: k2 compute receipt <job>" >&2; exit 2; }
            _compute_call GET /cli/compute/jobs/receipt "job=$(urlencode "$1")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render receipt
            ;;
        node) cmd_compute_node "$@" ;;
        grant) cmd_compute_grant "$@" ;;
        revoke)
            local node="" workspace=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] && [ -n "$workspace" ] || { echo "usage: k2 compute revoke <node> --workspace WS" >&2; exit 2; }
            _compute_call POST /cli/compute/grants/revoke "$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$workspace")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(("revoked %s for %s. " % (v["node"], v["workspace"]) if v["removed"] else "no grant to revoke. ") + v["note"])'
            ;;
        grants)
            local q=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --workspace) q="$q&workspace=$(urlencode "${2:-}")"; shift 2 ;;
                    *) shift ;;
                esac
            done
            _compute_call GET /cli/compute/grants "${q#&}"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render grants
            ;;
        preview)
            # The owner's switch for the whole preview (CN30, `computePreview`).
            # Not a compute route, so it works while compute is off. Owner
            # only: run it from a plain terminal, not a K2 tab (a tab sends
            # its session passport and the daemon refuses).
            local action="${1:-status}"
            case "$action" in
                on|off)
                    _compute_call POST /cli/settings/update "{\"computePreview\":$([ "$action" = on ] && echo true || echo false)}" tcp
                    ;;
                status) _compute_call GET /cli/settings/get "" tcp ;;
                *) echo "usage: k2 compute preview on|off|status" >&2; exit 2 ;;
            esac
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("Compute nodes (preview): %s on this server" % ("on" if v.get("computePreview") else "off"))'
            ;;
        placement)
            # How auto-placement ranks nodes (0.46.6, `computePlacement`):
            # v2 (default) or v1 (the 0.46.5 order). The owner's switch,
            # like preview: a plain terminal, not a K2 tab.
            local action="${1:-status}"
            case "$action" in
                v1|v2) _compute_call POST /cli/settings/update "{\"computePlacement\":\"$action\"}" tcp ;;
                status) _compute_call GET /cli/settings/get "" tcp ;;
                *) echo "usage: k2 compute placement v1|v2|status" >&2; exit 2 ;;
            esac
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
p = json.load(sys.stdin).get("computePlacement") or "v2"
what = {"v2": "load bands, a warm slot worth one band, bursts spread", "v1": "the 0.46.5 order: warm slot first, then free CPU"}.get(p, p)
print("Compute placement: %s on this server (%s)" % (p, what))'
            ;;
        access)
            local action="${1:-status}" workspace="" all=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    --all-machines) all=1; shift ;;
                    *) echo "usage: k2 compute access on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off)
                    # The Agent tab's "Allow compute nodes" (the single
                    # control, Rosson 2026-10-11): on also grants every
                    # machine switched on (and ones added later); off removes
                    # the workspace's grants. --all-machines is accepted for
                    # old scripts; it's what plain on/off does now.
                    : "$all"
                    _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
on=v["agentsCanUseCompute"]
m=v.get("granted", []) + v.get("kept", [])
tail=((": " + ", ".join(m)) if m else ": no machines switched on yet (ones added later are granted)") if on else (" (%d grants removed)" % v.get("grantsRemoved", 0) if "grantsRemoved" in v else "")
print("Allow compute nodes: %s for %s%s" % ("on" if on else "off", v["workspace"], tail))'
                    ;;
                *) echo "usage: k2 compute access on|off --workspace WS" >&2; exit 2 ;;
            esac
            ;;
        operate)
            # Node admin (Q14): "Allow this agent to operate compute nodes".
            local action="${1:-}" workspace=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute operate on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off) ;;
                *) echo "usage: k2 compute operate on|off --workspace WS" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute_operate K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("Allow this agent to operate compute nodes: %s for %s%s" % ("on" if v["agentsCanOperateCompute"] else "off", v["workspace"], " (narrow, pause, update, restart; root actions still wait for you)" if v["agentsCanOperateCompute"] else ""))'
            ;;
        manage)
            # "Allow managing compute nodes" (0194): owner only. Inside a K2
            # terminal this sends the passport, which the daemon refuses
            # (an agent never turns it on, for itself or anyone).
            local action="${1:-}" workspace=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute manage on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off) ;;
                *) echo "usage: k2 compute manage on|off --workspace WS" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute_manage K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
on=v["agentsCanManageCompute"]
print("Allow managing compute nodes: %s for %s%s" % ("on" if on else "off", v["workspace"], " (node add/confirm/rename/pause/retire/route, grant/revoke; each act is in Access Audit; preview and node remove stay yours)" if on else ""))'
            ;;
        usage)
            local days=7
            [ "${1:-}" = "--days" ] && days="${2:-7}"
            _compute_call GET /cli/compute/usage "days=$(urlencode "$days")" tcp
            _compute_render usage
            ;;
        events)
            # 0.46.7 (compute feedback part 2, item 4): your jobs' events
            # (queued, started, finished with exit, requeued, moved), so an
            # agent follows them without polling logs. --json = NDJSON.
            # --audit: the owner's audit log (what `events` was before).
            local limit="" audit=0 efollow=0 emine=0 etag="" esince="" emax=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --audit) audit=1; shift ;;
                    --limit) limit="${2:-}"; shift 2 ;;
                    --follow|-f) efollow=1; shift ;;
                    --mine) emine=1; shift ;;
                    --tag) etag="${2:-}"; shift 2 ;;
                    --since) esince="${2:-}"; shift 2 ;;
                    --max-wait) emax="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute events [--follow] [--mine] [--tag T] [--since CURSOR] [--limit N] [--max-wait D] [--json]  |  k2 compute events --audit [--limit N]" >&2; exit 2 ;;
                esac
            done
            if [ "$audit" = "1" ]; then
                _compute_call GET /cli/compute/events "limit=$(urlencode "${limit:-50}")" tcp
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                _compute_render events
                exit 0
            fi
            local eq="events=1" ews edeadline=""
            [ "$emine" = "1" ] && eq="$eq&mine=1"
            [ -n "$etag" ] && eq="$eq&tag=$(urlencode "$etag")"
            ews="$(_compute_ws "")"
            [ -n "$ews" ] && eq="$eq&workspace=$(urlencode "$ews")"
            [ -n "$emax" ] && edeadline=$(( $(date +%s) + $(_compute_secs "$emax") ))
            local cursor="$esince" first=1 eout
            while :; do
                local q="$eq" w=0
                if [ -n "$cursor" ]; then
                    q="$q&after=$(urlencode "$cursor")"
                elif [ "$first" = "1" ] && [ "$efollow" = "0" ]; then
                    q="$q&last=$(urlencode "${limit:-20}")"
                fi
                if [ "$efollow" = "1" ]; then
                    w=20
                    if [ -n "$edeadline" ]; then
                        local left=$(( edeadline - $(date +%s) ))
                        [ "$left" -lt 0 ] && left=0
                        [ "$left" -lt "$w" ] && w="$left"
                    fi
                    q="$q&wait=$w"
                fi
                _compute_call GET /cli/compute/jobs "$q"
                # The events, then the next cursor as the last line.
                eout="$(printf '%s' "$_CP_BODY" | K2_CP_JSON="$as_json" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
for e in v.get("events") or []:
    if os.environ.get("K2_CP_JSON") == "1":
        print(json.dumps(e, separators=(",", ":")))
    else:
        tail = ""
        if e.get("event") == "finished":
            tail = ": %s" % e.get("state")
            if e.get("exit") is not None:
                tail += " exit %s" % e["exit"]
            if e.get("reason"):
                tail += " (%s)" % e["reason"]
        print("%s  %s  %-9s on %s%s" % (time.strftime("%H:%M:%S", time.localtime(e.get("at") or 0)), e.get("short"), e.get("event"), e.get("node") or "?", tail))
print(v.get("cursor"))')"
                cursor="${eout##*$'\n'}"
                [ "$cursor" = "$eout" ] || printf '%s\n' "${eout%$'\n'*}"
                first=0
                [ "$efollow" = "1" ] || break
                if [ -n "$edeadline" ] && [ "$(date +%s)" -ge "$edeadline" ]; then
                    echo "[k2 compute] --max-wait passed; continue with: k2 compute events --follow --since $cursor" >&2
                    exit 0
                fi
            done
            echo "[k2 compute] next: k2 compute events --follow --since $cursor" >&2
            ;;
        local) cmd_compute_local "$@" ;;
        *)
            echo "usage: unknown compute verb '$sub' (k2 compute --help)" >&2
            exit 2
            ;;
    esac
}

