-- Workspace default model + optional force-on-resume (prd-workspace-default-model-and-api-model-override-v1).
-- NULL/"" default_model = unset → no workspace splice (today's argv).
-- force_model_on_resume default 0: K2-direct dead resume does not re-apply the model.
ALTER TABLE projects ADD COLUMN default_model TEXT;
ALTER TABLE projects ADD COLUMN force_model_on_resume INTEGER NOT NULL DEFAULT 0;
-- Persist API `model` across spawn-queue enqueue → drain (D8).
ALTER TABLE host_session_spawn_queue ADD COLUMN model TEXT;
