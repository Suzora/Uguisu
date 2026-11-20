-- Images in show notes are now archived. Forget the cache validators once, so that the next
-- check reads each feed in full and finds the images of episodes that are already archived.
UPDATE podcasts SET etag = NULL, last_modified = NULL;
