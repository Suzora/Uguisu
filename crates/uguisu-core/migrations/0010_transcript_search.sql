-- The full-text index also covers transcripts. The index is derived data: it is dropped here
-- and rebuilt from the episodes and the archived transcript files at the next start.
DROP TABLE episode_search;
CREATE VIRTUAL TABLE episode_search USING fts5(title, notes, transcript, tokenize = 'unicode61 remove_diacritics 2');
