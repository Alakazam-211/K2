# Permission cards: golden samples (K2 0.46.3, `cards=2`, `view.v = 1`)

These files are written by K2's own daemon code, not by hand:
`cargo test -p k2-daemon permission_samples`
(`crates/k2-daemon/src/permission/permission_samples.rs`). The same test fails
if the code's output and these files differ, so the wire can't change without
these files changing too. The phone contract is
`.k2/prds/permission-cards-phone-contract-v1.md`; **where the text and a sample
disagree, the sample is right.**

## File names

`<view>__<state>__<form>.json`, listed with a sha256 each in `index.json`.

| form | what it is | transport |
|---|---|---|
| `cards2` | the Thread doc a person gets with `cards=2` | doc |
| `fallback` | the doc the same viewer gets without `cards=2` (old builds), and what agent passports always get | doc |
| `item` | one `GET /cli/thread?cards=2` item `{collection, seq, id, doc, conversation_id}`; the delta's `items` / `changed[]` and catchup's items have this shape | get |
| `ws` | one `WS /cli/overlay/events?cards=2` frame `{collection, seq, id, doc, rev}` | ws |
| `response` | `POST /cli/thread/answer`: `{status, body}` for each answer and error a phone can meet | http |
| `hidden` | App guests: **no doc at all** on any path | none |
| `legacy` / `sealed-plaintext` | `push__permission__*`: the push to rosson's phone when the card is posted (lane A2): the relay payload (`K2Cloud::payload`) and the sealed lane's plaintext before sealing/padding (`e2e::alert_plaintext`); server stamp `rosson` / `inst_01`, `kid` and `ctr` fixed | push |

| view | who |
|---|---|
| `approver-click` | rosson, Owner login, on the `compute.use` card (Admin floor, click) |
| `approver-admin` | maya, Admin login, same card |
| `owner-token` | the owner token (desktop), same card |
| `asker` | dana, Member, the login the agent was working with (`askedFor`) |
| `member` | lee, Member, not the asker |
| `agent` | the asking agent's passport (fallback only, whatever it sends) |
| `approver-confirm` | rosson on the `apps.approve` card (Owner floor, confirm `app-approve`, held request `ar_22`) |
| `admin-wait-confirm` | maya on that card: below the Owner floor, so `mode:"wait"` |
| `batch-approver` / `batch-admin` | rosson / maya on a batch of three (`apps.build`, `apps.core`, `apps.approve` `ar_23`) |
| `garden-access` | rosson on K2's own `garden.access` card (prd-garden-access-v1 G16–G17): "billing made Sales board and wants to keep editing it", GardenOwner floor (reads `owner`), click, standing with "Turn off again"; `pending`, `approved`, `denied` (= Not now) |
| `garden-access-admin` | maya (Admin) on that card: below the floor, `mode:"wait"` |
| `garden-access-agent` | the asking agent's passport on that card (fallback only) |
| `garden-delete` | rosson on K2's own `garden.delete` card ("billing asks to delete Old board"), one time; `pending`, `approved` (deleted), `denied` (= Keep it) |
| `guest` | an App guest: never receives a permission card |
| `answer` | answer-route responses (`allow`, `deny`, `close`, `below-floor`, `confirm-required`, …) |

States: `pending`, `applying`, `approved`, `already_done` (Settings already
matches: `[Close]`), `already_done-closed` (after Close), `denied`, `expired`,
`stale`, `failed`, `cancelled`, `closed` (the switch was turned off); batch
`pending`, `partial` (click items allowed, the confirm item still waiting),
`done`.

The Garden cards are standard permission cards (`choice.action.id`
`permission`, `view.v` 1, kind `permission`, Allow / Deny, click proof):
the phone draws them with no new UI. K2 presents them itself while
Garden access lists are on (Beta or not); Deny means "Not now" / "Keep it".
Garden ids are K2-minted (`g-` + 8 hex), fixed here as `g-5a1e5b0a`
(Sales board) and `g-0e1db0a7` (Old board); the words always show the
Garden's current name.

## Fixed clock and ids

Asked `1761060000`, expires `1761319200` (72 h), dana's note `1761060300`,
answered `1761061200`. Doc ids are UUIDs in K2 (fixed here:
`7f3a2c90-…` single click card, `9b10ee41-…` confirm card, `b4a1c0de-…`
batch). Request ids are `pr_` + 6–32 hex, or the held request's own id
(`ar_22`). Workspaces `billing` and `shopdev`.

## Not here yet

- **Words:** until lanes E1/E2 register the real catalog actions, the
  `compute.use`, `apps.*` cards use stand-in specs with the PRD's words. When
  the real actions land, the samples take the real spec's words; shapes don't
  change.
