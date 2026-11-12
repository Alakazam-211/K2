# Signed approvals: checking one yourself

A **signed approval** is a person's yes to exactly one action, signed on their
own device (Face ID, Touch ID, a passcode or a password) with a key that never
leaves it. Whatever will act on the yes (the **relying party**) writes down what
it will do, keeps those bytes, and checks the signature against keys it already
trusts for that person. K2 only carries the bytes; it can delay an approval but
can't make one.

This page is for relying parties: how the bytes look, what an approval record
is, and how to check one with `k2 approve verify` or with plain `openssl`.

## The payload

```
K2-SIGNED-APPROVAL-1 <zero byte> <JSON text>
```

At most 16384 bytes. You write it once and keep it; nobody re-writes it. The
JSON is one object with exactly these fields:

| Field | Meaning |
|---|---|
| `kind`, `kind_v` | what sort of approval, e.g. `com.example.refund`, version 1 |
| `rp` | you: the name you check against when the approval comes back |
| `approver` | your name for the person; shown on their screen |
| `approver_keys` | the key ids you accept (1 to 16) |
| `nonce` | 32 lowercase hex, yours, used once |
| `issued`, `answer_by` | UTC `2026-11-11T14:00:00Z`; after `answer_by` you refuse |
| `body` | your fields: strings, whole numbers, true/false, lists of strings |

The JSON must be in the strict form or every device refuses it: no key twice,
no control, direction-override or invisible characters, whole numbers only
(write money as `"125.00 USD"`), and nothing nested inside `body`. The full
rules, with one test file per rule, are in
`crates/k2-approve/fixtures/sa1/` (`payloads.json`).

The person's device shows your fields under "K2 has no built-in wording for
this kind. Every field is shown below.", then who will act (`rp`), who is asked
and when it must be answered by. Name your fields so they read well.

## The approval record

```json
{"v":1,"payload_b64":"…","key_id":"…","alg":"ES256","sig_b64":"…"}
```

`payload_b64` is the payload, `sig_b64` the signature (ECDSA P-256 over
SHA-256 of the payload, DER), `key_id` the SHA-256 of the signing key's public
key (DER SubjectPublicKeyInfo), in hex. The record says nothing about how the
person proved it was them: you know that from the key you pinned.

## Keys you trust

A folder: `approvers.tsv` with one line per key,

```
# name	person	check	view
phone-bio	alex	biometric	device
laptop	alex	credential	host
```

and `<name>.pem` (the public key) for each name. **check** is `biometric` (a
face or fingerprint) or `credential` (a passcode, PIN or password, or a check
that may have been either). **view** is `device` (a phone: only the K2 app can
use the key, and its own screen showed your words), `host` (a computer where
other programs of the same user could ask the key to sign while showing other
words) or `page` (a web page). You pin these yourself when you add the key;
compare the six fingerprint words the device shows with
`k2 approve key-id <name>.pem`.

An optional fifth column, **attest**, says how the key was proven when you
pinned it: `production` or `development` (the K2 phone app's App Attest
attestation, checked once at enrolment: Apple's leaf certificate lasts three
days, so you store the result, not the certificate) or `none` (the default:
you compared the words). `--attest production` refuses any key that wasn't
attested by an App Store or TestFlight build (`not_attested`).

## With the k2 CLI

```
k2 approve verify --approval approval.json --keys ./approvers \
    --payload request.bin --rp shop@your-server.k2.dev --kind com.example.refund \
    --nonce <the nonce you issued> --check biometric --view device \
    --used-nonces ./used
```

Exit 0 means approved; exit 3 prints one line with the reason
(`refused (expired): …`). `--payload` makes sure the approval carries exactly
the bytes you stored; `--used-nonces` refuses a second use of the same nonce.
Without `--used-nonces`, single use is yours to enforce.

## With openssl only

For a relying party with no K2 code. Run every step; skip none.

1. Find the record's `key_id` among the keys you trust **for this person**, and
   read that key's check and view from your own records.
2. Check the record's payload is byte for byte the payload you stored for that
   nonce, then verify the signature over **your stored copy**:

   ```
   python3 -c 'import base64,json,sys; r=json.load(open(sys.argv[1])); open("sig.der","wb").write(base64.b64decode(r["sig_b64"])); open("payload.bin","wb").write(base64.b64decode(r["payload_b64"]))' approval.json
   cmp payload.bin stored-request.bin
   openssl dgst -sha256 -verify phone-bio.pem -signature sig.der stored-request.bin
   ```

   `Verified OK` and exit 0, or refuse.
3. Read the JSON after the prefix and the zero byte: `rp` is you, `kind` is the
   one you expect, `key_id` is in `approver_keys`, `approver` is that key's
   person, and the key's check and view meet your policy.
4. The nonce is yours and unused, and your clock is not past `answer_by`. Mark
   the nonce used.
5. Re-check the world (is the action still the right one?), then act.

This works for ES256 keys (phones and Macs). Windows Hello keys are RSA; their
recipe is published once their signature padding is confirmed.

`tests/approve/openssl-recipe.sh` runs this recipe over the shared fixtures.
