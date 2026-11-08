# Accepting a secret another agent sent: the phone (0.46.10)

For the K2 Companion team.

When one agent sends another a secret (`k2 vault send NAME --to <agent>`, delivery `hd_…`) and it waits for a person, K2 used to post a `handoff` Thread card (`doc.kind = "handoff"`). The phone never drew it, so the owner never saw it.

From 0.46.10 K2 posts a **standard permission card** instead: `choice.action.id = "permission"` with the `cards=2` view (`view.v` 1, `kind: "permission"`), Allow / Deny, click proof. Its catalog action is `vault.incoming.accept`.

## Nothing new is required

The phone already draws this shape for `apps.approve` and `garden.access`, and this card works the same way:

- **Allow** accepts the secret and **Deny** declines it. Answer through `POST /cli/thread/answer` as for any permission card (`answer: "Allow"` / `"Deny"`, `client: "phone"`). It takes no confirm token and no extra fields.
- K2 posts it itself, whatever the "Permission cards (Beta)" switches say. Draw permission cards from the view, never gate them on that switch.
- Only the owner or an Admin can answer it (`approvers.floor: "admin"`). For a Member, `viewer.mode` is `wait`.
- The push is the standard targeted permission push: title "<agent> asks for a permission", body "Accept a secret sent to <agent>". It is name-only and never carries a value.

## Worth checking

- **`consequences` matters on this card.** `whatChanges` says what, from whom and under what name, for example "Accept STRIPE_KEY (api_key) from sales on rosson.k2.dev into ops's vault as STRIPE_KEY_2". The sender's note, a name clash, how long the copy is kept and when the delivery expires are all in `consequences`. If the phone truncates `consequences`, give it a "more" toggle.
- **`values`** are `workspace`, `id` (the `hd_…` delivery), and, when set, `as` (the name it lands under on a clash) or `replace` (`yes`: a new version of one the same sender sent before). Show them like any other card's values.
- **Old `handoff` cards.** A delivery that was already waiting when a server updated keeps its old `handoff` card. Within a minute the daemon also posts the permission card for it, and deciding either one closes both. The phone can keep ignoring `kind: "handoff"`. It has nothing to answer there.
- **Closed by someone else.** If the delivery is decided in Settings or the CLI, expires, or the sender takes it back, the card closes on its own (answered, expired or withdrawn), with the same id and seq. That is the usual card-frame update.
