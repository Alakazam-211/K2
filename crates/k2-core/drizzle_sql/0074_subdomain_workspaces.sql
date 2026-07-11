-- 0074: workspace-attributed nested subdomains — which WORKSPACE a
-- nested K2 Connect subdomain label belongs to. The routing map itself
-- stays control-plane-owned (tunnel/subdomains.rs); this table is the
-- daemon-LOCAL attribution overlay: `label` is the nested subdomain
-- label (lowercase, e.g. "staging"), `project_id` the owning
-- `projects.id`. Written when a label is created/repointed through the
-- K2 surface (`k2 connect subdomain create/point`) or adopted with
-- `k2 connect subdomain claim`; removed on `rm`/`unclaim`. No FK —
-- labels can outlive (or predate) the workspace registry on purpose;
-- readers treat a dangling project_id as unattributed. Additive.
CREATE TABLE IF NOT EXISTS subdomain_workspaces (
    label TEXT PRIMARY KEY,
    project_id TEXT NOT NULL
);
