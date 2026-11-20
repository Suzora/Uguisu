-- Optional start date per podcast: episodes published before it are indexed, but their files
-- are not downloaded. NULL archives everything.
ALTER TABLE podcasts ADD COLUMN archive_from INTEGER;
