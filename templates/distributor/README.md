# Ship your own brand of K5: the distributor pipeline

This folder is a template. Copy it, whole, into a new private repo of your
own. With your brand file in it, it builds your brand of K5 from every K5
release, in step with K5, signs the build with your identity and puts it on
your own update feed.

The example distributor here is made up: its product is **Globex Studio**
and its command is **gy**. Change both to yours.

## What goes in your repo

Only these files. No K5 source is ever copied in: each build fetches K5's
public repo at one release tag, so every fix in K5 reaches you with the
next tag.

| file | what it is | yours to change |
|---|---|---|
| `brand/brand.toml` | your brand: its id, names, command, links, feeds and keys, each field explained in the file and in "Your brand file, field by field" below | yes: this is what makes the build yours |
| `distributor.conf` | the pipeline's settings: K5's repo, which tags are releases, the first tag you build, your feed and your update key's public half | yes |
| `hold.txt` | K5 tags you hold back | yes, whenever you need to |
| `hooks/sign.sh` | signs the build with your keys | yes: fit it to your signing |
| `hooks/publish.sh` | uploads the signed build to your feed | yes: fit it to your host |
| `pipeline/driver.sh` | the steps, in order; any CI can call it | no (see "What you must never change") |
| `.github/workflows/brand-build.yml` | the reference CI: when the driver runs, where, with which secret | the schedule, the runners, the environment's rules |

## Your brand's names

A brand has two names, and neither is made from the other:

- the **product** (`[names] product`): what people read, in prose, windows
  and messages;
- the **command** (`[cli] name`): what people and agents type. Every help
  page, study page, permission card, skill and `Ask with:` line of your
  build says it.

Both must pass K5's name policy, `scripts/brand.sh lint`. It refuses a
command that is a system command, a shell builtin, a PowerShell or cmd.exe
name, a developer tool or a common shell alias; K5's own names and their
look-alikes; a name another brand K5 knows of already has; and a name that
is a word in K5's own text. `--live` also asks the build host's shells and
the package indexes. Run it on your brand before you settle on the names:

    git clone --depth 1 --branch <K5 tag> <K5 repo> k5
    bash k5/scripts/brand.sh lint brand --live

The pipeline runs it again on every build, against the tag it builds: a
name that was free can start to collide later.

**Ask us to check and reserve your names before your first build.** Each
distributor's brand file lives in its own private repo, so the public lint
can't see another distributor's names. We keep a private list of the
brands we have reserved (command, product and id) and check new names
against it (`brand.sh lint <folder> --registry <our list>`). Send us your
command, product name and brand id; we check them, reserve them, and tell
you they are yours. Two distributors can then never ship the same command
or product name.

If two brands' programs ever meet on one machine anyway (one brand's
command run in a session of another brand's server), the CLI prints a
warning line naming both. That warning is the safety net on one machine;
the reservation is what keeps it from happening.

## Your brand file, field by field

`brand/brand.toml` holds everything your build and a hosted box of yours
need to be yours. A field you leave out is K5's own value (the default
brand, `brand/k2` in K5's repo), or is made from your names where the
table says so.

A field marked **must, your own** names K5 or reaches K5 when left out
(K5's company, sign-in name, site, support address, update feeds or
keys), so the build and `bash scripts/brand.sh lint brand` refuse a
brand that leaves it out or sets it to K5's own value, naming the field.
A field marked **must, any value** is refused when left out, but may be
K5's own value when you write it. `--hosted` also refuses a brand that
leaves out a field marked **must (--hosted)**, or sets one to K5's own
value.

Shapes: every URL is `https://<domain>[/<path>]`; domains are lower case;
an email is `<name>@<domain>`; a key is a minisign public key line (the
base64 line of its `.pub` file) with its key id; a colour is `#RRGGBB`;
`mail.system_from` may use `{server}` for its domain;
`""` means none (no link, or off). A table or key K5 does not know stops
the build, naming it.

| field | what it is | you |
|---|---|---|
| id (top level) | the brand's id: stamped into every binary; pick it once | **must** |
| `schema` | the brand file's format: `1` | **must** |
| `names.product` | the product's name, for prose and screens | **must** |
| `names.short` | the name where space is short | may (left out: the product's) |
| `names.company` | your company | **must**, your own |
| `names.lockup` | the product with its maker | may (left out: `<product> by <company>`) |
| `names.server` | what a box is called | may (left out: `<product> Server`) |
| `names.login` | the account your users sign in with | **must**, your own |
| `cli.name`, `cli.aliases` | the command and its other names ("Your brand's names" above) | **must** (aliases may be `[]`) |
| `cli.no_bash` | your build carries no shell CLI: the k5 program is the command, with no script inside or beside it and no fallback to one. Not taken from K5's file: left out it is `true` | may (left out: `true`); `--hosted` refuses `false` |
| `package.bundle_id`, `desktop_id`, `gui_binary` | the app's ids on macOS and Linux | **must** for app builds |
| `package.update_feed` | the desktop app's update manifest | **must**, your own |
| `update.keys` | the update keys your build trusts, primary first; never K5's. They sign your daemon, so they are also the trust root of the Sew pins it carries (there is no separate Sew signer). The template's key is a test-only placeholder that `--hosted` refuses | **must**, your own |
| `zones.label` | the zone your boxes are named under, `<subdomain>.<label>`: their only name (no second name under K5's own zone) | **must**, your own |
| `control.connect`, `cert`, `push`, `relay`, `relay_list`, `web_api` | Connect, the certificate broker, the push gateway, the relays, the website's API | leave out: they are ours. A brand that sets `[control]` sets all six |
| `feeds.daemon` | the headless daemon's update manifest | **must**, your own |
| `feeds.downloads` | the folder a release's files are downloaded from | **must**, your own |
| `machine.home`, `binary_prefix`, `unit_prefix` | the home folder (`.k2`), what programs and systemd units are named after (`k2-daemon`, `k2-daemon.service`); ops scripts find your boxes by these | leave out: a brand may not rename them yet |
| `look.icons` | your icons folder, inside your brand folder | **must (--hosted)**; the build asks for it once something reads it |
| `look.accent` | your accent colour | may (left out: the style's own) |
| `links.site` | your public website | **must**, your own |
| `links.support_email` | where your users write for help | **must**, your own |
| `links.contact_email` | where your users write when their box can't send feedback (the CLI's send-feedback pages print it) | **must**, your own |
| `links.repo` | your public source repo's page, https (where the CLI's text names the brand's repo) | **must**, your own |
| `links.support_url`, `terms`, `privacy` | your support page, terms of service and privacy policy | **must (--hosted)**, not `""` |
| `links.dashboard` | where a user manages their account | **must**, your own, or `""` (none) |
| `links.feedback` | where Send feedback goes | **must**, your own, or `""` (the button is off) |
| `mail.system_from` | the address system mail comes from. There is no fleet-wide sender: each box sends from its own server name through its own hostmail, so `{server}` stands for the box's name (`noreply@{server}` is `noreply@box.partner.example` for the box `box.partner.example`). SPF, DKIM and DMARC go on each box's server name, never on your apex domain | may (left out: `noreply@{server}`) |
| `mail.system_from_name` | the name on it | may (left out: the product's) |
| `mail.postal` | `true` or `false`: your boxes may join Postal | **must**, any value |
| `phone.bundle_id`, `team_id`, `push_topic` | the phone app your boxes pair with: K5's own app's values (`brand/k2` in K5's repo), or your own app's | **must**, any value |
| `sew.call_name` | what calls and chat name the service | may (left out: the product's) |

Most of these fields are read by nothing in K5 yet: K5's release notes
say when each one starts to change what a build does. Set them now, so
your brand is whole when they do.

## How a build starts

**In step with K5.** Every K5 release is a tag in K5's public repo
(`v0.47.0`, `v0.47.1`, ...). The reference workflow runs every hour.
`driver.sh pick` lists K5's release tags and picks the newest one that:

- matches `tag_regex` and is at least `first_tag`;
- is newer than your last shipped one (the newest `shipped/<tag>` tag in
  your repo, which the ship job pushes after each release);
- is not in `hold.txt`.

When it finds one, the build job builds it, and the ship job signs and
publishes it. If two K5 releases came out since your last run, you get the
newer one: a feed only moves forward.

**By hand.** Run the workflow with a tag (and `dry_run` off) to build one
tag now. A tag you already shipped, or an older one, is refused.

## How to hold a tag

Add the tag to `hold.txt`, with your reason after a `#`:

    v0.47.3  # waiting for our own QA of the new sign-in screen

The pipeline never picks a held tag. If a newer tag comes out and is not
held, that one is built instead. To ship a held tag, take its line out.

A line with only `all` holds every new tag: a freeze. Nothing new is built
until you take it out.

You can still test a held tag: `driver.sh dry-run <tag>` (or the workflow
with that tag and `dry_run` on) runs every step up to signing.

## The steps

`pipeline/driver.sh` runs them in this order. A step that fails stops the
build with a line that says why. **SECRETS** marks the steps that need
yours.

| # | step | what it does |
|---|---|---|
| 1 | fetch | K5's public repo at the tag, nothing else; with `--sha`, the tag must still be the commit `pick` saw |
| 2 | tools | the tag carries the brand tools the driver calls; an older tag is refused |
| 3 | licence | `scripts/brand/licence-gate.sh`: the tag's `Cargo.lock` holds no crate on `scripts/brand/copyleft-crates.txt`. Today every tag is refused here, for `libgit2-sys` (see "The licence gate") |
| 4 | ceiling-zero | `scripts/brand/ceiling-zero-gate.sh`: every command line is answered by K5's own program. A tag where any command still falls through to the old shell CLI (which prints K5's names) is refused |
| 5 | lint | `scripts/brand.sh lint` on your brand, then `--live` when `lint_live = on` |
| 6 | build | K5's own Linux release build with `K2_BRAND_DIR` set to your brand folder. **SECRETS** for a real build: your mail sign-in clients |
| 7 | judge | the output judges read everything the CLI prints for every help and study page: no other brand's name, and each of your names in its place. K5's own list of lines it still owes a fix ("debt") is not carried into your build: any such line fails it. Then every binary must carry your brand mark, and only yours |
| 8 | keys | a report: does the build trust your update key, and none of K5's? |
| 9 | check | in the ship job: the output is a real build (not a dry run) of your brand, unchanged since step 6; with `publish = on`, step 8 must pass |
| 10 | sign | `hooks/sign.sh` with your signing key. **SECRETS** |
| 11 | publish | `hooks/publish.sh` to your feed, then the tag `shipped/<K5 tag>` in your repo. **SECRETS** |

**Dry run:** `driver.sh dry-run <tag>` runs steps 1 to 8 on any release
tag, held or old, with no secrets at all. Its output says `dry_run = yes`
and is never shipped. Use it to test a new K5 tag, or a change to your
brand file. `driver.sh check <tag>` runs only the gates (steps 1 to 5),
in a few seconds, with no build.

## Signing and your update feed

You sign with **your** identity and publish to **your** feed. Nothing in
the pipeline can sign with K5's keys or publish to K5's feed: it never has
them.

- **Your update key** is a minisign key pair. Its secret half is a CI
  secret, used only in the ship job. Its public half is `update_pubkey` in
  `distributor.conf`; your build must carry it, so your users' servers
  accept your updates and nobody else's.
- **`hooks/sign.sh`** signs each binary and `SHA256SUMS` in the form K5's
  updaters check (the trusted comment names the file and the version, so
  an old build can't be served as a new one). When your build also makes
  desktop apps, their signing goes here too: for macOS your own Developer
  ID certificate and notarization, for Windows your own code signing
  certificate.
- **`hooks/publish.sh`** uploads to your feed's host and writes the
  manifest last, so a client never sees a version before its files.
- The ship job runs in its own GitHub environment, `release`: give it
  required reviewers if a person should approve each release. It runs no
  K5 code, only the driver and your hooks.

## What a K5 tag gives you today

Say so plainly to your own team: brand builds are new, and some parts of
a build don't follow the brand file yet.

- **Built:** the Linux server programs (the daemon, the CLI, the compute
  node runtime, the root helper, the App engine), with your names in what
  the CLI prints and your brand mark in every binary.
- **Not built by this pipeline yet:** the desktop apps and their
  installers. They follow when a K5 release says its packaging reads the
  brand file; the driver then gains a step for them.
- **Your update key and your feed's address are not read from your brand
  yet.** K5's update keys are compiled into the programs. Step 8 checks
  for this on every build, and the ship job refuses to publish a build
  that trusts K5's keys or lacks yours. So keep `publish = off` until a K5
  release says a brand build carries its own update key; until then ship
  signs the build and stops, and you hand it to testers yourself.
- A tag is only built when it links nothing a private white-label build
  can't ship (step 3), every command is native (step 4) and the CLI's
  output names your brand alone (step 7). Early tags fail one of them;
  `driver.sh check` tells you which, quickly.
- **No tag passes step 3 yet.** Every K5 release so far links
  `libgit2-sys`. No partner build can pass until a K5 release replaces
  libgit2 with gitoxide. See "The licence gate".

## The licence gate

Step 3 reads the tag's `Cargo.lock` and refuses your build when it holds
a crate on the tag's `scripts/brand/copyleft-crates.txt`. Each row there
names the crate, why partner builds can't ship it, and what lifts the
refusal. The refusal names the crate, its version and what lifts it, with
the reason on the next line:

    globex: can't build from v0.47.3: it includes libgit2-sys 0.17.0+1.8.1, which partner builds can't ship. This lifts in the first release where gitoxide has replaced libgit2.

The rule today, in plain words:

- **libgit2** is allowed in K5's own builds only, and refused for partner
  builds until gitoxide replaces it. libgit2 is GPL-2.0 with a
  linking exception, and its xdiff part is LGPL-2.1 and statically linked:
  a private build can't meet those terms. So the gate refuses any tag
  whose `Cargo.lock` has `libgit2-sys`.
- **pg_query** (PostgreSQL License) and **SQLite** (public domain) are
  accepted in every build, yours included. The list marks them
  `allowed`; the gate names them in its output and never refuses them.

The gate lifts when libgit2 goes: the first K5 release built on gitoxide
has no `libgit2-sys` in its `Cargo.lock`, and that release passes step 3.
Until then, `driver.sh check` refuses every tag at step 3, and that is
expected. K5's own builds (brand `k2` or `k5`) and K5's CI only get a
report from the gate, never a refusal.

## Secrets and what you provide

| what | used by | where it lives |
|---|---|---|
| `MAIL_GMAIL_CLIENT_ID`, `MAIL_GMAIL_CLIENT_SECRET` | build (step 6, real builds) | CI secrets: your own Google sign-in client for mail; required |
| `MAIL_MICROSOFT_CLIENT_ID` | build (step 6, real builds) | CI secret: your own Microsoft client; optional |
| `UPDATE_MINISIGN_KEY`, `UPDATE_MINISIGN_PASSWORD` | ship (step 10) | CI secrets of the `release` environment: your update key's secret half |
| `FEED_UPLOAD_URL`, `FEED_UPLOAD_TOKEN` | ship (step 11) | CI secrets of the `release` environment: where uploads go, and a token that may write there only |
| `update_pubkey`, `feed_url` | ship (steps 9 to 11) | `distributor.conf`: public values |
| Apple Developer ID certificate, notarization key; Windows code signing certificate | sign, when apps are built | CI secrets, when that step comes |
| Feed hosting | your users' builds | your own host: a web server, a storage bucket or releases in your own repo |

## What you must never change

- **K5's source.** The pipeline builds K5's tag as it is. If you ever need
  code of your own, keep it as a small set of patches on top of each tag,
  rebased every release, and send anything general to K5 as a brand
  setting.
- **The gates.** Never skip step 3, 4, 5 or 7, never pass the judges a
  keep file with debt rows in it, never build a tag whose ceiling isn't
  zero, and never build a tag the licence gate refuses. The licence gate
  keeps code out of your build that a private build can't ship; the
  others make the build say your names and only yours.
- **The split between build and ship.** Signing keys never go to the build
  job: it runs K5's build code.
- **Your brand's id, once shipped.** It is in every binary, and a coming
  K5 release writes it into each server's data folder too. Changing it
  strands your users' servers.
- **Aliases.** An alias may only be one of your own former command names.
  Never `k2`, `k5`, or another brand's command.
- **Names on the wire.** Routes, headers, token prefixes and message
  formats are the same in every brand's build, so servers of different
  brands can talk to each other. Your brand file changes what people read,
  never what servers send.

## More targets

The reference builds `x86_64-unknown-linux-gnu`. For ARM Linux, run the
build job again with `target = aarch64-unknown-linux-gnu` (as K5's own
`daemon-binaries.yml` does, with its cross compilers), one output folder
per target.

## Any other CI

The workflow is a thin wrapper. On another CI, run:

    bash pipeline/driver.sh pick                   # "<tag> <commit>", or nothing
    bash pipeline/driver.sh build <tag> --sha <commit> --out out
    bash pipeline/driver.sh ship --out out         # with the signing secrets only here
    git push origin refs/tags/shipped/<tag>

Run `build` and `ship` in separate jobs, and give only `ship` your signing
and feed secrets. Both need Linux, git, Python 3.11 or newer, and for
`build` rustup with cmake, clang and pkg-config.
