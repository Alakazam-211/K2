-- 0318: who a permission card asks (0.46.17).
--
-- An agent may name its approvers (`k2 permission request … --to a,b`), and
-- the owner may set who agents' cards go to when it names no one
-- (`permissionApprovers` in the settings file). One row per name a request
-- asks: a Connect login, or `owner` (the owner token's person). No rows: the
-- request asks everyone who may answer it, as before 0.46.17.
--
-- It narrows who is asked (the card's Allow and Deny, the push, the Tickets
-- item, "Waiting on you"), never who may answer: the answer still needs the
-- request's floor. Written once, when the request is made; read by
-- `permission::approvers`. Rows go with their request's id.
--
-- Additive only; safe to run again.
CREATE TABLE IF NOT EXISTS permission_request_approvers (
  request_id TEXT NOT NULL,
  approver   TEXT NOT NULL,
  PRIMARY KEY (request_id, approver)
) WITHOUT ROWID;
