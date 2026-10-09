-- Phase 10: the tags a file carried before Uguisu first wrote any (ADR 0012).
--
-- JSON, NULL until a tag write captures it: the managed fields' values and a
-- description of the embedded cover (mime, size, hash), never the cover's
-- bytes. A record whose file was tagged before this column existed stays
-- NULL for good - its current tags are Uguisu's, not the original ones - and
-- a re-download resets it, because the snapshot describes those bytes only.
ALTER TABLE archive_files ADD COLUMN original_tags TEXT;
