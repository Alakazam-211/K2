# Spike: macOS signing inside one K2 Compute job (0.46.7, TC-26)

prd-tauri-core-0467-v1 TC-26, vs-live TA-24. Script:
`scripts/spike/macos-sign-in-job.sh`. Run 2026-10-29 by Tauri core Lane B.

## The question

Can the owner's own Mac, enrolled as their K2 Compute node, sign, notarize
and updater-sign a Tauri `.app` **unattended**, inside a compute job, with
the keys delivered to that job only (`k2 compute run --secret`)?

The obvious setup can't work: jobs run as `_k2node`, not the owner, and the
Seatbelt wall denies `/Users` and `/Library/Keychains` (TA-24), so there is
no login keychain and no system keychain. The spike tests the alternative:
**a temporary keychain inside the job folder**, the `.p12` and Apple's
intermediate imported into it, `codesign --keychain <that file>` (the
user's keychain search list is never touched), everything removed on exit.

## What ran

`--selftest` (no real key anywhere: a throwaway self-signed identity and a
dummy `.app`) on both Mac minis, as `_k2node` inside the wall:

- k2n2-m1, jobs `344e6a34`, `8f1e45dc`, `f66312db` (iterations)
- k2n3-m1, job `10fdb163` (final script, exit 0)

macOS 26.4.1, Command Line Tools at `/Library/Developer/CommandLineTools`.

## Results against the exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Temporary keychain in the job folder; `.p12` imported; Apple intermediate imported | **Pass.** `security create-keychain` / `import` / `set-key-partition-list` all work as `_k2node` in the wall (`/Users` confirmed walled off). |
| 2 | `codesign --timestamp --options runtime` of a Tauri `.app` | **Not proven; strongly indicated.** A self-signed identity can't be made trusted in a job: `add-trusted-cert` → "The authorization was denied since no user interaction was possible", so codesign says "no identity found". But trust evaluation itself works in the wall: Apple's Developer ID G2 intermediate verifies to Apple's root (`security verify-cert`), and `timestamp.apple.com` answers. A real Developer ID identity chains through that intermediate, so it should be valid here. Needs one real-key run. |
| 3 | `notarytool submit --key … --wait` accepted; `stapler staple` | **Not run** (needs our App Store Connect key). Both tools are present in the Command Line Tools on the minis. |
| 4 | `tauri signer sign` makes a `.sig` TC-8 accepts | **Not run** (needs an updater key). `npx` is present on the minis. TC-8's check is covered by Lane B's daemon tests with the TEST key. |
| 5 | `spctl -a -vv` passes on another Mac | **Not run** (needs 2–3). |
| 6 | After the job: keychain deleted, no key file left, job folder wiped | **Pass.** The exit trap deletes the keychain, `rm -P`s key files, removes the work folder and checks none remain, on success and on failure. |
| 7 | Refuse a node not owned by the App's owner (`signing_node_not_owner`) | **Not built here.** Node ownership is per server (TA-24). Rule for the 0.46.8 recipe: refuse a node that is shared with another server. Both minis are "shared with 1 other server", so the recipe would refuse them for customer signing, which is the intended result. |

## Verdict

**Partial pass: the mechanism works; the one real-key run decides.**
The two unknowns TA-24 raised are answered:

- The keychain can live in the job folder, be unlocked, take the identity and
  its partition list, and be removed completely, all without a GUI session.
- Trust evaluation (trustd and the system roots) works from inside the wall.

What is left is one run with our own Developer ID, ASC key and updater key
on a mini acting as "an owner's Mac" (criteria 2–5). That run needs Rosson's
go, because it uses our real signing identity (the release keys stay on
z3mbpZ otherwise):

```
k2 compute run k2n2-m1 --sha <sha> --needs node@22 \
  --secret <p12 b64>=K2_SIGN_P12_B64 --secret <p12 pw>=K2_SIGN_P12_PASSWORD \
  --secret <identity>=K2_SIGN_IDENTITY \
  --secret <p8 b64>=K2_NOTARY_KEY_P8_B64 --secret <key id>=K2_NOTARY_KEY_ID --secret <issuer>=K2_NOTARY_ISSUER \
  --secret <updater key>=TAURI_SIGNING_PRIVATE_KEY --secret <updater pw>=TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
  --artifact 'out/*' \
  -- bash scripts/spike/macos-sign-in-job.sh --app <path/to/Our.app>
```

Then `spctl -a -vv` on another Mac (criterion 5) and `k2 app updates push`
of `out/*.tar.gz` with its `.sig` (criterion 4 end to end).

- **If it passes:** the `tauri-macos` recipe is built in 0.46.8 on this
  script, with the criterion-7 refusal.
- **If codesign fails with `errSecInternalComponent`:** the interactive way
  (the owner's own terminal session and login keychain) stays the only
  documented path in 0.46.7, and the recipe waits.

## Notes for the recipe (0.46.8)

- Use `openssl rand` for the keychain password: `tr … | head` trips
  `pipefail` (exit 141), which the first run hit.
- Pass `--keychain` to `codesign`; never edit the job user's search list.
- Import Apple's intermediate into the temporary keychain: the wall hides
  `/Library/Keychains`. Pin its sha256 in the recipe (the spike fetched it
  over HTTPS from apple.com without a pin).
- `security set-key-partition-list -S apple-tool:,apple:,codesign:` is what
  keeps codesign from asking for a GUI prompt.
