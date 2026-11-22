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
| `brand/brand.toml` | your brand: its id, product name and command, each field explained in the file | yes: this is what makes the build yours |
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
| 3 | ceiling-zero | `scripts/brand/ceiling-zero-gate.sh`: every command line is answered by K5's own program. A tag where any command still falls through to the old shell CLI (which prints K5's names) is refused |
| 4 | lint | `scripts/brand.sh lint` on your brand, then `--live` when `lint_live = on` |
| 5 | build | K5's own Linux release build with `K2_BRAND_DIR` set to your brand folder. **SECRETS** for a real build: your mail sign-in clients |
| 6 | judge | the output judges read everything the CLI prints for every help and study page: no other brand's name, and each of your names in its place. K5's own list of lines it still owes a fix ("debt") is not carried into your build: any such line fails it. Then every binary must carry your brand mark, and only yours |
| 7 | keys | a report: does the build trust your update key, and none of K5's? |
| 8 | check | in the ship job: the output is a real build (not a dry run) of your brand, unchanged since step 5; with `publish = on`, step 7 must pass |
| 9 | sign | `hooks/sign.sh` with your signing key. **SECRETS** |
| 10 | publish | `hooks/publish.sh` to your feed, then the tag `shipped/<K5 tag>` in your repo. **SECRETS** |

**Dry run:** `driver.sh dry-run <tag>` runs steps 1 to 7 on any release
tag, held or old, with no secrets at all. Its output says `dry_run = yes`
and is never shipped. Use it to test a new K5 tag, or a change to your
brand file. `driver.sh check <tag>` runs only the gates (steps 1 to 4),
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
  yet.** K5's update keys are compiled into the programs. Step 7 checks
  for this on every build, and the ship job refuses to publish a build
  that trusts K5's keys or lacks yours. So keep `publish = off` until a K5
  release says a brand build carries its own update key; until then ship
  signs the build and stops, and you hand it to testers yourself.
- A tag is only built when every command is native (step 3) and the CLI's
  output names your brand alone (step 6). Early tags fail one or the
  other; `driver.sh check` tells you which, quickly.

## Secrets and what you provide

| what | used by | where it lives |
|---|---|---|
| `MAIL_GMAIL_CLIENT_ID`, `MAIL_GMAIL_CLIENT_SECRET` | build (step 5, real builds) | CI secrets: your own Google sign-in client for mail; required |
| `MAIL_MICROSOFT_CLIENT_ID` | build (step 5, real builds) | CI secret: your own Microsoft client; optional |
| `UPDATE_MINISIGN_KEY`, `UPDATE_MINISIGN_PASSWORD` | ship (step 9) | CI secrets of the `release` environment: your update key's secret half |
| `FEED_UPLOAD_URL`, `FEED_UPLOAD_TOKEN` | ship (step 10) | CI secrets of the `release` environment: where uploads go, and a token that may write there only |
| `update_pubkey`, `feed_url` | ship (steps 8 to 10) | `distributor.conf`: public values |
| Apple Developer ID certificate, notarization key; Windows code signing certificate | sign, when apps are built | CI secrets, when that step comes |
| Feed hosting | your users' builds | your own host: a web server, a storage bucket or releases in your own repo |

## What you must never change

- **K5's source.** The pipeline builds K5's tag as it is. If you ever need
  code of your own, keep it as a small set of patches on top of each tag,
  rebased every release, and send anything general to K5 as a brand
  setting.
- **The gates.** Never skip step 3, 4 or 6, never pass the judges a keep
  file with debt rows in it, never build a tag whose ceiling isn't zero.
  They are what make the build say your names and only yours.
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
