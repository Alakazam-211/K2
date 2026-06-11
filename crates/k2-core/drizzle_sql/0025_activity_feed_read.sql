-- Add read flag to activity_feed for message tracking.
-- Agents mark messages as read after seeing them in checkin.
ALTER TABLE activity_feed ADD COLUMN read INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_feed_unread ON activity_feed(to_agent, read) WHERE read = 0;
