-- Keep a new copy when the source replaces an episode's media file (off by default: a podcast
-- that moves to another host changes every address and would be downloaded a second time).
ALTER TABLE podcasts ADD COLUMN keep_versions INTEGER NOT NULL DEFAULT 0;

-- Set when the enclosure address of an archived episode changed and a new copy is wanted.
ALTER TABLE episodes ADD COLUMN refetch INTEGER NOT NULL DEFAULT 0;

CREATE INDEX media_by_verification ON media_files (status, verified_at);
