-- Phase 10: a downloaded episode whose feed now points at different audio
-- (ADR 0015). Set by a refresh that changes the primary enclosure's URL or
-- declared length after the file was archived; the file is kept as it is.
-- NULL means nothing changed since the download. A re-download clears it.
ALTER TABLE archive_files ADD COLUMN source_changed_at TEXT;
