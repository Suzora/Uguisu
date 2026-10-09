-- Phase 11: each podcast's number of episodes, kept by the database, so the
-- library sorts by it from an index. Counting every podcast's episodes for
-- each page took 78 ms at 10 000 podcasts and 500 000 episodes.
--
-- An insert and a delete are the only changes to count: no statement moves
-- an episode to another podcast. When a podcast is deleted, its episodes go
-- by cascade before their AFTER DELETE body runs (0005), so the decrement
-- finds no row to update and changes nothing.
ALTER TABLE podcasts ADD COLUMN episode_count INTEGER NOT NULL DEFAULT 0;

UPDATE podcasts
   SET episode_count = (SELECT count(*) FROM episodes WHERE episodes.podcast_id = podcasts.id);

CREATE INDEX idx_podcasts_episode_count ON podcasts (episode_count DESC, id);

CREATE TRIGGER episodes_count_ai AFTER INSERT ON episodes BEGIN
    UPDATE podcasts SET episode_count = episode_count + 1 WHERE id = new.podcast_id;
END;

CREATE TRIGGER episodes_count_ad AFTER DELETE ON episodes BEGIN
    UPDATE podcasts SET episode_count = episode_count - 1 WHERE id = old.podcast_id;
END;
