-- Namespace declarations of the source feed's root element, as a JSON list of [prefix, uri]
-- pairs. Needed to publish the original <item> elements, which use these prefixes.
ALTER TABLE podcasts ADD COLUMN namespaces TEXT;

-- Forget the cache validators once, so that the next check fetches each feed in full and
-- records its namespaces even if the feed itself has not changed.
UPDATE podcasts SET etag = NULL, last_modified = NULL;
