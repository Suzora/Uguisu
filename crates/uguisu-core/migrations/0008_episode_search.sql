-- Full-text index over episode titles and show notes (as plain text). The row id is the
-- episode's id. The index is derived data: Uguisu rebuilds it when it is out of step.
CREATE VIRTUAL TABLE episode_search USING fts5(title, notes, tokenize = 'unicode61 remove_diacritics 2');
