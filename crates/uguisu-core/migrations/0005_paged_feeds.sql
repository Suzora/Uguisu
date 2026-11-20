-- Feeds split into pages (RFC 5005) are now read completely. Forget the cache validators once,
-- so that the next check fetches each feed in full and follows its pages even if the first page
-- has not changed.
UPDATE podcasts SET etag = NULL, last_modified = NULL;
