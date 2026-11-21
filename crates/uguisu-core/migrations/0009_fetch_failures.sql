-- How many times in a row fetching the source feed has failed. Used to report a feed that
-- keeps failing once, not on every attempt.
ALTER TABLE podcasts ADD COLUMN fetch_failures INTEGER NOT NULL DEFAULT 0;
