-- The file that is archived when the feed offers a better one than its enclosure
-- (podcast:alternateEnclosure, media:content, a further Atom enclosure). NULL: the enclosure.
-- The enclosure columns keep what the feed states; a replaced file is still recognised by them.
ALTER TABLE episodes ADD COLUMN media_url TEXT;
ALTER TABLE episodes ADD COLUMN media_type TEXT;
