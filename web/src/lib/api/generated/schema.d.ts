export interface paths {
    "/api/v1/archive": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["archive_list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/import": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["import"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/invalid": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["invalid"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/manifests": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["manifest_status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/manifests/write": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["manifests_write_all"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/manifests/{podcast_id}/verify": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["manifest_verify"];
        put?: never;
        post: operations["manifest_verify_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/manifests/{podcast_id}/write": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["manifest_write"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/missing": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["missing"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/orphans": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["orphans"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/policies": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["policies"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/rebuild": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["rebuild"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/reconcile": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["archive_reconcile"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/restore": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["restore"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/stats": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["archive_stats"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/verify": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["verify_all"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["archive_show"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/media": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * A media file lives one HTTP request away from a browser, so the record's
         *     path is checked against the two shapes that are never audio before it is
         *     resolved: nothing under the reserved control directory and no sidecar is
         *     reachable through this route, whatever a row says.
         */
        get: operations["media"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/path-preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["path_preview"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/redownload": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["redownload"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/relocate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["relocate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/sidecar": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["sidecar_show"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/sidecar/write": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["sidecar_write"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/tags": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["tags_show"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/tags/write": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["tags_write"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/archive/{episode_id}/verify": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["verify_one"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/exchange": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["auth_exchange"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/login": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["login"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/logout": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["logout"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/password": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["password"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/session": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["session"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/tokens": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_tokens"];
        put?: never;
        post: operations["create_token"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/tokens/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete: operations["revoke_token"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/tokens/{id}/revoke": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["revoke_token_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/db/backup": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["backup_database"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/db/check": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["check_database"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/db/vacuum": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["vacuum_database"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/discovery/providers": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["providers"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/discovery/records": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_records"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/discovery/records/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["show_record"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/discovery/resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["resolve_get"];
        put?: never;
        post: operations["resolve_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/discovery/search": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["discovery_search"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["download_list"];
        put?: never;
        post: operations["enqueue"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/pause": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["pause_all"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/reconcile": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["download_reconcile"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["resume_all"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/retry-failed": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["retry_failed"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/stats": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["download_stats"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["download_show"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/{id}/cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["download_cancel"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/{id}/pause": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["download_pause"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/{id}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["download_resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/downloads/{id}/retry": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["download_retry"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/episodes/duplicates": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_duplicates"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/episodes/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["show_episode"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/episodes/{id}/resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["resolve_duplicate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["events"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/feeds/inspect": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["inspect"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/feeds/{source_id}/status": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["feed_status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["health"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_podcasts"];
        put?: never;
        post: operations["add_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/opml": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["export_opml"];
        put?: never;
        post: operations["import_opml"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/refresh": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["refresh_all"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["show_podcast"];
        put?: never;
        post?: never;
        delete: operations["remove_podcast"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/archive": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["archive_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/artwork": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["artwork_show"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/artwork/fetch": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["artwork_fetch"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/artwork/image": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The bytes of the artwork a podcast currently uses. Content-addressed, so
         *     the hash is both the file name and a strong validator.
         */
        get: operations["artwork_image"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["enqueue_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/episodes": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_episodes"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/move-feed": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["move_feed"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/pause": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["pause_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/policy": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["policy_show"];
        put: operations["policy_set"];
        post: operations["policy_set_post"];
        delete: operations["policy_clear_delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/policy/clear": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["policy_clear"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/refresh": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["refresh_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/remove": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["remove_podcast_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["resume_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/podcasts/{id}/schedule": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["schedule_podcast"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/scheduler": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["scheduler"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/scheduler/maintenance": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["run_maintenance"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/scheduler/pause": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["scheduler_pause"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/scheduler/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["scheduler_resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/scheduler/run": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Runs one scheduler pass now. The loop keeps its own cadence; this is
         *     for an operator who does not want to wait for it.
         */
        post: operations["run_pass"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/search": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["library_search"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/search/reindex": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["reindex"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_settings"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/{key}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put: operations["set_setting"];
        post?: never;
        delete: operations["clear_setting"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/status": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** @description Body of `POST /api/v1/podcasts`. */
        AddBody: {
            /** @description Feed URL, website or directory page. */
            input: string;
        };
        /** @description What adding a podcast produced. */
        AddOutcome: {
            /** @description `false` when the podcast already existed. */
            created: boolean;
            /** @description The podcast (new or already present). */
            podcast: components["schemas"]["Podcast"];
            report?: null | components["schemas"]["RefreshReport"];
            /** @description How the input was resolved. */
            resolved: components["schemas"]["ResolvedFeed"];
            /** @description Its current source. */
            source: components["schemas"]["PodcastSource"];
        };
        /** @description A pair that looked similar but was not merged. */
        AmbiguityNote: {
            /** @description Title of the other candidate. */
            other_title: string;
            /** @description Short explanation. */
            reason: string;
            /**
             * Format: float
             * @description Title similarity.
             */
            similarity: number;
        };
        /** @description JSON error body: the shape every failure answers with. */
        ApiError: {
            /** @description The error. */
            error: components["schemas"]["ApiErrorBody"];
            /**
             * Format: int32
             * @description Schema version, as on every success body.
             */
            schema: number;
        };
        /** @description Inner error object. */
        ApiErrorBody: {
            /** @description Stable kind. */
            kind: string;
            /** @description Message. */
            message: string;
        };
        /**
         * @description An API token (`auth_tokens`), without the secret it was issued with.
         *
         *     The secret exists once, in the response that creates it. Everything here
         *     can be shown to the operator.
         */
        ApiToken: {
            /**
             * Format: date-time
             * @description When it was issued.
             */
            created_at: string;
            /**
             * Format: date-time
             * @description When it stops working, if it was given a deadline.
             */
            expires_at?: string | null;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /**
             * Format: date-time
             * @description When it was last accepted. Refreshed at most hourly, as for a session.
             */
            last_used_at?: string | null;
            /** @description The operator's label for it. */
            name: string;
            /**
             * Format: date-time
             * @description When it was revoked, if it was.
             */
            revoked_at?: string | null;
            /** @description What it may do. */
            scope: components["schemas"]["Scope"];
        };
        /**
         * @description Classification of an archive failure. Local archive problems get their
         *     own kinds: an HTTP or download kind would say nothing about a file that
         *     is missing from disk.
         * @enum {string}
         */
        ArchiveErrorKind: "archive_not_found" | "archive_missing" | "archive_invalid" | "hash_mismatch" | "size_mismatch" | "path_invalid" | "path_collision" | "template_invalid" | "relocation_failed" | "verification_io" | "policy_conflict" | "policy_invalid" | "sidecar_invalid" | "sidecar_missing" | "manifest_invalid" | "import_ambiguous" | "import_unmatched" | "import_source_invalid" | "artwork_invalid" | "tags_unsupported" | "tags_failed";
        /**
         * @description A file Uguisu owns in the archive: one active record per episode.
         *
         *     Record identity: `episode_id`, `podcast_id`, `registered_at`.
         *     **The bytes on disk now**: `size_bytes`, `hash_algo`, `hash_value`,
         *     `mtime_unix` — these follow the file, so a tag write moves them.
         *     **The bytes as received**: `source_size_bytes`, `source_hash_algo`,
         *     `source_hash_value` — provenance, written once per download and never
         *     again, which is what tells a file Uguisu retagged apart from one that
         *     was altered behind its back. (A *re*-download of the same episode
         *     replaces them, because they then describe different bytes; they are
         *     immutable for the life of one download, not of the row.)
         *     Derived facts: `relative_path` (a relocation moves it), `content_type`,
         *     `sniffed_type`. Verification metadata: `verification_state`,
         *     `verification_reason`, `verified_at`. Metadata state: `origin`,
         *     `tag_state`, `tag_mode`, `tagged_at`, `sidecar_written_at`.
         */
        ArchiveFile: {
            /** @description `Content-Type` as served during the download. */
            content_type?: string | null;
            /**
             * Format: date-time
             * @description Row creation.
             */
            created_at: string;
            /** @description The episode this artifact belongs to (unique: one active file). */
            episode_id: components["schemas"]["Ulid"];
            /** @description Hash algorithm (`sha256`). */
            hash_algo: string;
            /** @description Hash of the complete file **as it is now**. */
            hash_value: string;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /**
             * Format: int64
             * @description Modification time when the record was written (seconds since the
             *     epoch); `None` when the platform did not report one.
             */
            mtime_unix?: number | null;
            /** @description How the record came to exist. */
            origin: components["schemas"]["ArchiveOrigin"];
            original_tags?: null | components["schemas"]["OriginalTags"];
            /** @description Its podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /**
             * Format: date-time
             * @description When the artifact was first recorded.
             */
            registered_at: string;
            /** @description Path relative to the media root, POSIX separators. */
            relative_path: string;
            /**
             * Format: date-time
             * @description When the portable sidecar beside the file was last written.
             */
            sidecar_written_at?: string | null;
            /**
             * Format: int64
             * @description Length in bytes as recorded.
             */
            size_bytes: number;
            /** @description Container guessed from the first bytes. */
            sniffed_type?: string | null;
            /**
             * Format: date-time
             * @description When a refresh last found the feed pointing at different audio for
             *     this download (another URL or declared length). The file is kept as
             *     it is; a re-download clears this (ADR 0015).
             */
            source_changed_at?: string | null;
            /** @description Hash algorithm of [`Self::source_hash_value`]. */
            source_hash_algo?: string | null;
            /**
             * @description Hash of the bytes as received. Equal to `hash_value` until Uguisu
             *     writes tags; a verification pass never touches it.
             */
            source_hash_value?: string | null;
            /**
             * Format: int64
             * @description Length as received; `None` only for rows written before the length was
             *     recorded, which the
             *     migration could not fill.
             */
            source_size_bytes?: number | null;
            tag_mode?: null | components["schemas"]["TagMode"];
            /** @description How far a tag write got. */
            tag_state: components["schemas"]["TagState"];
            /**
             * Format: date-time
             * @description When tags were last written.
             */
            tagged_at?: string | null;
            /**
             * Format: date-time
             * @description Last change.
             */
            updated_at: string;
            /** @description Why, from [`reason`]. */
            verification_reason?: string | null;
            /** @description What the last verification found. */
            verification_state: components["schemas"]["VerificationState"];
            /**
             * Format: date-time
             * @description When the last verification ran.
             */
            verified_at?: string | null;
        };
        /**
         * @description The state of one podcast's manifest file.
         *
         *     The manifest is derived data: `stale` is set inside the very
         *     transaction that changes an artifact, so a crash can only ever leave
         *     "marked stale but actually fresh" — never the reverse.
         */
        ArchiveManifest: {
            /**
             * Format: int64
             * @description How many artifacts the last write listed.
             */
            entries: number;
            /**
             * Format: date-time
             * @description When the manifest was last written.
             */
            generated_at?: string | null;
            /** @description SHA-256 of the manifest text itself, once written. */
            hash_value?: string | null;
            /** @description The podcast whose artifacts it lists. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Where the file lives, relative to the media root. */
            relative_path: string;
            /** @description Whether the index has changed since the last write. */
            stale: boolean;
            /**
             * Format: date-time
             * @description Since when it has been stale.
             */
            stale_since?: string | null;
            /**
             * Format: date-time
             * @description Last change to this row.
             */
            updated_at: string;
        };
        /**
         * @description How an archive record came to exist.
         *
         *     This is provenance, not evidence: it says where the bytes came from,
         *     never whether they are still correct. Only a verification pass decides
         *     that.
         * @enum {string}
         */
        ArchiveOrigin: "download" | "import" | "rebuild";
        /** @description One page of archive records. */
        ArchivePage: {
            /** @description The records. */
            files: components["schemas"]["ArchiveFile"][];
            next_after?: null | components["schemas"]["Ulid"];
        };
        /**
         * @description The automatic download rules for one podcast, as stored.
         *
         *     A row exists only where a user overrode the global defaults; the engine
         *     merges it with [`ArchiveConfig`](crate::config::ArchiveConfig) into the
         *     effective policy.
         */
        ArchivePolicy: {
            /**
             * Format: int32
             * @description Episodes published longer ago than this are left alone; `None`
             *     uses the global default.
             */
            max_age_days?: number | null;
            /**
             * Format: int32
             * @description At most this many of the podcast's episodes may be waiting to be
             *     archived at once (queued, retrying, downloading or finalizing);
             *     `None` uses the global default, `0` means no limit.
             */
            max_backlog?: number | null;
            /** @description Manual or automatic. */
            mode: components["schemas"]["PolicyMode"];
            /** @description The podcast this applies to. */
            podcast_id: components["schemas"]["Ulid"];
            priority?: null | components["schemas"]["Priority"];
            /**
             * Format: date-time
             * @description Last change.
             */
            updated_at: string;
        };
        /** @description What an archive reconciliation repaired and found. */
        ArchiveReconcileReport: {
            /**
             * Format: int64
             * @description Artifacts checked for existence.
             */
            checked: number;
            /**
             * Format: int64
             * @description Artifacts whose file turned out not to be a file.
             */
            invalid: number;
            /**
             * Format: int64
             * @description Artifacts whose file turned out to be gone.
             */
            missing: number;
            /**
             * Format: int64
             * @description Completed downloads that had no archive record and now do.
             */
            registered: number;
            /**
             * Format: int64
             * @description Tag writes that were in flight when the process stopped.
             */
            tagging_settled?: number;
            /**
             * Format: int64
             * @description Completed downloads whose file could not be registered.
             */
            unregisterable: number;
        };
        /**
         * @description Archive state of an episode (denormalized; owned by the download engine on).
         * @enum {string}
         */
        ArchiveState: "expected" | "skipped" | "queued" | "downloading" | "archived" | "missing" | "modified" | "failed";
        /** @description A podcast's artwork, current and superseded. */
        ArtworkBody: {
            current?: null | components["schemas"]["PodcastArtwork"];
            /**
             * @description Images an earlier fetch stored. They are still on disk: Uguisu
             *     replaces artwork, it does not remove it.
             */
            history: components["schemas"]["PodcastArtwork"][];
        };
        /** @description What a fetch did. */
        ArtworkFetchBody: {
            artwork?: null | components["schemas"]["PodcastArtwork"];
            detail?: string | null;
            state: string;
        };
        /**
         * @description An image container Uguisu stores as podcast artwork.
         *
         *     Deliberately short: these three are what podcast feeds serve and what
         *     every tag format can carry. A format that is not on this list is
         *     refused, never guessed at — the point of the list is that the bytes
         *     were recognised, not that the server was believed.
         * @enum {string}
         */
        ArtworkFormat: "jpeg" | "png" | "webp";
        /** @description Body of `POST /api/v1/podcasts/{id}/artwork/fetch`. */
        ArtworkRequest: {
            /** @description Skip the conditional request and fetch the bytes again. */
            force?: boolean | null;
        };
        /**
         * @description How an attempt ended.
         * @enum {string}
         */
        AttemptOutcome: "completed" | "failed" | "retry_scheduled" | "cancelled" | "paused" | "interrupted";
        /** @description Hit/miss counters. */
        CacheStats: {
            /**
             * Format: int64
             * @description Entries currently held (approximate).
             */
            entries: number;
            /**
             * Format: int64
             * @description Cache hits.
             */
            hits: number;
            /**
             * Format: int64
             * @description Cache misses.
             */
            misses: number;
        };
        /** @description What a provider can do. */
        Capabilities: {
            /** @description Lookup by feed URL. */
            lookup_by_feed_url: boolean;
            /** @description Lookup by `podcast:guid`. */
            lookup_by_guid: boolean;
            /** @description Lookup by the provider's own id. */
            lookup_by_id: boolean;
            /** @description Lookup by iTunes id. */
            lookup_by_itunes_id: boolean;
            /** @description Supplies a popularity signal. */
            popularity: boolean;
            /** @description Free-text search. */
            search: boolean;
        };
        /** @description A chapters document reference (`podcast:chapters`). */
        ChaptersRef: {
            /** @description MIME type. */
            mime_type?: string | null;
            /** @description URL. */
            url: string;
        };
        /** @description Circuit breaker state. */
        CircuitState: {
            /** @enum {string} */
            state: "closed";
        } | {
            /**
             * Format: int64
             * @description Seconds until the next probe.
             */
            retry_in_secs: number;
            /** @enum {string} */
            state: "open";
        } | {
            /** @enum {string} */
            state: "half_open";
        };
        /** @description What clearing a policy did. */
        ClearOutcome: {
            /** @description `false` when the podcast had no override to begin with. */
            cleared: boolean;
            podcast_id: components["schemas"]["Ulid"];
            /** @description The vocabulary a skip reason comes from, so a client can render it. */
            reasons: string[];
        };
        Cleared: {
            cleared: boolean;
            key: string;
        };
        /** @description Answer of the global pause/resume commands. */
        ControlBody: {
            control: components["schemas"]["DownloadControl"];
        };
        /**
         * @description Quality of a normalized date (`docs/FEED_ENGINE.md`).
         * @enum {string}
         */
        DateQuality: "exact" | "assumed_utc" | "future" | "ancient" | "invalid";
        /** @description A copy of the database (ADR 0056). */
        DbBackup: {
            /**
             * Format: int64
             * @description Its size.
             */
            bytes: number;
            /** @description The copy, on the machine that wrote it. */
            path: string;
        };
        /** @description What `PRAGMA integrity_check` and `PRAGMA foreign_key_check` found. */
        DbCheck: {
            /** @description Rows naming a parent that is not there, up to a hundred. */
            foreign_keys: string[];
            /** @description `integrity_check`'s findings, up to a hundred. */
            integrity: string[];
            /** @description Neither check found anything. */
            ok: boolean;
        };
        /** @description The database's size before and after a `VACUUM`. */
        DbVacuum: {
            /**
             * Format: int64
             * @description Bytes after.
             */
            bytes_after: number;
            /**
             * Format: int64
             * @description Bytes before.
             */
            bytes_before: number;
        };
        /**
         * @description What a resolution decided, and why (ADR 0030).
         *
         *     **Provenance only.** Nothing reads these rows back as a feed source: a
         *     search result does not become a podcast, and `search`, `resolve` and
         *     `podcast add` stay three separate things. `podcast_id` is filled in
         *     only when the user went on to add one, and survives that podcast's
         *     deletion as a `NULL` rather than taking the record with it.
         */
        DiscoveryRecord: {
            /**
             * Format: date-time
             * @description When the row was written.
             */
            created_at: string;
            /** @description One line about why, when there is anything to say. */
            detail?: string | null;
            /** @description The feed that was decided on. */
            feed_url?: string | null;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description What the user typed. */
            input: string;
            podcast_id?: null | components["schemas"]["Ulid"];
            /** @description Which provider answered, when one did. */
            provider?: string | null;
            /** @description The provider's own reference, when there was one. */
            provider_ref?: string | null;
            /**
             * Format: date-time
             * @description When the resolution ran.
             */
            resolved_at: string;
            /** @description How it ended. */
            status: components["schemas"]["ResolutionOutcome"];
            /** @description The steps taken, as the resolver reported them. */
            steps: string[];
            /** @description Non-fatal notes. */
            warnings: string[];
            /** @description The podcast's website, when the resolution found one. */
            website?: string | null;
        };
        /**
         * @description How a search ended. Distinguishes "nothing matched" from "nothing could
         *     be asked" so that the UI never collapses failures into "not found".
         * @enum {string}
         */
        DiscoverySearchOutcome: "results" | "no_results" | "all_providers_failed" | "no_providers_enabled";
        /** @description One attempt of a job (`download_attempts`, append-only). */
        DownloadAttempt: {
            /**
             * Format: int32
             * @description 1-based attempt number.
             */
            attempt_no: number;
            /**
             * Format: int64
             * @description Average rate.
             */
            avg_rate_bps?: number | null;
            /**
             * Format: int64
             * @description Bytes received in this attempt.
             */
            bytes_received: number;
            /**
             * Format: int64
             * @description Wall-clock duration.
             */
            duration_ms: number;
            /** @description Failure detail. */
            error_detail?: string | null;
            error_kind?: null | components["schemas"]["DownloadErrorKind"];
            /**
             * Format: date-time
             * @description End, when the attempt ended.
             */
            finished_at?: string | null;
            /**
             * Format: int32
             * @description HTTP status of the final hop.
             */
            http_status?: number | null;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description The job. */
            job_id: components["schemas"]["Ulid"];
            /**
             * Format: date-time
             * @description Scheduled retry, when one was scheduled.
             */
            next_attempt_at?: string | null;
            outcome?: null | components["schemas"]["AttemptOutcome"];
            /**
             * Format: int64
             * @description Offset the attempt started at (0 = fresh).
             */
            range_start: number;
            /**
             * Format: uri
             * @description URL used.
             */
            source_url: string;
            /**
             * Format: date-time
             * @description Start.
             */
            started_at: string;
        };
        /** @description Global queue control (`download_control`, one row). */
        DownloadControl: {
            /** @description Whether no job is claimed. */
            paused: boolean;
            /**
             * Format: date-time
             * @description Since when.
             */
            paused_at?: string | null;
            paused_reason?: null | components["schemas"]["PauseAllReason"];
            /**
             * Format: date-time
             * @description Last change.
             */
            updated_at: string;
        };
        /**
         * @description Classification of a failed download attempt (`docs/DOWNLOAD_ENGINE.md`
         *     "Retry taxonomy"). Whether a kind is retried is decided by
         *     [`DownloadErrorKind::is_retryable`]; the worker may still refuse a retry
         *     when the attempt budget is spent.
         * @enum {string}
         */
        DownloadErrorKind: "network" | "timeout" | "dns" | "tls" | "http" | "rate_limited" | "not_found" | "unauthorized" | "forbidden" | "range_unsupported" | "range_invalid" | "content_length_mismatch" | "disk_full" | "permission_denied" | "io" | "validation" | "cancelled" | "storage" | "policy_blocked";
        /** @description A download job (`download_jobs`); exactly one per episode. */
        DownloadJob: {
            /** @description Whether the server supports byte ranges (`None` = unknown). */
            accept_ranges?: boolean | null;
            /**
             * Format: int32
             * @description Attempts started so far.
             */
            attempt_count: number;
            /**
             * Format: int64
             * @description Acknowledged bytes in the `.part` file.
             */
            bytes_downloaded: number;
            /**
             * Format: date-time
             * @description When a worker last claimed the job.
             */
            claimed_at?: string | null;
            /** @description `Content-Type` as served (recorded, never enforced). */
            content_type?: string | null;
            /**
             * Format: date-time
             * @description Creation time (queue order within a priority).
             */
            created_at: string;
            enclosure_id?: null | components["schemas"]["Ulid"];
            /** @description The episode being downloaded (unique). */
            episode_id: components["schemas"]["Ulid"];
            /** @description `ETag` of the resource being resumed. */
            etag?: string | null;
            /**
             * Format: date-time
             * @description Completion, failure or cancellation time.
             */
            finished_at?: string | null;
            /** @description Hash algorithm (`sha256`). */
            hash_algo: string;
            /** @description Hash of the complete file, set when finalization starts. */
            hash_value?: string | null;
            /** @description `scheme://host:port` for per-host limits. */
            host_key: string;
            /** @description Identifier; also the `.part` file name. */
            id: components["schemas"]["Ulid"];
            /** @description Detail of the last failure (no headers, no secrets). */
            last_error_detail?: string | null;
            last_error_kind?: null | components["schemas"]["DownloadErrorKind"];
            /**
             * Format: int32
             * @description Last HTTP status seen.
             */
            last_http_status?: number | null;
            /** @description `Last-Modified` of the resource being resumed. */
            last_modified?: string | null;
            /**
             * Format: int32
             * @description Attempt budget.
             */
            max_attempts: number;
            /**
             * Format: date-time
             * @description When a `retrying` job becomes eligible.
             */
            next_attempt_at?: string | null;
            /** @description `.part` path relative to the media directory (POSIX separators). */
            part_path: string;
            /** @description Its podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Queue priority. */
            priority: components["schemas"]["Priority"];
            /**
             * Format: date-time
             * @description When progress was last persisted.
             */
            progress_at?: string | null;
            /** @description Container guessed from the first bytes (recorded, never enforced). */
            sniffed_type?: string | null;
            /**
             * Format: uri
             * @description The URL being fetched.
             */
            source_url: string;
            /**
             * Format: date-time
             * @description First claim.
             */
            started_at?: string | null;
            /** @description State. */
            state: components["schemas"]["DownloadState"];
            /** @description Why the job is in that state (stable vocabulary, `docs/DOWNLOAD_ENGINE.md`). */
            state_reason?: string | null;
            /** @description Final path relative to the media directory (POSIX separators). */
            target_path: string;
            /**
             * Format: int64
             * @description Complete length when known.
             */
            total_bytes?: number | null;
            /**
             * Format: date-time
             * @description Last change.
             */
            updated_at: string;
        };
        /**
         * @description Persisted state of a download job (ADR 0018).
         * @enum {string}
         */
        DownloadState: "queued" | "downloading" | "finalizing" | "retrying" | "paused" | "completed" | "failed" | "cancelled";
        /** @description Queue statistics. */
        DownloadStats: {
            /** @description Jobs per state. */
            by_state: {
                [key: string]: number;
            };
            /**
             * Format: date-time
             * @description Earliest scheduled retry.
             */
            next_retry_at?: string | null;
            /**
             * Format: int32
             * @description Orphan `.part` files found by the last reconciliation.
             */
            orphan_parts: number;
            paused_all?: null | components["schemas"]["PauseAllReason"];
            /**
             * Format: int32
             * @description Jobs held by workers in this process.
             */
            running: number;
            /** @description Whether the scheduler runs in this process. */
            workers_started: boolean;
        };
        /** @description One page of candidate duplicates, newest first. */
        DuplicatePage: {
            /** @description The candidates. */
            duplicates: components["schemas"]["DuplicatePair"][];
            next_after?: null | components["schemas"]["Ulid"];
        };
        /** @description A candidate duplicate with the episode it probably duplicates. */
        DuplicatePair: {
            /** @description The candidate, stored as skipped and never downloaded. */
            candidate: components["schemas"]["Episode"];
            original?: null | components["schemas"]["Episode"];
        };
        /**
         * @description How a person resolved a candidate duplicate (ADR 0051).
         * @enum {string}
         */
        DuplicateResolution: "same" | "separate";
        /** @description What a resolution did. */
        DuplicateResolved: {
            /** @description The candidate; after `same` it no longer exists. */
            candidate: components["schemas"]["Ulid"];
            /**
             * @description The episode that remains: the original after `same`, the candidate
             *     after `separate`.
             */
            episode: components["schemas"]["Episode"];
            /** @description The episode it was a candidate duplicate of. */
            original: components["schemas"]["Ulid"];
            /** @description Whether the archive policy queued the separated episode. */
            queued: boolean;
            /** @description How the candidate was resolved. */
            resolution: components["schemas"]["DuplicateResolution"];
        };
        /** @description The resolved policy, flattened for the wire. */
        EffectiveBody: {
            /** Format: int32 */
            max_age_days: number;
            /** Format: int32 */
            max_backlog: number;
            mode: components["schemas"]["PolicyMode"];
            priority: components["schemas"]["Priority"];
        };
        /** @description A media file offered by an episode (primary enclosure or an alternate). */
        Enclosure: {
            /**
             * Format: int64
             * @description `podcast:alternateEnclosure@bitrate`.
             */
            bitrate?: number | null;
            /** @description `podcast:alternateEnclosure@codecs`. */
            codecs?: string | null;
            /** @description Owning episode. */
            episode_id: components["schemas"]["Ulid"];
            /**
             * Format: int32
             * @description `podcast:alternateEnclosure@height`.
             */
            height?: number | null;
            /** @description Identifier (stable across refreshes for the same URL). */
            id: components["schemas"]["Ulid"];
            /** @description `podcast:integrity@type`. */
            integrity_type?: string | null;
            /** @description `podcast:integrity@value`. */
            integrity_value?: string | null;
            /** @description Whether this is the item's primary enclosure. */
            is_primary: boolean;
            /** @description Media kind derived from the MIME type. */
            kind: components["schemas"]["EnclosureKind"];
            /** @description `podcast:alternateEnclosure@lang`. */
            lang?: string | null;
            /**
             * Format: int64
             * @description Declared length in bytes.
             */
            length_bytes?: number | null;
            /** @description Declared MIME type, lower-cased. */
            mime_type?: string | null;
            /**
             * Format: int32
             * @description Position within the item (0 = first).
             */
            position: number;
            /** @description Additional `podcast:source` URIs. */
            sources: string[];
            /** @description `podcast:alternateEnclosure@title`. */
            title?: string | null;
            /**
             * Format: uri
             * @description Media URL as published.
             */
            url: string;
        };
        /**
         * @description Media kind of an enclosure, from its declared MIME type.
         * @enum {string}
         */
        EnclosureKind: "audio" | "video" | "other";
        /** @description Body of `POST /api/v1/downloads`. */
        EnqueueBody: {
            /** @description The episode to download. */
            episode_id: string;
            /** @description Queue priority (default `normal`). */
            priority?: components["schemas"]["Priority"];
        };
        /** @description What an enqueue did. */
        EnqueueOutcome: {
            /** @description A new job was queued. */
            job: components["schemas"]["DownloadJob"];
            /** @enum {string} */
            outcome: "created";
        } | {
            /** @description The episode already has a pending or paused job. */
            job: components["schemas"]["DownloadJob"];
            /** @enum {string} */
            outcome: "existing";
        } | {
            /**
             * @description A failed or cancelled job, or a completed one by a redownload, was
             *     queued again with a fresh budget.
             */
            job: components["schemas"]["DownloadJob"];
            /** @enum {string} */
            outcome: "requeued";
        } | {
            /** @description The episode is already downloaded. */
            job: components["schemas"]["DownloadJob"];
            /** @enum {string} */
            outcome: "already_completed";
        };
        /** @description Body of `POST /api/v1/podcasts/{id}/downloads`. */
        EnqueuePodcastBody: {
            /** @description Queue priority (default `normal`). */
            priority?: components["schemas"]["Priority"];
        };
        /** @description Result of a bulk enqueue. */
        EnqueueSummary: {
            /**
             * Format: int32
             * @description Episodes already downloaded.
             */
            completed: number;
            /**
             * Format: int32
             * @description New jobs.
             */
            created: number;
            /**
             * Format: int32
             * @description Episodes that already had a pending or paused job.
             */
            existing: number;
            /**
             * Format: int32
             * @description Failed/cancelled jobs queued again.
             */
            requeued: number;
            /** @description Episodes left out, with reasons. */
            skipped: components["schemas"]["SkippedEpisode"][];
        };
        /** @description Every enveloped success body carries the API's schema version. */
        Envelope: {
            /** @enum {integer} */
            schema: 1;
        };
        /** @description An episode as Uguisu understands the feed item. */
        Episode: {
            /** @description Archive state, owned by the download and archive engines. */
            archive_state: components["schemas"]["ArchiveState"];
            /**
             * Format: uri
             * @description Episode artwork.
             */
            artwork_url?: string | null;
            /** @description Item author. */
            author?: string | null;
            /** @description Hash of the comparable fields (change detection). */
            content_hash: string;
            /**
             * Format: date-time
             * @description Creation time.
             */
            created_at: string;
            /** @description Description with markup. */
            description_html?: string | null;
            /** @description Description as text. */
            description_text?: string | null;
            duplicate_of_episode_id?: null | components["schemas"]["Ulid"];
            /** @description Reasons for the duplicate candidacy. */
            duplicate_reasons: string[];
            /** @description Duration as written. */
            duration_raw?: string | null;
            /**
             * Format: int32
             * @description Duration in seconds.
             */
            duration_secs?: number | null;
            /** @description Media offered by the item. */
            enclosures: components["schemas"]["Enclosure"][];
            /**
             * Format: int32
             * @description Episode number.
             */
            episode_number?: number | null;
            /** @description `full` / `trailer` / `bonus`. */
            episode_type?: string | null;
            /** @description Explicit flag. */
            explicit?: boolean | null;
            /** @description Podcasting 2.0 data. */
            extras: components["schemas"]["EpisodeExtras"];
            /**
             * Format: date-time
             * @description First time the item was seen.
             */
            first_seen_at: string;
            /** @description GUID as published (may be empty or duplicated in bad feeds). */
            guid?: string | null;
            /** @description `guid@isPermaLink`. */
            guid_is_permalink?: boolean | null;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description Stable identity (ADR 0006). */
            identity: components["schemas"]["EpisodeIdentity"];
            /**
             * Format: date-time
             * @description Last time the item was seen in the feed.
             */
            last_seen_in_feed_at: string;
            /**
             * Format: uri
             * @description Episode web page.
             */
            link?: string | null;
            /** @description The item could not be fully parsed. */
            malformed: boolean;
            /** @description Why it is malformed. */
            malformed_reason?: string | null;
            /**
             * Format: int32
             * @description Consecutive complete fetches that did not contain the item.
             */
            missing_streak: number;
            /** @description Owning podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /**
             * Format: date-time
             * @description Normalized publication time (UTC).
             */
            published_at?: string | null;
            /** @description How trustworthy `published_at` is. */
            published_at_quality: components["schemas"]["DateQuality"];
            /** @description Publication time as written in the feed. */
            published_at_raw?: string | null;
            /**
             * Format: date-time
             * @description When removal was detected (never deletes archive state).
             */
            removed_from_feed_at?: string | null;
            /**
             * Format: int32
             * @description Season number.
             */
            season?: number | null;
            /** @description Why the episode was skipped (policy rule or `duplicate of <id>`). */
            skip_reason?: string | null;
            /**
             * Format: date-time
             * @description Sort key: `published_at`, else `first_seen_at`.
             */
            sort_at: string;
            /** @description Normalized title for ordering. */
            sort_title: string;
            /** @description Raw parsed item as Uguisu understood it (source vs normalized). */
            source_metadata?: unknown;
            /** @description `itunes:subtitle`. */
            subtitle?: string | null;
            /** @description Title. */
            title: string;
            /**
             * Format: date-time
             * @description Last modification time.
             */
            updated_at: string;
            /**
             * Format: date-time
             * @description Source-declared update time.
             */
            updated_at_source?: string | null;
        };
        /** @description Change counts per episode class. */
        EpisodeCounts: {
            /**
             * Format: int32
             * @description New episodes.
             */
            added: number;
            /**
             * Format: int32
             * @description Episodes stored as candidate duplicates.
             */
            ambiguous: number;
            /**
             * Format: int32
             * @description Items that could not be fully parsed.
             */
            malformed: number;
            /**
             * Format: int32
             * @description Episodes newly detected as removed.
             */
            removed_detected: number;
            /**
             * Format: int32
             * @description Items in the feed.
             */
            seen: number;
            /**
             * Format: int32
             * @description Unchanged episodes.
             */
            unchanged: number;
            /**
             * Format: int32
             * @description Changed episodes.
             */
            updated: number;
        };
        /**
         * @description One episode with everything a reader needs about it.
         *
         *     Composed rather than joined: four reads by primary key, against three
         *     `Option`s a caller would otherwise fetch separately and interleave wrongly.
         */
        EpisodeDetail: {
            archive?: null | components["schemas"]["ArchiveFile"];
            /** @description The episode, with its enclosures and extras. */
            episode: components["schemas"]["Episode"];
            job?: null | components["schemas"]["DownloadJob"];
            /** @description Its podcast's title, so a page can be headed without a second request. */
            podcast_title: string;
        };
        /** @description Podcasting 2.0 data of an episode, stored as one JSON document. */
        EpisodeExtras: {
            /** @description Chapter documents. */
            chapters?: components["schemas"]["ChaptersRef"][];
            /** @description Funding links. */
            funding?: components["schemas"]["Funding"][];
            license?: null | components["schemas"]["License"];
            location?: null | components["schemas"]["Location"];
            /** @description People. */
            persons?: components["schemas"]["Person"][];
            /** @description Unmodelled elements. */
            raw_extensions?: components["schemas"]["RawExtension"][];
            /** @description Soundbites. */
            soundbites?: components["schemas"]["Soundbite"][];
            /** @description Transcripts. */
            transcripts?: components["schemas"]["TranscriptRef"][];
            /** @description `podcast:txt` records. */
            txt?: components["schemas"]["Txt"][];
            /** @description `podcast:value` block, kept as JSON. */
            value?: unknown;
        };
        /** @description One episode a search found. */
        EpisodeHit: {
            /** @description Whether the episode is archived, queued, missing, … */
            archive_state: components["schemas"]["ArchiveState"];
            /**
             * Format: int32
             * @description Duration in seconds.
             */
            duration_secs?: number | null;
            /** @description The episode. */
            episode_id: components["schemas"]["Ulid"];
            /** @description Its podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description The podcast's title, so a result list needs no second query. */
            podcast_title: string;
            /**
             * Format: date-time
             * @description Publication time, when the feed gave a usable one.
             */
            published_at?: string | null;
            /**
             * Format: double
             * @description The relevance FTS5 computed (lower is better; negated on the way
             *     out so that higher is better).
             */
            relevance: number;
            /** @description Where the match is, with the matched terms marked. */
            snippet: string;
            /** @description The episode's title. */
            title: string;
        };
        /** @description The computed identity of a feed item and the signals behind it. */
        EpisodeIdentity: {
            /** @description Normalized primary enclosure URL key when present. */
            enclosure_key?: string | null;
            /** @description Fingerprint key, always computed when a title exists. */
            fingerprint_key?: string | null;
            /** @description Normalized GUID when the feed provided one (unique or not). */
            guid_key?: string | null;
            /** @description Stable key, e.g. `guid:<normalized>`, `url:<normalized>`, `fp:<hex>`. */
            key: string;
            /** @description Human-readable explanation of the choice. */
            reason: string;
            /** @description Which cascade level produced the key. */
            source: components["schemas"]["IdentitySource"];
        };
        /** @description One page of episodes, newest first. */
        EpisodePage: {
            /** @description The episodes. */
            episodes: components["schemas"]["Episode"][];
            next_after?: null | components["schemas"]["Ulid"];
        };
        /** @description An event envelope. */
        Event: components["schemas"]["EventKind"] & {
            episode_id?: null | components["schemas"]["Ulid"];
            /** @description Identifier (time-ordered). */
            id: components["schemas"]["Ulid"];
            /**
             * Format: date-time
             * @description When it happened.
             */
            occurred_at: string;
            podcast_id?: null | components["schemas"]["Ulid"];
            /**
             * Format: int32
             * @description Wire schema version.
             */
            schema: number;
        };
        /**
         * @description Event kinds with their payloads. Serialized with a `kind` tag holding the
         *     dotted event name.
         */
        EventKind: {
            /**
             * Format: uri
             * @description Feed URL of the current source.
             */
            feed_url: string;
            /** @enum {string} */
            kind: "podcast.added";
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
            /** @description Title. */
            title: string;
        } | {
            /**
             * Format: int64
             * @description Episodes the library held for it.
             */
            episodes: number;
            /**
             * Format: uri
             * @description Feed URL of the source that was current.
             */
            feed_url?: string | null;
            /**
             * Format: int64
             * @description Archived files it had, all of them still on disk.
             */
            files: number;
            /** @enum {string} */
            kind: "podcast.removed";
            /** @description Title. */
            title: string;
        } | {
            /** @description Whether validators were sent. */
            conditional: boolean;
            /**
             * Format: uri
             * @description Feed URL.
             */
            feed_url: string;
            /** @enum {string} */
            kind: "podcast.feed.refresh.started";
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
        } | {
            /** @description Counts. */
            episodes: components["schemas"]["EpisodeCounts"];
            /** @description Fetch log id. */
            fetch_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "podcast.feed.refresh.completed";
            /** @description Whether podcast metadata changed. */
            podcast_changed: boolean;
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
            /** @description Whether the document was truncated. */
            truncated: boolean;
        } | {
            /**
             * Format: int32
             * @description Failures in a row.
             */
            consecutive_failures: number;
            /** @description Detail. */
            detail: string;
            /** @description Classification. */
            error_kind: components["schemas"]["FetchErrorKind"];
            /** @description Fetch log id. */
            fetch_id: components["schemas"]["Ulid"];
            /**
             * Format: int32
             * @description HTTP status when one was received.
             */
            http_status?: number | null;
            /** @enum {string} */
            kind: "podcast.feed.refresh.failed";
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
        } | {
            /** @description Fetch log id. */
            fetch_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "podcast.feed.not_modified";
            /** @description How that was determined. */
            reason: components["schemas"]["NotModifiedReason"];
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
        } | {
            /** @description Changed field names. */
            fields: string[];
            /** @enum {string} */
            kind: "podcast.metadata.updated";
        } | {
            /**
             * Format: uri
             * @description Primary enclosure URL, when present.
             */
            enclosure_url?: string | null;
            /** @description Identity key. */
            identity_key: string;
            /** @enum {string} */
            kind: "episode.discovered";
            /**
             * Format: date-time
             * @description Publication time.
             */
            published_at?: string | null;
            /** @description Title. */
            title: string;
        } | {
            /** @description Changed field names. */
            fields: string[];
            /** @enum {string} */
            kind: "episode.updated";
        } | {
            /** @enum {string} */
            kind: "episode.removal_detected";
            /**
             * Format: int32
             * @description Consecutive fetches without the item.
             */
            missing_streak: number;
        } | {
            /** @description The episode it probably duplicates. */
            duplicate_of: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "episode.identity_ambiguous";
            /** @description Match reasons. */
            reasons: string[];
        } | {
            /** @description The candidate. */
            candidate: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "episode.duplicate_resolved";
            /** @description The episode it was a candidate duplicate of. */
            original: components["schemas"]["Ulid"];
            /** @description How it was resolved. */
            resolution: components["schemas"]["DuplicateResolution"];
        } | {
            /**
             * Format: uri
             * @description Announced URL.
             */
            announced: string;
            /** @enum {string} */
            kind: "feed.url.change_detected";
            /** @description Why it was not adopted. */
            reason: string;
            /** @description Source id. */
            source_id: components["schemas"]["Ulid"];
            /** @description How it was announced. */
            via: components["schemas"]["ReplacementReason"];
        } | {
            /**
             * Format: uri
             * @description Previous URL.
             */
            from: string;
            /** @description Previous source. */
            from_source_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "feed.url.changed";
            /**
             * Format: uri
             * @description New URL.
             */
            to: string;
            /** @description New source. */
            to_source_id: components["schemas"]["Ulid"];
            /** @description How it was announced. */
            via: components["schemas"]["ReplacementReason"];
        } | {
            /**
             * Format: uri
             * @description URL to fetch.
             */
            enclosure_url: string;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.queued";
            /** @description Priority. */
            priority: components["schemas"]["Priority"];
            /**
             * @description `true` when an existing job was queued again: a failed or cancelled
             *     one, or a completed one by a redownload.
             */
            requeued: boolean;
        } | {
            /**
             * Format: int32
             * @description 1-based attempt number.
             */
            attempt: number;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.started";
            /**
             * Format: int64
             * @description Bytes already on disk that the attempt resumes from.
             */
            resumed_from: number;
            /**
             * Format: int64
             * @description Complete length when known before the request.
             */
            total_bytes?: number | null;
            /**
             * Format: uri
             * @description URL being fetched.
             */
            url: string;
        } | {
            /**
             * Format: int64
             * @description Bytes on disk.
             */
            bytes_downloaded: number;
            /**
             * Format: int64
             * @description Remaining seconds at that rate, when the total is known.
             */
            eta_secs?: number | null;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.progress";
            /**
             * Format: float
             * @description Completion in percent when the total is known.
             */
            percentage?: number | null;
            /**
             * Format: int64
             * @description Smoothed rate.
             */
            speed_bps: number;
            /**
             * Format: int64
             * @description Complete length when known.
             */
            total_bytes?: number | null;
        } | {
            /**
             * Format: int64
             * @description Bytes kept.
             */
            bytes_downloaded: number;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.paused";
            /** @description Why (`user`, `paused_all`, `disk_full`). */
            reason: string;
        } | {
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.resumed";
        } | {
            /**
             * Format: int32
             * @description The attempt that failed.
             */
            attempt: number;
            /** @description Detail. */
            detail: string;
            /** @description Classification. */
            error_kind: components["schemas"]["DownloadErrorKind"];
            /**
             * Format: int32
             * @description HTTP status when one was received.
             */
            http_status?: number | null;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.retry_scheduled";
            /**
             * Format: date-time
             * @description When the next attempt may start.
             */
            next_attempt_at: string;
        } | {
            /**
             * Format: int32
             * @description Attempts it took.
             */
            attempts: number;
            /** @description `Content-Type` as served. */
            content_type?: string | null;
            /**
             * Format: int64
             * @description Wall-clock time of the last attempt.
             */
            duration_ms: number;
            /** @description Hash algorithm. */
            hash_algo: string;
            /** @description Hash of the file. */
            hash_value: string;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.completed";
            /** @description Final path relative to the media directory (POSIX separators). */
            path: string;
            /**
             * Format: int64
             * @description File size.
             */
            size_bytes: number;
            /** @description Container guessed from the first bytes. */
            sniffed_type?: string | null;
        } | {
            /**
             * Format: int32
             * @description Attempts made.
             */
            attempts: number;
            /** @description Detail. */
            detail: string;
            /** @description Classification of the last error. */
            error_kind: components["schemas"]["DownloadErrorKind"];
            /**
             * Format: int32
             * @description HTTP status when one was received.
             */
            http_status?: number | null;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.failed";
            /** @description Why (`max_attempts`, `not_retryable`, `validation`, `target_exists`, …). */
            reason: string;
        } | {
            /**
             * Format: int64
             * @description Bytes kept in the `.part`.
             */
            bytes_downloaded: number;
            /** @description Job id. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "download.cancelled";
        } | {
            /** @enum {string} */
            kind: "download.paused_all";
            /** @description Why. */
            reason: components["schemas"]["PauseAllReason"];
        } | {
            /** @enum {string} */
            kind: "download.resumed_all";
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @description Hash algorithm. */
            hash_algo: string;
            /** @description Hash of the file. */
            hash_value: string;
            /** @enum {string} */
            kind: "archive.registered";
            /** @description Path relative to the media root. */
            path: string;
            /**
             * Format: int64
             * @description File size.
             */
            size_bytes: number;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @description How hard the check looked. */
            depth: components["schemas"]["VerifyDepth"];
            /** @enum {string} */
            kind: "archive.verified";
            /** @description Path relative to the media root. */
            path: string;
            /** @description Why it passed (`size_match`, `hash_match`). */
            reason: string;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "archive.missing";
            /** @description Path that was checked. */
            path: string;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @description How hard the check looked. */
            depth: components["schemas"]["VerifyDepth"];
            /** @enum {string} */
            kind: "archive.invalid";
            /** @description Path that was checked. */
            path: string;
            /** @description Why it failed (`size_mismatch`, `hash_mismatch`, `not_a_file`, …). */
            reason: string;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @description Where it was. */
            from: string;
            /** @enum {string} */
            kind: "archive.relocated";
            /** @description Where it is now. */
            to: string;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "archive.source_changed";
            /**
             * Format: int64
             * @description Its declared length now.
             */
            new_length?: number | null;
            /** @description Its URL now. */
            new_url: string;
            /**
             * Format: int64
             * @description Its declared length before, when the feed gave one.
             */
            old_length?: number | null;
            /** @description The primary enclosure's URL before the refresh. */
            old_url: string;
            /** @description The archived file. */
            path: string;
        } | {
            /** @description The job the policy created or re-used. */
            job_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "archive.policy_queued";
            /**
             * Format: int32
             * @description Policy vocabulary version, so a consumer can tell the rules apart.
             */
            policy_version: number;
            /** @description Priority the job was queued at. */
            priority: components["schemas"]["Priority"];
        } | {
            /** @enum {string} */
            kind: "archive.policy_skipped";
            /**
             * Format: int32
             * @description Policy vocabulary version.
             */
            policy_version: number;
            /** @description Why, from `archive::policy_reason`. */
            reason: string;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "archive.sidecar.written";
            /** @description Sidecar path relative to the media root. */
            path: string;
        } | {
            /** @enum {string} */
            kind: "archive.sidecar.invalid";
            /** @description Sidecar path relative to the media root. */
            path: string;
            /** @description Why (`malformed`, `too_large`, `unsupported_schema`, …). */
            reason: string;
        } | {
            /**
             * Format: int64
             * @description How many artifacts it lists.
             */
            entries: number;
            /** @enum {string} */
            kind: "archive.manifest.written";
            /** @description Manifest path relative to the media root. */
            path: string;
        } | {
            /**
             * Format: int64
             * @description Files present under the podcast that the manifest does not list.
             */
            added: number;
            /**
             * Format: int64
             * @description Listed files whose bytes differ.
             */
            changed: number;
            /** @enum {string} */
            kind: "archive.manifest.mismatch";
            /**
             * Format: int64
             * @description Listed files that are gone.
             */
            missing: number;
            /** @description Manifest path relative to the media root. */
            path: string;
        } | {
            /** @description Whether records were written (`false` is a dry run). */
            applied: boolean;
            /**
             * Format: int64
             * @description Sidecars that contradict an existing record, or each other.
             */
            conflicts: number;
            /** @enum {string} */
            kind: "archive.rebuild.completed";
            /**
             * Format: int64
             * @description Records restored.
             */
            rebuilt: number;
            /**
             * Format: int64
             * @description Sidecars read.
             */
            scanned: number;
            /**
             * Format: int64
             * @description Records that already agreed with their sidecar.
             */
            unchanged: number;
            /**
             * Format: int64
             * @description Sidecars naming an episode this library does not have.
             */
            unknown_episode: number;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /**
             * Format: int32
             * @description Matching confidence in hundredths (100 = certain).
             */
            confidence: number;
            /** @enum {string} */
            kind: "archive.imported";
            /** @description Why it matched (`scored`, `embedded_guid`, `source_database`). */
            matched_by: string;
            /** @description Where it landed, relative to the media root. */
            path: string;
        } | {
            /** @enum {string} */
            kind: "archive.import.skipped";
            /** @description Detail. */
            reason: string;
            /** @description How it was classified (`ambiguous`, `unmatched`, `already_present`, …). */
            state: string;
        } | {
            /**
             * Format: int64
             * @description Files Uguisu already has, byte for byte.
             */
            already_present: number;
            /**
             * Format: int64
             * @description Files that matched more than one episode too closely to choose.
             */
            ambiguous: number;
            /** @description Whether files were copied (`false` is a dry run). */
            applied: boolean;
            /** @description The layout that read the source tree. */
            format: string;
            /**
             * Format: int64
             * @description Files imported.
             */
            imported: number;
            /** @enum {string} */
            kind: "archive.import.completed";
            /**
             * Format: int64
             * @description Files examined.
             */
            scanned: number;
            /**
             * Format: int64
             * @description Files that matched nothing well enough.
             */
            unmatched: number;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @description The managed fields that changed. */
            fields: string[];
            /** @description The file's new hash. */
            hash_value: string;
            /** @enum {string} */
            kind: "archive.tagged";
            /** @description Which mode wrote them. */
            mode: components["schemas"]["TagMode"];
            /** @description Path relative to the media root. */
            path: string;
            /**
             * Format: int64
             * @description The file's new size.
             */
            size_bytes: number;
        } | {
            /** @description Archive record id. */
            archive_file_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "archive.tags.skipped";
            /** @description Why (`unsupported`, `nothing_to_write`, `not_verified`, …). */
            reason: string;
        } | {
            /** @description Artwork record id. */
            artwork_id: components["schemas"]["Ulid"];
            /** @description The container, as recognised from the bytes. */
            format: components["schemas"]["ArtworkFormat"];
            /** @description Hash of the file. */
            hash_value: string;
            /** @enum {string} */
            kind: "podcast.artwork.fetched";
            /** @description Path relative to the media root. */
            path: string;
            /**
             * Format: int64
             * @description Length in bytes.
             */
            size_bytes: number;
        } | {
            /** @description The artwork that was revalidated. */
            artwork_id: components["schemas"]["Ulid"];
            /** @enum {string} */
            kind: "podcast.artwork.unchanged";
        } | {
            /** @enum {string} */
            kind: "podcast.artwork.failed";
            /**
             * @description Why: the HTTP client's error kind (`policy`, `dns`, `connect`,
             *     `timeout`, `body_too_large`, …), `http_<status>`, or why the image
             *     was refused.
             */
            reason: string;
            /**
             * Format: uri
             * @description Where it was fetched from.
             */
            url: string;
        } | {
            /**
             * Format: int64
             * @description Podcasts the pass found due.
             */
            due: number;
            /** @enum {string} */
            kind: "scheduler.tick";
            /**
             * Format: int64
             * @description How long it intends to sleep before looking again.
             */
            sleep_ms: number;
            /**
             * Format: int64
             * @description Refreshes it started (bounded by the free concurrency).
             */
            started: number;
        } | {
            /** @enum {string} */
            kind: "scheduler.paused";
            /** @description Why, as the operator or the engine said it. */
            reason: string;
        } | {
            /** @enum {string} */
            kind: "scheduler.resumed";
        } | {
            /** @description The `UGUISU_*` key. */
            key: string;
            /** @enum {string} */
            kind: "settings.changed";
            /** @description Whether the row was removed rather than written. */
            removed: boolean;
            /** @description Whether the change needs a restart to take effect. */
            restart_required: boolean;
        } | {
            /** @description The `UGUISU_*` key, or an unknown one. */
            key: string;
            /** @enum {string} */
            kind: "settings.rejected";
            /** @description The parser's message, or why the key is refused. */
            message: string;
        } | {
            /**
             * Format: int64
             * @description How long it took.
             */
            duration_ms: number;
            /**
             * Format: int64
             * @description Episodes indexed.
             */
            episodes: number;
            /** @description Whether the index was rebuilt from scratch. */
            full: boolean;
            /** @enum {string} */
            kind: "search.reindexed";
            /**
             * Format: int64
             * @description Podcasts indexed.
             */
            podcasts: number;
        };
        /** @description Answer of `GET /api/v1/events?after=`. */
        EventList: {
            events: components["schemas"]["Event"][];
        };
        /**
         * @description What a desktop bootstrap answers with.
         *
         *     The session cookie is in `Set-Cookie`, where script cannot read it; nothing
         *     here is a secret the page has to keep.
         */
        ExchangeBody: {
            csrf_token: string;
            /** Format: date-time */
            expires_at: string;
        };
        /** @description One row of the fetch log (`feed_fetches`). */
        FeedFetch: components["schemas"]["RefreshOutcome"] & {
            /**
             * Format: int64
             * @description Wall-clock duration.
             */
            duration_ms: number;
            /** @description Episode counts. */
            episodes: components["schemas"]["EpisodeCounts"];
            /**
             * Format: date-time
             * @description When the attempt started.
             */
            fetched_at: string;
            /** @description The body fingerprint differed from the previous one. */
            fingerprint_changed: boolean;
            /** @description HTTP summary. */
            http: components["schemas"]["HttpSummary"];
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description Some items were malformed. */
            partial: boolean;
            /** @description Podcast metadata changed. */
            podcast_changed: boolean;
            /** @description Podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Source that was fetched. */
            source_id: components["schemas"]["Ulid"];
            /** @description The document was truncated. */
            truncated: boolean;
            /** @description A new feed URL was announced. */
            url_change_detected: boolean;
            /** @description Warnings. */
            warnings: string[];
        };
        /** @description Feed health hints reported by directories (never authoritative). */
        FeedHealthHints: {
            /** @description Directory marks the feed as dead. */
            dead?: boolean | null;
            /**
             * Format: int32
             * @description Last HTTP status the directory got from the feed.
             */
            http_status?: number | null;
            /**
             * Format: date-time
             * @description Last time the directory saw the feed change.
             */
            last_update?: string | null;
            /** @description `podcast:locked` as seen by the directory. */
            locked?: boolean | null;
        };
        /**
         * @description Syntax family of a feed document.
         * @enum {string}
         */
        FeedKind: "rss2" | "atom" | "rss1";
        /** @description What [`Engine::move_feed`] found and did. */
        FeedMove: {
            /** @description Why it passed, or why not. */
            check: string;
            /**
             * Format: uri
             * @description Its feed URL before the move.
             */
            from: string;
            /** @description Whether the podcast now uses `to`. */
            moved: boolean;
            /** @description The podcast. */
            podcast_id: components["schemas"]["Ulid"];
            report?: null | components["schemas"]["RefreshReport"];
            /**
             * Format: uri
             * @description The feed URL it moves to, after redirects.
             */
            to: string;
            /** @description Whether the same-show check passed. */
            verified: boolean;
        };
        /** @description Fetch state of a source, as `feed status` shows it. */
        FeedStatus: {
            last_fetch?: null | components["schemas"]["FeedFetch"];
            /** @description The source. */
            source: components["schemas"]["PodcastSource"];
        };
        /** @description Feed URL status after a refresh. */
        FeedUrlStatus: {
            /** @enum {string} */
            status: "unchanged";
        } | {
            /**
             * Format: uri
             * @description The announced URL.
             */
            announced: string;
            /** @description Why it was not adopted. */
            reason: string;
            /** @enum {string} */
            status: "change_detected";
        } | {
            /**
             * Format: uri
             * @description Previous URL.
             */
            from: string;
            /** @enum {string} */
            status: "changed";
            /**
             * Format: uri
             * @description New URL.
             */
            to: string;
            /** @description How the change was announced. */
            via: components["schemas"]["ReplacementReason"];
        };
        /**
         * @description Classification of a failed feed fetch or parse. Drives retry and
         *     scheduling policies later; stored with the source and the fetch log.
         * @enum {string}
         */
        FetchErrorKind: "network_error" | "timeout" | "dns_error" | "tls_error" | "http_client_error" | "http_server_error" | "rate_limited" | "unauthorized" | "forbidden" | "not_found" | "malformed_xml" | "unsupported_feed" | "too_large" | "too_deep" | "invalid_content_type" | "invalid_podcast_feed" | "blocked_by_policy" | "cancelled";
        /**
         * @description Fetch state of a podcast source (`docs/STATE_MACHINES.md` §6).
         * @enum {string}
         */
        FetchState: "never_fetched" | "fetching" | "fetched" | "not_modified" | "failed" | "disabled";
        /**
         * @description Fetch bookkeeping of a source: what happened last and what the next
         *     conditional request must send.
         */
        FetchStatus: {
            /**
             * Format: int32
             * @description Failures since the last success or not-modified answer.
             */
            consecutive_failures: number;
            /** @description SHA-256 (hex) of the last successfully processed body. */
            content_fingerprint?: string | null;
            /** @description `ETag` of the last successful fetch. */
            etag?: string | null;
            /**
             * Format: date-time
             * @description Last attempt, successful or not.
             */
            last_attempt_at?: string | null;
            /**
             * Format: int64
             * @description Size in bytes of the last successfully processed body.
             */
            last_content_length?: number | null;
            /**
             * Format: date-time
             * @description Last failed attempt.
             */
            last_error_at?: string | null;
            /** @description Detail of the last error. */
            last_error_detail?: string | null;
            last_error_kind?: null | components["schemas"]["FetchErrorKind"];
            /**
             * Format: int32
             * @description HTTP status of the last attempt, when one was received.
             */
            last_http_status?: number | null;
            /** @description `Last-Modified` of the last successful fetch. */
            last_modified?: string | null;
            /**
             * Format: date-time
             * @description Last attempt answered "not modified".
             */
            last_not_modified_at?: string | null;
            /**
             * Format: date-time
             * @description Last attempt that fetched and processed a feed.
             */
            last_success_at?: string | null;
            /** @description Current state. */
            state: components["schemas"]["FetchState"];
        };
        /** @description A group of findings: how many, and the first few by name. */
        Findings: {
            /** Format: int64 */
            count: number;
            sample: string[];
        };
        /** @description A funding link (`podcast:funding`). */
        Funding: {
            /** @description Call to action. */
            text?: string | null;
            /** @description URL. */
            url: string;
        };
        /** @description Body of `GET /api/v1/health`. */
        Health: {
            /** @description Always `"ok"` when the process can answer at all. */
            status: string;
            /** @description Uguisu version. */
            version: string;
        };
        /** @description What the HTTP exchange looked like. */
        HttpSummary: {
            /**
             * Format: int64
             * @description Body size in bytes.
             */
            bytes?: number | null;
            /** @description Whether a conditional request was sent. */
            conditional: boolean;
            /** @description `ETag` of the response. */
            etag?: string | null;
            /** @description The `ETag` changed compared to the stored one. */
            etag_changed: boolean;
            /**
             * Format: uri
             * @description Final URL after redirects.
             */
            final_url?: string | null;
            /** @description `Last-Modified` of the response. */
            last_modified?: string | null;
            /**
             * Format: int32
             * @description Number of redirects followed.
             */
            redirects: number;
            /**
             * Format: int32
             * @description Status code, when a response was received.
             */
            status?: number | null;
        };
        /**
         * @description Which signal produced an episode's identity key (ADR 0006).
         * @enum {string}
         */
        IdentitySource: "guid" | "enclosure_url" | "fingerprint";
        /** @description What an import would do, or did. */
        ImportBody: {
            /** Format: int64 */
            already_present: number;
            /** Format: int64 */
            ambiguous: number;
            applied: boolean;
            /** Format: int64 */
            conflicts: number;
            format: string;
            /** Format: int64 */
            imported: number;
            /** Format: int64 */
            invalid: number;
            items: components["schemas"]["ImportItem"][];
            /** Format: int64 */
            scanned: number;
            source_root: string;
            /** Format: int32 */
            threshold: number;
            /** Format: int64 */
            unmatched: number;
            /** Format: int64 */
            unreadable: number;
        };
        ImportItem: {
            action: string;
            /** Format: int32 */
            confidence: number;
            detail?: string | null;
            episode_id?: null | components["schemas"]["Ulid"];
            matched_by?: string | null;
            podcast_id?: null | components["schemas"]["Ulid"];
            /** Format: int64 */
            size_bytes: number;
            source_path: string;
            target_path?: string | null;
        };
        /** @description Body of `POST /api/v1/archive/import`. */
        ImportRequest: {
            /** @description Copy the files. Absent means a dry run. */
            apply?: boolean | null;
            /** @description The layout; detected when absent. */
            format?: string | null;
            /** @description The directory to read, on the server's own filesystem. */
            path: string;
            /** @description Import only into this podcast. */
            podcast?: string | null;
            /**
             * @description Podgrab's database, on the server's own filesystem, read only to
             *     name each file exactly (ADR 0050); implies the Podgrab layout.
             */
            podgrab_db?: string | null;
            /**
             * Format: int32
             * @description Confidence a match needs, in percent.
             */
            threshold?: number | null;
        };
        /**
         * @description Whether the index can be trusted.
         * @enum {string}
         */
        IndexState: "ready" | "building" | "stale";
        /** @description One item as the engine would see it. */
        InspectedItem: {
            /**
             * Format: int32
             * @description Duration in seconds, when parseable.
             */
            duration_secs?: number | null;
            /**
             * Format: uri
             * @description Primary enclosure URL.
             */
            enclosure_url?: string | null;
            /** @description Number of enclosures (primary and alternates). */
            enclosures: number;
            /** @description Identity key the item would get. */
            identity_key: string;
            /** @description Position in the feed. */
            index: number;
            /**
             * Format: date-time
             * @description Publication instant, when parseable.
             */
            published_at?: string | null;
            /** @description Date quality. */
            published_at_quality: components["schemas"]["DateQuality"];
            /** @description Normalized title. */
            title: string;
            /** @description Normalization warnings. */
            warnings: string[];
        };
        /** @description What `feed inspect` reports. */
        Inspection: {
            /** @description Channel author. */
            author?: string | null;
            /** @description Number of categories. */
            categories: number;
            /**
             * Format: int64
             * @description Wall-clock time of fetch and parse.
             */
            duration_ms: number;
            /** @description Encoding the body was decoded with. */
            encoding: string;
            /** @description HTTP facts of the fetch. */
            http: components["schemas"]["HttpSummary"];
            /** @description Identity source → number of items. */
            identity_sources: {
                [key: string]: number;
            };
            /** @description Items parsed. */
            items: number;
            /** @description Items with at least one enclosure. */
            items_with_enclosure: number;
            /** @description Syntax family. */
            kind: components["schemas"]["FeedKind"];
            /** @description Language. */
            language?: string | null;
            /** @description `podcast:locked`. */
            locked?: boolean | null;
            /** @description Whether the feed looks like a podcast feed. */
            looks_like_podcast: boolean;
            /** @description Items the parser had to isolate. */
            malformed_items: number;
            /**
             * Format: uri
             * @description `itunes:new-feed-url`.
             */
            new_feed_url?: string | null;
            /** @description `podcast:guid`. */
            podcast_guid?: string | null;
            /** @description The first items. */
            preview: components["schemas"]["InspectedItem"][];
            /**
             * Format: int32
             * @description Report schema version.
             */
            schema: number;
            /** @description Channel title. */
            title: string;
            /** @description Whether the parser stopped early. */
            truncated: boolean;
            /**
             * Format: uri
             * @description The URL that was asked for.
             */
            url: string;
            /** @description Total number of warnings before capping. */
            warning_count: number;
            /** @description Parser, channel and item warnings (capped). */
            warnings: string[];
            /**
             * Format: uri
             * @description Website.
             */
            website?: string | null;
        };
        /** @description A job command's answer: the job after the command. */
        JobBody: {
            job: components["schemas"]["DownloadJob"];
        };
        /** @description A job with its history and live progress. */
        JobDetail: {
            /** @description Attempts, oldest first. */
            attempts: components["schemas"]["DownloadAttempt"][];
            /** @description The job. */
            job: components["schemas"]["DownloadJob"];
            progress?: null | components["schemas"]["ProgressSnapshot"];
        };
        /** @description One page of jobs, newest first. */
        JobPage: {
            /** @description Jobs, each with the episode and podcast it belongs to named. */
            jobs: components["schemas"]["JobSummary"][];
            next_after?: null | components["schemas"]["Ulid"];
        };
        /**
         * @description A queue row with the names a reader needs.
         *
         *     Flattened on the wire, so a client that knew only `DownloadJob` still reads
         *     every field it knew and gains four. `DownloadJob` itself is untouched: it
         *     is the persisted record, and an episode's title is not part of it — it
         *     belongs to the episode, and duplicating it into the job would make the two
         *     able to disagree.
         */
        JobSummary: components["schemas"]["DownloadJob"] & {
            /** @description The episode's title. */
            episode_title: string;
            /**
             * Format: float
             * @description Transferred percentage, when the total length is known.
             */
            percentage?: number | null;
            /** @description Its podcast's title. */
            podcast_title: string;
            /**
             * Format: date-time
             * @description When the episode was published, when the feed said so.
             */
            published_at?: string | null;
        };
        /** @description One key, as `config validate` and the settings API report it. */
        KeyDescription: {
            /** @description The `UGUISU_*` name. */
            key: string;
            /** @description Whether a change takes effect without a restart. */
            live: boolean;
            /** @description Which layer that value came from. */
            origin: components["schemas"]["Origin"];
            /** @description Whether this key may be stored at all. */
            persistable: boolean;
            /** @description Whether a stored value exists but something above it wins. */
            pinned: boolean;
            /**
             * @description The stored value, if there is one — **even when it is not the one
             *     in force**, because "I set that and nothing happened" is the
             *     question this field exists to answer.
             */
            stored?: string | null;
            /** @description The value in force, rendered; secrets are redacted. */
            value: string;
        };
        /** @description A licence (`podcast:license`). */
        License: {
            /** @description Identifier or text. */
            text: string;
            /** @description URL of the licence. */
            url?: string | null;
        };
        /** @description A location (`podcast:location`). */
        Location: {
            /** @description `geo` URI. */
            geo?: string | null;
            /** @description Name. */
            name: string;
            /** @description OpenStreetMap reference. */
            osm?: string | null;
        };
        /**
         * @description What a login answers with. Never the password, and never the cookie value:
         *     that goes in `Set-Cookie`, where script cannot read it.
         */
        LoginBody: {
            csrf_token: string;
            /** Format: date-time */
            expires_at: string;
            username: string;
        };
        /** @description Body of `POST /api/v1/auth/login`. */
        LoginRequest: {
            password: string;
            username: string;
        };
        /** @description What the housekeeping pass did. */
        MaintenanceReport: {
            /**
             * Format: int64
             * @description Expired discovery cache rows deleted.
             */
            cache_expired: number;
            /**
             * Format: int64
             * @description Event rows deleted.
             */
            events_pruned: number;
            /**
             * Format: int64
             * @description Sessions that can no longer authenticate anything, deleted.
             */
            sessions_pruned: number;
        };
        /** @description What a manifest check found. */
        ManifestCheckBody: {
            added: components["schemas"]["Findings"];
            changed: components["schemas"]["Findings"];
            clean: boolean;
            missing: components["schemas"]["Findings"];
            path: string;
            podcast_id: components["schemas"]["Ulid"];
            rehashed: boolean;
            stale: boolean;
            /** Format: int64 */
            unchanged: number;
            unreadable: components["schemas"]["Findings"];
        };
        /** @description Manifest state per podcast. */
        ManifestPage: {
            manifests: components["schemas"]["ArchiveManifest"][];
        };
        /** @description What writing one or more manifests produced. */
        ManifestWriteBody: {
            written: components["schemas"]["ManifestWritten"][];
        };
        ManifestWritten: {
            /**
             * @description `false` when a change landed while the file was being written, so
             *     the manifest is still stale and the next flush will redo it.
             */
            cleared: boolean;
            /** Format: int64 */
            entries: number;
            path: string;
            podcast_id: components["schemas"]["Ulid"];
        };
        /** @description Body of `POST /api/v1/podcasts/{id}/move-feed`. */
        MoveFeedRequest: {
            /** @description Check, and never move. */
            dry_run?: boolean;
            /** @description Move even when the same-show check fails. */
            force?: boolean;
            /** @description The feed URL to move to. */
            url: string;
        };
        /** @description What creating a token answers with. The only place the secret exists. */
        NewTokenBody: {
            secret: string;
            token: components["schemas"]["ApiToken"];
        };
        /** @description A normalized search input. */
        NormalizedQuery: {
            /** @description `folded` with diacritics and non-Latin scripts transliterated to ASCII. */
            ascii: string;
            /** @description NFKC, case-folded, punctuation removed, whitespace collapsed; Unicode kept. */
            folded: string;
            /** @description Term or URL. */
            kind: components["schemas"]["QueryKind"];
            /** @description Input as typed (trimmed). */
            raw: string;
            /** @description Tokens of `ascii`, in order, duplicates kept. */
            tokens: string[];
        };
        /**
         * @description Why a refresh found nothing new without parsing.
         * @enum {string}
         */
        NotModifiedReason: "http304" | "fingerprint";
        /**
         * @description What became, or would become, of one outline.
         * @enum {string}
         */
        OpmlAction: "add" | "already_present" | "duplicate" | "invalid" | "needs_review" | "conflict" | "failed";
        /** @description How many outlines ended in each action. */
        OpmlCounts: {
            /**
             * Format: int64
             * @description New feeds, added or to be added.
             */
            add: number;
            /**
             * Format: int64
             * @description Feeds the library already has.
             */
            already_present: number;
            /**
             * Format: int64
             * @description Second feeds of a show already in the library.
             */
            conflict: number;
            /**
             * Format: int64
             * @description Repeats within the document.
             */
            duplicate: number;
            /**
             * Format: int64
             * @description Feeds that could not be verified or stored.
             */
            failed: number;
            /**
             * Format: int64
             * @description Unusable URLs.
             */
            invalid: number;
            /**
             * Format: int64
             * @description Pages rather than feeds.
             */
            needs_review: number;
        };
        /** @description The plan of an import, or what applying it did. */
        OpmlImport: {
            /** @description Whether the new feeds were added. */
            applied: boolean;
            /** @description Outlines per action. */
            counts: components["schemas"]["OpmlCounts"];
            /** @description Every feed outline, in document order. */
            items: components["schemas"]["OpmlItem"][];
        };
        /** @description Body of `POST /api/v1/podcasts/opml`. */
        OpmlImportRequest: {
            /** @description Verify and add the new feeds. Absent means a dry run, which sends no request. */
            apply?: boolean | null;
            /** @description The OPML document, as text; at most 1 MiB. */
            opml: string;
            policy?: null | components["schemas"]["PolicyUpdate"];
        };
        /** @description One feed outline of the document and what became of it. */
        OpmlItem: {
            /** @description What became of it. */
            action: components["schemas"]["OpmlAction"];
            /** @description Why, for anything but `add` and `already_present`. */
            detail?: string | null;
            podcast_id?: null | components["schemas"]["Ulid"];
            /** @description The outline's title, else its text. */
            title?: string | null;
            /** @description The outline's `xmlUrl`, as written. */
            xml_url: string;
        };
        /**
         * @description Where a configuration value came from.
         *
         *     Declaration order *is* precedence, lowest first, and [`Origin::rank`]
         *     makes that usable. An explicitly set environment variable outranks a
         *     stored setting on purpose (ADR 0028): the operator who wrote it into a
         *     unit file or a compose file has to be able to rely on it, and a value
         *     the API would silently ignore is worse than one it refuses to take.
         * @enum {string}
         */
        Origin: "default" | "settings" | "env" | "cli";
        /** @description An embedded cover image, as [`OriginalTags`] records it. */
        OriginalCover: {
            /** @description Its MIME type. */
            mime: string;
            /** @description Lowercase hex SHA-256 of its bytes. */
            sha256: string;
            /**
             * Format: int64
             * @description Its length in bytes.
             */
            size_bytes: number;
        };
        /**
         * @description The managed tags a file carried before Uguisu first wrote any (ADR 0012).
         *
         *     Captured once, before the first tag write, and kept for the life of one
         *     download. A file Uguisu had already tagged when this was introduced never
         *     gets one: its tags are Uguisu's, and recording them as the original would
         *     be false.
         */
        OriginalTags: {
            /**
             * Format: date-time
             * @description When the tags were read.
             */
            captured_at: string;
            cover?: null | components["schemas"]["OriginalCover"];
            /**
             * @description Values by managed field name (`title`, `album`, …); a field the file
             *     did not carry is absent.
             */
            values: {
                [key: string]: string;
            };
        };
        /** @description What `archive orphans` found; nothing is ever removed. */
        OrphansBody: {
            clean: boolean;
            leftovers: components["schemas"]["Findings"];
            orphan_parts: components["schemas"]["Findings"];
            /** Format: int64 */
            scanned: number;
            stray_sidecars: components["schemas"]["Findings"];
            unknown_media: components["schemas"]["Findings"];
            unreadable: components["schemas"]["Findings"];
        };
        PassBody: {
            /** Format: int32 */
            due: number;
            paused: boolean;
            /** Format: int32 */
            started: number;
        };
        /** @description Body of `POST /api/v1/auth/password`. */
        PasswordRequest: {
            /**
             * @description Required once a credential exists: a session cookie alone must not be
             *     enough to change the password it was opened with.
             */
            current_password?: string | null;
            new_password: string;
            /** @description The operator's name. Defaults to the one already set, or `uguisu`. */
            username?: string | null;
        };
        /**
         * @description What the template would produce for an episode, without writing
         *     anything.
         */
        PathPreview: {
            /** @description The path it currently has, when it is archived. */
            current?: string | null;
            /** @description The episode. */
            episode_id: components["schemas"]["Ulid"];
            /** @description The path the template produces, before collisions are considered. */
            rendered: string;
            /** @description The path the episode would actually get, after collision handling. */
            resolved: string;
            /** @description The disambiguating suffix, when one was needed. */
            suffix?: string | null;
            /** @description The template that was rendered. */
            template: string;
            /** @description Whether [`Engine::relocate`] would move the file. */
            would_move: boolean;
        };
        /**
         * @description Why every download is paused (`download_control`).
         * @enum {string}
         */
        PauseAllReason: "user" | "disk_full";
        PauseBody: {
            /** @description Why, in the operator's own words. */
            reason?: string | null;
        };
        /** @description A person credit (`podcast:person`). */
        Person: {
            /** @description Group. */
            group?: string | null;
            /** @description Link. */
            href?: string | null;
            /** @description Image URL. */
            img?: string | null;
            /** @description Name. */
            name: string;
            /** @description Role. */
            role?: string | null;
        };
        /** @description A show as the user sees it. Never contains provider-specific fields. */
        Podcast: {
            /**
             * Format: uri
             * @description Artwork URL as published.
             */
            artwork_url?: string | null;
            /** @description `itunes:author`. */
            author?: string | null;
            /** @description Categories, flattened (`Technology`, `Technology / Tech News`). */
            categories: string[];
            /** @description `copyright`. */
            copyright?: string | null;
            /**
             * Format: date-time
             * @description Creation time.
             */
            created_at: string;
            /** @description Description with markup, as published. */
            description_html?: string | null;
            /** @description Description reduced to text. */
            description_text?: string | null;
            /** @description Resolved top-level folder name, filled by the archive engine. */
            directory_name?: string | null;
            /** @description `itunes:explicit`. */
            explicit?: boolean | null;
            /** @description Feed syntax family of the current source. */
            feed_kind: components["schemas"]["FeedKind"];
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description BCP-47 language tag as published. */
            language?: string | null;
            /** @description Last error message shown to the user. */
            last_error?: string | null;
            /**
             * Format: date-time
             * @description Scheduler: last successful refresh.
             */
            last_refresh_at?: string | null;
            /** @description Hash of the comparable channel fields (change detection). */
            metadata_hash: string;
            /**
             * Format: date-time
             * @description Scheduler: next planned refresh.
             */
            next_refresh_at?: string | null;
            /** @description `itunes:owner/itunes:email`. */
            owner_email?: string | null;
            /** @description `itunes:owner/itunes:name`. */
            owner_name?: string | null;
            /** @description `podcast:guid`. */
            podcast_guid?: string | null;
            /** @description Publisher (`managingEditor` / `dc:publisher`) when distinct from the author. */
            publisher?: string | null;
            /**
             * Format: int64
             * @description Per-podcast refresh interval override.
             */
            refresh_interval_secs?: number | null;
            /** @description Normalized title for ordering (articles stripped, case folded). */
            sort_title: string;
            /** @description Lifecycle status. */
            status: components["schemas"]["PodcastStatus"];
            /** @description `itunes:subtitle`. */
            subtitle?: string | null;
            /** @description Title as published. */
            title: string;
            /**
             * Format: date-time
             * @description Last modification time.
             */
            updated_at: string;
            /**
             * Format: uri
             * @description Show website.
             */
            website?: string | null;
        };
        /**
         * @description A stored artwork file for a podcast.
         *
         *     Artwork is content-addressed, so fetching a replacement can never
         *     destroy the previous one; exactly one row per podcast carries
         *     `is_current`. It lives in its own table because `ArchiveFile` is for
         *     episode media and its `episode_id` is unique.
         */
        PodcastArtwork: {
            /** @description `Content-Type` as served (kept for the record; never trusted alone). */
            content_type?: string | null;
            /**
             * Format: date-time
             * @description Row creation.
             */
            created_at: string;
            /** @description `ETag` of the response, for the next conditional request. */
            etag?: string | null;
            /** @description The container, as recognised from the bytes. */
            format: components["schemas"]["ArtworkFormat"];
            /** @description Hash algorithm (`sha256`). */
            hash_algo: string;
            /** @description Hash of the file; also its name on disk. */
            hash_value: string;
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description Whether this is the artwork Uguisu currently uses. */
            is_current: boolean;
            /** @description `Last-Modified` of the response. */
            last_modified?: string | null;
            /** @description The podcast it belongs to. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Path relative to the media root, POSIX separators. */
            relative_path: string;
            /**
             * Format: date-time
             * @description When it was fetched.
             */
            retrieved_at: string;
            /**
             * Format: int64
             * @description Length in bytes.
             */
            size_bytes: number;
            /**
             * Format: uri
             * @description Where it was fetched from.
             */
            source_url?: string | null;
            /**
             * Format: date-time
             * @description Last change.
             */
            updated_at: string;
        };
        /**
         * @description A podcast as seen by one or more providers, normalized.
         *
         *     Only information the provider actually supplied is set; nothing is
         *     invented. `provenance` records which provider supplied each field so a
         *     merged candidate can still explain itself.
         */
        PodcastCandidate: {
            /**
             * Format: uri
             * @description Artwork URL.
             */
            artwork?: string | null;
            /** @description Author / host. */
            author?: string | null;
            /** @description Category names. */
            categories: string[];
            /** @description Description, plain text. */
            description?: string | null;
            /**
             * Format: int32
             * @description Episode count as reported.
             */
            episode_count?: number | null;
            /** @description Explicit flag as reported. */
            explicit?: boolean | null;
            /**
             * Format: uri
             * @description Feed URL (`None` means the provider knows the show but not its feed).
             */
            feed_url?: string | null;
            /** @description Health hints. */
            health: components["schemas"]["FeedHealthHints"];
            /** @description Every provider that contributed to this candidate. */
            identities: components["schemas"]["ProviderIdentity"][];
            /**
             * Format: int64
             * @description Apple/iTunes collection id (cross-provider identifier).
             */
            itunes_id?: number | null;
            /** @description Language tag. */
            language?: string | null;
            /**
             * Format: date-time
             * @description Newest episode date as reported.
             */
            last_published?: string | null;
            /** @description Podcasting 2.0 `podcast:guid` (cross-provider identifier). */
            podcast_guid?: string | null;
            /** @description Popularity signals, one per supplying provider. */
            popularity: components["schemas"]["Popularity"][];
            /** @description Field name → provider that supplied the value. */
            provenance: {
                [key: string]: components["schemas"]["ProviderId"];
            };
            /** @description Publisher or owner name. */
            publisher?: string | null;
            /** @description Show title. */
            title: string;
            /**
             * Format: uri
             * @description Podcast website.
             */
            website?: string | null;
        };
        /** @description A podcast with its current source and a few counters. */
        PodcastDetail: {
            announced?: null | components["schemas"]["PodcastSource"];
            /**
             * Format: int64
             * @description Episodes not detected as removed from the feed.
             */
            episodes_present: number;
            /**
             * Format: int64
             * @description Episodes stored for it.
             */
            episodes_total: number;
            last_fetch?: null | components["schemas"]["FeedFetch"];
            /** @description The podcast. */
            podcast: components["schemas"]["Podcast"];
            source?: null | components["schemas"]["PodcastSource"];
        };
        /** @description One podcast a search found. */
        PodcastHit: {
            /** @description Its author. */
            author?: string | null;
            /** @description The podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /**
             * Format: double
             * @description The relevance FTS5 computed.
             */
            relevance: number;
            /** @description Where the match is. */
            snippet: string;
            /** @description Its title. */
            title: string;
        };
        /** @description One page of podcasts. */
        PodcastPage: {
            next_after?: null | components["schemas"]["Ulid"];
            /** @description The podcasts. */
            podcasts: components["schemas"]["PodcastDetail"][];
        };
        /** @description What removing a podcast took out of the library (ADR 0055). */
        PodcastRemoval: {
            /**
             * Format: int64
             * @description Episodes the library held for it.
             */
            episodes: number;
            /**
             * Format: int64
             * @description Archived files it had; every one is still on disk.
             */
            files: number;
            /** @description The podcast that is gone. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Its title. */
            title: string;
        };
        PodcastSchedule: {
            podcast_id: components["schemas"]["Ulid"];
            status: components["schemas"]["PodcastStatus"];
        };
        /**
         * @description Where a podcast's feed comes from, with history. Exactly one source per
         *     podcast is current.
         */
        PodcastSource: {
            /**
             * Format: uri
             * @description Canonical URL from `atom:link rel="self"` or redirects.
             */
            canonical_url?: string | null;
            /**
             * Format: date-time
             * @description Creation time.
             */
            created_at: string;
            /**
             * Format: date-time
             * @description When the source was discovered.
             */
            discovered_at: string;
            /**
             * Format: uri
             * @description Feed URL as configured or discovered.
             */
            feed_url: string;
            /** @description Fetch bookkeeping. */
            fetch: components["schemas"]["FetchStatus"];
            /** @description Identifier. */
            id: components["schemas"]["Ulid"];
            /** @description Whether this is the current source. */
            is_current: boolean;
            /** @description Owning podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Provider that produced the source (`apple`, `website`, `manual`, …). */
            provider: string;
            /** @description Provider-specific reference. */
            provider_ref?: string | null;
            replaced_by_source_id?: null | components["schemas"]["Ulid"];
            replacement_reason?: null | components["schemas"]["ReplacementReason"];
            /**
             * Format: date-time
             * @description Last modification time.
             */
            updated_at: string;
            /**
             * Format: date-time
             * @description When the feed was last verified as a podcast feed.
             */
            verified_at?: string | null;
            /**
             * Format: uri
             * @description Website URL at discovery time.
             */
            website_url?: string | null;
        };
        /**
         * @description Lifecycle status of a podcast (`docs/STATE_MACHINES.md` §5).
         * @enum {string}
         */
        PodcastStatus: "active" | "paused" | "error" | "archived";
        /** @description One podcast's policy, with the values actually in force. */
        PolicyBody: {
            /** @description What the global defaults and the override add up to. */
            effective: components["schemas"]["EffectiveBody"];
            podcast_id: components["schemas"]["Ulid"];
            stored?: null | components["schemas"]["ArchivePolicy"];
        };
        /** @description Every stored per-podcast policy. */
        PolicyList: {
            policies: components["schemas"]["ArchivePolicy"][];
        };
        /**
         * @description Whether a podcast's episodes are queued automatically.
         * @enum {string}
         */
        PolicyMode: "manual" | "auto";
        /**
         * @description One podcast's policy: the body of `PUT /api/v1/podcasts/{id}/policy`,
         *     and the `policy` an OPML import stores with each podcast it adds.
         *
         *     Every field but `mode` is optional and means "use the global default"
         *     when absent, which is how one podcast can override the backlog without
         *     pinning the age limit as a side effect.
         */
        PolicyUpdate: {
            /**
             * Format: int32
             * @description Ignore episodes published longer ago than this.
             */
            max_age_days?: number | null;
            /**
             * Format: int32
             * @description Episodes of this podcast that may await archiving at once.
             */
            max_backlog?: number | null;
            /** @description `manual` or `auto`. */
            mode: string;
            priority?: null | components["schemas"]["Priority"];
        };
        /** @description A popularity signal as supplied by one provider. */
        Popularity: {
            /** @description What the raw value means (`subscribers`, `rank`). */
            label: string;
            /**
             * Format: float
             * @description Provider-specific normalization to 0..1.
             */
            normalized: number;
            /** @description Provider that supplied it. */
            provider: components["schemas"]["ProviderId"];
            /**
             * Format: double
             * @description Raw value (subscribers, rank…).
             */
            raw: number;
        };
        /**
         * @description Queue priority; higher runs first, FIFO within a level.
         * @enum {string}
         */
        Priority: "low" | "normal" | "high";
        /** @description Live progress of a running job. */
        ProgressSnapshot: {
            /**
             * Format: date-time
             * @description When the snapshot was taken.
             */
            at: string;
            /**
             * Format: int64
             * @description Bytes on disk including the resumed prefix.
             */
            bytes_downloaded: number;
            /**
             * Format: int64
             * @description Remaining seconds at the smoothed rate, when the total is known.
             */
            eta_secs?: number | null;
            /**
             * Format: float
             * @description Completion when the total is known.
             */
            percentage?: number | null;
            /**
             * Format: int64
             * @description Smoothed transfer rate (exponential moving average of 1 s samples).
             */
            speed_bps: number;
            /**
             * Format: int64
             * @description Complete length when known.
             */
            total_bytes?: number | null;
        };
        /**
         * @description How one provider fared.
         * @enum {string}
         */
        ProviderCallStatus: "ok" | "skipped" | "failed" | "timed_out" | "cancelled";
        /** @description Health of one provider. */
        ProviderHealth: {
            /**
             * Format: double
             * @description Exponentially weighted average latency of successful calls.
             */
            avg_latency_ms?: number | null;
            /** @description Circuit state. */
            circuit: components["schemas"]["CircuitState"];
            /**
             * Format: int32
             * @description Consecutive failures.
             */
            consecutive_failures: number;
            /** @description Kind of the last failure. */
            last_error?: string | null;
            /**
             * Format: date-time
             * @description Last failure and its kind.
             */
            last_failure?: string | null;
            /**
             * Format: date-time
             * @description Last successful call.
             */
            last_success?: string | null;
            /**
             * Format: int64
             * @description Total calls attempted (excluding cache hits).
             */
            total_calls: number;
            /**
             * Format: int64
             * @description Total failures.
             */
            total_failures: number;
        };
        /**
         * @description Identifies a discovery provider (directory, index or resolver).
         *
         *     Known providers are exposed as constants; the type is open so that
         *     additional providers can be registered without touching this crate.
         */
        ProviderId: string;
        /** @description Where a provider knows the podcast from. */
        ProviderIdentity: {
            /**
             * Format: float
             * @description The provider's confidence that this record matches the query (0..1).
             */
            confidence: number;
            /**
             * Format: date-time
             * @description When the record was fetched.
             */
            fetched_at: string;
            /** @description The provider. */
            provider: components["schemas"]["ProviderId"];
            /** @description The provider's own identifier (Apple `collectionId`, Podcast Index feed `id`, gpodder feed URL…). */
            provider_ref: string;
            /**
             * Format: uri
             * @description A directory page for the podcast at this provider, when one exists.
             */
            url?: string | null;
        };
        /** @description Per-provider outcome of a search. */
        ProviderOutcome: {
            /** @description Candidates contributed before dedup. */
            candidates: number;
            /** @description Error message when failed/skipped. */
            error?: string | null;
            /** @description Stable error kind. */
            error_kind?: string | null;
            /** @description Served from cache. */
            from_cache: boolean;
            /**
             * Format: int64
             * @description Wall-clock latency.
             */
            latency_ms: number;
            /** @description Provider. */
            provider: components["schemas"]["ProviderId"];
            /** @description Status. */
            status: components["schemas"]["ProviderCallStatus"];
        };
        /** @description Status exposed by the API and CLI. */
        ProviderStatus: {
            /** @description Attribution text. */
            attribution?: string | null;
            /** @description Capabilities. */
            capabilities: components["schemas"]["Capabilities"];
            /** @description Why it is disabled, when it is. */
            disabled_reason?: string | null;
            /** @description Documentation link. */
            docs_url: string;
            /** @description Whether searches use this provider. */
            enabled: boolean;
            /** @description Health. */
            health: components["schemas"]["ProviderHealth"];
            /** @description Identifier. */
            id: components["schemas"]["ProviderId"];
            /** @description Name. */
            name: string;
            /** @description Whether credentials are required. */
            requires_credentials: boolean;
            /**
             * Format: float
             * @description Trust weight.
             */
            trust: number;
        };
        /** @description Body of `GET /api/v1/discovery/providers`. */
        ProvidersResponse: {
            /** @description Cache counters. */
            cache: components["schemas"]["CacheStats"];
            /** @description Every registered provider with status and health. */
            providers: components["schemas"]["ProviderStatus"][];
        };
        /** @description Whether the input is a free-text term or a URL. */
        QueryKind: {
            /** @enum {string} */
            kind: "term";
        } | {
            /** @enum {string} */
            kind: "url";
            /**
             * Format: uri
             * @description A URL (feed, website or directory page) to resolve instead of searching.
             */
            url: string;
        };
        /** @description A ranked, merged candidate. */
        RankedCandidate: {
            /** @description Similar candidates that were deliberately not merged. */
            ambiguities: components["schemas"]["AmbiguityNote"][];
            /** @description The candidate. */
            candidate: components["schemas"]["PodcastCandidate"];
            /**
             * Format: float
             * @description Score normalized to 0..1.
             */
            confidence: number;
            /** @description Explanation. */
            explanation: components["schemas"]["RankingExplanation"];
            /** @description 1-based rank. */
            rank: number;
            /**
             * Format: float
             * @description Score in 0..`max_total`.
             */
            score: number;
        };
        /** @description An episode hit with its score. */
        RankedEpisode: components["schemas"]["EpisodeHit"] & {
            /**
             * Format: double
             * @description Sum of the contributions.
             */
            score: number;
            /** @description Why, when the caller asked. */
            signals: components["schemas"]["SearchSignal"][];
        };
        /** @description A podcast hit with its score. */
        RankedPodcast: components["schemas"]["PodcastHit"] & {
            /**
             * Format: double
             * @description Sum of the contributions.
             */
            score: number;
            /** @description Why, when the caller asked. */
            signals: components["schemas"]["SearchSignal"][];
        };
        /** @description Why a candidate scored what it scored. */
        RankingExplanation: {
            /**
             * Format: float
             * @description Maximum possible total for the configured weights.
             */
            max_total: number;
            /** @description Signals in evaluation order. */
            signals: components["schemas"]["Signal"][];
            /**
             * Format: float
             * @description Sum of contributions.
             */
            total: number;
        };
        /** @description An element Uguisu does not model, kept verbatim. */
        RawExtension: {
            /** @description Attributes. */
            attributes: {
                [key: string]: string;
            };
            /** @description Qualified name as written (`media:content`). */
            name: string;
            /** @description Text content. */
            text?: string | null;
        };
        /** @description What a rebuild found. */
        RebuildBody: {
            applied: boolean;
            conflicts: components["schemas"]["Findings"];
            malformed: components["schemas"]["Findings"];
            missing_media: components["schemas"]["Findings"];
            /** Format: int64 */
            rebuilt: number;
            /** Format: int64 */
            scanned: number;
            /** Format: int64 */
            unchanged: number;
            unknown_episode: components["schemas"]["Findings"];
            unreadable: components["schemas"]["Findings"];
        };
        /** @description Body of `POST /api/v1/archive/rebuild`. */
        RebuildRequest: {
            /** @description Write the records. Absent means a dry run. */
            apply?: boolean | null;
            /** @description Restrict to one podcast. */
            podcast?: string | null;
        };
        /** @description What startup reconciliation found and did. */
        ReconcileReport: {
            /**
             * Format: int32
             * @description `finalizing` jobs failed as `target_exists`: what is at the target
             *     is not the file they downloaded, and both it and the `.part` stay.
             */
            finalization_conflicts?: number;
            /**
             * Format: int32
             * @description `finalizing` jobs failed as `finalization`: the `.part` is there, but
             *     renaming it failed again for another reason, and it stays.
             */
            finalization_failed?: number;
            /**
             * Format: int32
             * @description `finalizing` jobs with neither `.part` nor target.
             */
            finalization_lost: number;
            /**
             * Format: int32
             * @description `finalizing` jobs whose target was in place or could be renamed now.
             */
            finalized: number;
            /**
             * Format: int32
             * @description `paused` jobs left alone.
             */
            kept_paused: number;
            /**
             * Format: int32
             * @description `retrying` jobs left alone.
             */
            kept_retrying: number;
            /**
             * Format: int32
             * @description Deep only: completed jobs whose target is missing.
             */
            missing_targets: number;
            /** @description `.part` files under `.uguisu-tmp` that belong to no job (relative paths). */
            orphan_parts: string[];
            paused_all?: null | components["schemas"]["PauseAllReason"];
            /**
             * Format: int32
             * @description `downloading` jobs re-queued as `queued(recovered)`.
             */
            recovered: number;
            /**
             * Format: int32
             * @description Deep only: pending jobs whose target already exists.
             */
            unexpected_targets: number;
        };
        RecordList: {
            records: components["schemas"]["DiscoveryRecord"][];
        };
        /** @description Answer of `POST /api/v1/podcasts/refresh`. */
        RefreshAllBody: {
            entries: components["schemas"]["RefreshAllEntry"][];
        };
        /** @description One podcast's result inside a [`Engine::refresh_all`] run. */
        RefreshAllEntry: {
            error?: null | components["schemas"]["UguisuError"];
            /** @description The podcast. */
            podcast_id: components["schemas"]["Ulid"];
            report?: null | components["schemas"]["RefreshReport"];
            /** @description Its title at the time of the run. */
            title: string;
        };
        /** @description Outcome of a refresh. */
        RefreshOutcome: {
            /** @enum {string} */
            outcome: "fetched";
        } | {
            /** @enum {string} */
            outcome: "not_modified";
            /** @description How that was determined. */
            reason: components["schemas"]["NotModifiedReason"];
        } | {
            /** @description Detail. */
            detail: string;
            /** @description Classification. */
            kind: components["schemas"]["FetchErrorKind"];
            /** @enum {string} */
            outcome: "failed";
        };
        /** @description Everything a refresh found out (brief §56). */
        RefreshReport: components["schemas"]["RefreshOutcome"] & {
            /**
             * Format: int64
             * @description Wall-clock duration.
             */
            duration_ms: number;
            /** @description Episode counts. */
            episodes: components["schemas"]["EpisodeCounts"];
            /** @description Feed URL status. */
            feed_url: components["schemas"]["FeedUrlStatus"];
            /** @description The fetch log entry. */
            fetch_id: components["schemas"]["Ulid"];
            /** @description HTTP exchange summary. */
            http: components["schemas"]["HttpSummary"];
            /** @description Podcast-level fields that changed. */
            podcast_changed_fields: string[];
            /** @description The podcast. */
            podcast_id: components["schemas"]["Ulid"];
            /** @description Whether removal detection was skipped and why. */
            removal_suppressed?: string | null;
            /**
             * Format: int32
             * @description Report schema version.
             */
            schema: number;
            /** @description The source that was refreshed (the current one before any migration). */
            source_id: components["schemas"]["Ulid"];
            /** @description Whether the feed document was truncated by a limit or an XML error. */
            truncated: boolean;
            /** @description Parser and pipeline warnings. */
            warnings: string[];
        };
        /** @description What a rebuild did. */
        ReindexReport: {
            /**
             * Format: int64
             * @description How long it took.
             */
            duration_ms: number;
            /**
             * Format: int64
             * @description Episodes indexed.
             */
            episodes: number;
            /**
             * Format: int64
             * @description Podcasts indexed.
             */
            podcasts: number;
        };
        /** @description A stored setting the engine is not applying, and why. */
        RejectedSetting: {
            /** @description The `UGUISU_*` key. */
            key: string;
            /** @description What the parser said about it. */
            message: string;
            /** @description The value as stored, unchanged. */
            value: string;
        };
        /**
         * @description A looser query asked because every provider answered the query as typed
         *     with nothing (ADR 0005).
         */
        RelaxedQuery: {
            /** @description How each provider answered it. */
            providers: components["schemas"]["ProviderOutcome"][];
            /** @description The query sent to the providers. */
            query: string;
        };
        /** @description Body of `POST /api/v1/archive/{episode_id}/relocate`. */
        RelocateBody: {
            /** @description Report what would happen without moving anything. */
            dry_run?: boolean;
        };
        /** @description What a relocation did, or would do. */
        Relocation: {
            /** @description The episode. */
            episode_id: components["schemas"]["Ulid"];
            /** @description Where the artifact was. */
            from: string;
            /** @description `false` for a dry run, or when the path did not change. */
            moved: boolean;
            /** @description Where it is now (or would be). */
            to: string;
        };
        /**
         * @description Why a source was replaced by another one.
         * @enum {string}
         */
        ReplacementReason: "redirect" | "new-feed-url" | "manual" | "matched-on-import";
        /**
         * @description How a resolution ended.
         * @enum {string}
         */
        ResolutionOutcome: "resolved" | "unresolved" | "failed";
        /** @description Body of `POST /api/v1/episodes/{id}/resolve`. */
        ResolutionRequest: {
            /**
             * @description `same` merges the candidate into the episode it duplicates;
             *     `separate` makes it an episode of its own.
             */
            resolution: components["schemas"]["DuplicateResolution"];
        };
        /** @description One step of a resolution. */
        ResolutionStep: {
            /** @description Detail. */
            detail: string;
            /**
             * Format: int64
             * @description Time since the resolution started.
             */
            elapsed_ms: number;
            /** @description Kind. */
            kind: components["schemas"]["StepKind"];
            /** @description Whether the step succeeded. */
            ok: boolean;
            /**
             * Format: uri
             * @description URL involved, if any.
             */
            url?: string | null;
        };
        /** @description Body of `POST /api/v1/discovery/resolve`. */
        ResolveBody: {
            /** @description Feed URL, website URL or directory page URL. */
            input: string;
        };
        /** @description Why resolution failed. */
        ResolveError: {
            /** @description The input. */
            input: string;
            /** @enum {string} */
            kind: "not_a_url";
        } | {
            /** @description Detail. */
            detail: string;
            /** @enum {string} */
            kind: "not_a_feed";
            /**
             * Format: uri
             * @description URL.
             */
            url: string;
        } | {
            /** @enum {string} */
            kind: "no_feed_link_found";
            /** @description Candidate feed URLs that were tried. */
            tried: string[];
            /**
             * Format: uri
             * @description Page URL.
             */
            url: string;
        } | {
            /** @enum {string} */
            kind: "feed_invalid";
            /** @description Reason. */
            reason: string;
            /**
             * Format: uri
             * @description Feed URL.
             */
            url: string;
        } | {
            /** @enum {string} */
            kind: "http_status";
            /**
             * Format: int32
             * @description Status.
             */
            status: number;
            /**
             * Format: uri
             * @description URL.
             */
            url: string;
        } | {
            /** @description Detail. */
            detail: string;
            /** @description Error kind from `uguisu-http`. */
            error_kind: string;
            /** @enum {string} */
            kind: "network";
            /**
             * Format: uri
             * @description URL.
             */
            url: string;
        } | {
            /** @description Detail. */
            detail: string;
            /** @enum {string} */
            kind: "blocked_by_policy";
            /**
             * Format: uri
             * @description URL.
             */
            url: string;
        } | {
            /** @description Detail. */
            detail: string;
            /** @enum {string} */
            kind: "provider_unavailable";
            /** @description Provider. */
            provider: string;
        } | {
            /** @description Suggested search term, when one can be derived. */
            hint?: string | null;
            /** @enum {string} */
            kind: "no_feed_available";
            /** @description Platform name. */
            platform: string;
        } | {
            /** @enum {string} */
            kind: "budget_exceeded";
            /** @description Requests made. */
            requests: number;
        } | {
            /** @enum {string} */
            kind: "cancelled";
        };
        /** @description The `error` object of a resolution failure. */
        ResolveErrorBody: {
            /**
             * @description The failure with its own fields, for a client that renders them — the
             *     CLI lists the URLs a `no_feed_link_found` tried, which `message` cannot
             *     carry.
             */
            detail: components["schemas"]["ResolveError"];
            /** @description Stable kind, from the same place `kind` comes from on every failure. */
            kind: string;
            /** @description Message. */
            message: string;
            /** @description What to try next. */
            suggestion: string;
        };
        /**
         * @description Failure body of `/discovery/resolve`: the standard envelope, plus the two
         *     things only this route has.
         */
        ResolveFailureBody: {
            /** @description The error. */
            error: components["schemas"]["ResolveErrorBody"];
            /** @description Steps taken, in order. */
            provenance: components["schemas"]["ResolutionStep"][];
            /**
             * Format: int32
             * @description Schema version, as on every other body.
             */
            schema: number;
        };
        /** @description A verified feed. */
        ResolvedFeed: {
            /**
             * Format: uri
             * @description Artwork.
             */
            artwork?: string | null;
            /** @description Author. */
            author?: string | null;
            /**
             * Format: uri
             * @description Canonical URL from `atom:link rel="self"` when it was confirmed.
             */
            canonical_url?: string | null;
            /** @description Description (truncated). */
            description?: string | null;
            /** @description Feed syntax. */
            feed_kind: components["schemas"]["FeedKind"];
            /**
             * Format: uri
             * @description The feed URL to subscribe to.
             */
            feed_url: string;
            /** @description The input as given. */
            input: string;
            /** @description Items seen in the feed body. */
            item_count: number;
            /** @description Items with media. */
            items_with_media: number;
            /** @description Language. */
            language?: string | null;
            /** @description `podcast:locked`. */
            locked?: boolean | null;
            /**
             * Format: uri
             * @description `itunes:new-feed-url` when the feed announces a move.
             */
            moved_to?: string | null;
            /**
             * Format: date-time
             * @description Newest item date.
             */
            newest_item?: string | null;
            /** @description `podcast:guid`. */
            podcast_guid?: string | null;
            /** @description Steps taken. */
            provenance: components["schemas"]["ResolutionStep"][];
            /** @description Title. */
            title?: string | null;
            /**
             * Format: date-time
             * @description When the feed was verified.
             */
            verified_at: string;
            /** @description Non-fatal notes. */
            warnings: string[];
            /**
             * Format: uri
             * @description Podcast website.
             */
            website?: string | null;
        };
        /** @description What a restore would do, or did (ADR 0060). */
        RestoreBody: {
            applied: boolean;
            /** @description One line per missing archived file. */
            items: components["schemas"]["RestoreLine"][];
            /**
             * Format: int64
             * @description Files in the folder that were looked at.
             */
            scanned: number;
            source_root: string;
        };
        RestoreLine: {
            /** @description `restore`, `returned`, `source_only`, `taken`, `changed`, `not_found` or `failed`. */
            action: string;
            detail?: string | null;
            episode_id: components["schemas"]["Ulid"];
            podcast_id: components["schemas"]["Ulid"];
            /** @description The file in the folder with the record's bytes, relative to the folder. */
            source_path?: string | null;
            /** @description Where the record says the file belongs, relative to the media root. */
            target_path: string;
        };
        /** @description Body of `POST /api/v1/archive/restore`. */
        RestoreRequest: {
            /** @description Copy the files back. Absent means a dry run. */
            apply?: boolean | null;
            /** @description The folder to look in, on the server's own filesystem. */
            path: string;
            /** @description Only this podcast's missing files. */
            podcast?: string | null;
        };
        /** @description Answer of `POST /api/v1/downloads/retry-failed`. */
        RetryFailedBody: {
            /** Format: int64 */
            requeued: number;
        };
        ScheduleBody: {
            /**
             * @description When the podcast is next due, RFC 3339. `null` means "as soon as
             *     the scheduler looks"; `podcast refresh` is what "now" means.
             */
            at?: string | null;
        };
        /**
         * @description Whether automatic refreshing is running, and since when it is not.
         *
         *     Deliberately not the download queue's control row: a full disk pauses
         *     transfers by itself, and an operator who stops transfers is not asking
         *     the library to go stale.
         *
         *     `paused_reason` is free text rather than an enum, unlike the download
         *     queue's fixed vocabulary: the queue pauses itself for reasons the
         *     engine knows, while this is paused by a person, whose reason is theirs
         *     to write. `scheduler.paused` carries the same string.
         */
        SchedulerControl: {
            /**
             * Format: date-time
             * @description When the maintenance pass last ran. This is what lets a restart
             *     work out when the next one is due instead of running it on boot.
             */
            last_maintenance_at?: string | null;
            /** @description Whether the scheduler starts refreshes. */
            paused: boolean;
            /**
             * Format: date-time
             * @description Since when.
             */
            paused_at?: string | null;
            /** @description Why, when somebody said. */
            paused_reason?: string | null;
            /**
             * Format: date-time
             * @description Last change to this row.
             */
            updated_at: string;
        };
        /**
         * @description What the scheduler is doing, for `scheduler status`, the API and the
         *     status page.
         */
        SchedulerStatus: {
            /**
             * Format: int32
             * @description How many it may run at once.
             */
            concurrency: number;
            /**
             * Format: int32
             * @description Podcasts due for a refresh at this moment.
             */
            due_now: number;
            /**
             * @description Whether automatic refreshing is configured at all
             *     (`UGUISU_FEED_SCHEDULER`).
             */
            enabled: boolean;
            /**
             * Format: int32
             * @description Refreshes the scheduler has running right now.
             */
            inflight: number;
            /**
             * Format: int64
             * @description The default interval between refreshes of one podcast.
             */
            interval_secs: number;
            /**
             * Format: date-time
             * @description When housekeeping last ran.
             */
            last_maintenance_at?: string | null;
            /**
             * Format: date-time
             * @description When the next planned refresh falls due.
             */
            next_due_at?: string | null;
            /** @description Whether the persisted pause is set. */
            paused: boolean;
            /**
             * Format: date-time
             * @description Since when.
             */
            paused_at?: string | null;
            /** @description Why it is paused. */
            paused_reason?: string | null;
            /**
             * @description Whether this process is running the loop. `false` in a one-shot
             *     CLI command, which is not a fault: nothing schedules but `serve`.
             */
            running: boolean;
        };
        /**
         * @description What a credential is allowed to do.
         *
         *     Two values, because the API has exactly two authenticated levels. A third
         *     would have to mean something no route asks about.
         * @enum {string}
         */
        Scope: "read" | "write";
        /** @description The index's state as the database holds it. */
        SearchIndexStatus: {
            /**
             * Format: date-time
             * @description When it was last finished.
             */
            built_at?: string | null;
            /** @description What went wrong, when something did. */
            detail?: string | null;
            /**
             * Format: int64
             * @description Episodes in the index now.
             */
            episodes: number;
            /**
             * Format: int64
             * @description Podcasts in the index now.
             */
            podcasts: number;
            /** @description Ready, building or stale. */
            state: components["schemas"]["IndexState"];
            /**
             * Format: date-time
             * @description Last change to this row.
             */
            updated_at: string;
        };
        /**
         * @description How a search ended. Never a silence that has to be guessed at.
         * @enum {string}
         */
        SearchOutcome: "ok" | "no_results" | "empty_query" | "index_building" | "index_stale";
        /** @description The result of a search (also each streamed snapshot). */
        SearchResponse: {
            /** @description Attribution strings for providers that contributed results. */
            attribution: string[];
            /** @description Whether every provider has answered or timed out. */
            complete: boolean;
            /** @description Outcome. */
            outcome: components["schemas"]["DiscoverySearchOutcome"];
            /** @description Provider outcomes for the query as typed. */
            providers: components["schemas"]["ProviderOutcome"][];
            /** @description The normalized query. */
            query: components["schemas"]["NormalizedQuery"];
            /**
             * @description Looser queries asked because the query as typed found nothing; when
             *     not empty, every result came from them, ranked against the query as typed.
             */
            relaxed?: components["schemas"]["RelaxedQuery"][];
            /** @description Ranked results. */
            results: components["schemas"]["RankedCandidate"][];
            /**
             * Format: int32
             * @description JSON shape version.
             */
            schema: number;
            /** @description Timing. */
            timing: components["schemas"]["SearchTiming"];
        };
        /** @description Everything a search returns. */
        SearchResults: {
            /**
             * Format: int64
             * @description How long it took.
             */
            duration_ms: number;
            /** @description Matching episodes, best first. */
            episodes: components["schemas"]["RankedEpisode"][];
            /** @description The index's state at the time of the search. */
            index: components["schemas"]["SearchIndexStatus"];
            /** @description How it ended. */
            outcome: components["schemas"]["SearchOutcome"];
            /** @description Matching podcasts, best first. */
            podcasts: components["schemas"]["RankedPodcast"][];
            /**
             * @description The terms that were actually searched for, so a user can see what
             *     happened to what they typed.
             */
            terms: string[];
            /** @description Whether the query was cut short (too many or too long terms). */
            truncated: boolean;
        };
        /** @description One component of a hit's score. */
        SearchSignal: {
            /**
             * Format: double
             * @description `weight × value`.
             */
            contribution: number;
            /** @description Stable name. */
            name: string;
            /** @description One line for a person reading `--explain`. */
            note: string;
            /**
             * Format: double
             * @description Observed value, 0..1.
             */
            value: number;
            /**
             * Format: double
             * @description Configured weight.
             */
            weight: number;
        };
        /** @description Timing information. */
        SearchTiming: {
            /**
             * Format: int64
             * @description Time until the first non-empty snapshot.
             */
            first_results_ms?: number | null;
            /**
             * Format: int64
             * @description Total time until this snapshot.
             */
            total_ms: number;
        };
        /** @description Body of `GET /api/v1/auth/session`. */
        SessionBody: {
            /** @description Whether anything on this server needs a credential. */
            auth_required: boolean;
            /** @description Whether *this* request is authenticated. */
            authenticated: boolean;
            /**
             * @description Whether a password has been set, which is the same question asked of
             *     the state rather than of this request.
             */
            credential_set: boolean;
            /** @description The token a mutating request must echo, for a session only. */
            csrf_token?: string | null;
            /** @description `session`, `token`, or absent. */
            principal?: string | null;
            /** @description The operator's name, when there is a credential. */
            username?: string | null;
        };
        SettingBody: {
            /** @description Who is making the change, for the audit column. */
            updated_by?: string | null;
            /** @description The value, in the same syntax the environment variable uses. */
            value: string;
        };
        /**
         * @description Everything `uguisu config validate`, `GET /api/v1/settings` and the
         *     status page need to explain the configuration in force.
         */
        SettingsReport: {
            /** @description Every key Uguisu knows, with its value, origin and provenance. */
            keys: components["schemas"]["KeyDescription"][];
            /** @description Stored values that do not parse, kept and ignored. */
            rejected: components["schemas"]["RejectedSetting"][];
            /** @description Stored values that parse but are not in force. */
            unused: components["schemas"]["UnusedSetting"][];
        };
        /**
         * @description The portable record written next to every archived media file
         *     (`<media file>.json`, ADR 0007 and ADR 0024).
         *
         *     This is what makes the archive self-describing: copy one episode out of
         *     the archive and its sidecar travels with it; lose the database entirely
         *     and `archive reconcile --rebuild` reads the sidecars back.
         *
         *     **A sidecar is metadata, never evidence.** It records what Uguisu knew
         *     when it was written. A rebuild therefore restores a record as
         *     [`VerificationState::Unchecked`] with reason [`reason::REBUILT`]; only a
         *     real verification pass may ever write `verified`. A file that has been
         *     edited since still has a perfectly well-formed sidecar.
         *
         *     Unknown fields are **ignored** rather than rejected, so a sidecar
         *     written by a newer Uguisu still reads here; a newer `schema` is refused
         *     by name, because that is a statement the reader cannot interpret.
         */
        Sidecar: {
            /** @description The file as Uguisu last recorded it. */
            archive: components["schemas"]["SidecarArchive"];
            /** @description The episode. */
            episode: components["schemas"]["SidecarEpisode"];
            /** @description What wrote it, for a human reading the file. */
            generator: string;
            /** @description The podcast. */
            podcast: components["schemas"]["SidecarPodcast"];
            /**
             * Format: int32
             * @description Document schema; [`Sidecar::SCHEMA`] is what this build writes.
             */
            schema: number;
            source?: null | components["schemas"]["SidecarSource"];
            /**
             * Format: date-time
             * @description When it was written.
             */
            written_at: string;
        };
        /**
         * @description The file half of a [`Sidecar`]: what the record says about the bytes on
         *     disk *now*.
         */
        SidecarArchive: {
            /** @description `Content-Type` as served. */
            content_type?: string | null;
            /** @description Hash algorithm. */
            hash_algo: string;
            /** @description Hash of the whole file. */
            hash_value: string;
            /** @description How the record came to exist. */
            origin?: components["schemas"]["ArchiveOrigin"];
            original_tags?: null | components["schemas"]["OriginalTags"];
            /**
             * Format: date-time
             * @description When the artifact was first recorded.
             */
            registered_at: string;
            /**
             * @description Path relative to the media root when the sidecar was written. A
             *     reader must not rely on it: the sidecar's own location is the truth
             *     after a user has moved things.
             */
            relative_path: string;
            /**
             * Format: int64
             * @description Length in bytes.
             */
            size_bytes: number;
            /** @description Container guessed from the first bytes. */
            sniffed_type?: string | null;
            tag_mode?: null | components["schemas"]["TagMode"];
            /** @description Whether Uguisu has written tags, and in which mode. */
            tag_state?: components["schemas"]["TagState"];
            /**
             * Format: date-time
             * @description When tags were last written.
             */
            tagged_at?: string | null;
        };
        /** @description The episode half of a [`Sidecar`]. */
        SidecarEpisode: {
            /**
             * Format: uri
             * @description Episode artwork as the feed declared it; Uguisu does not download it.
             */
            artwork_url?: string | null;
            /** @description Chapter documents as the feed declared them, not downloaded. */
            chapters?: components["schemas"]["ChaptersRef"][];
            /** @description Plain-text description, as stored (already truncated). */
            description_text?: string | null;
            /**
             * Format: int32
             * @description Duration in seconds.
             */
            duration_secs?: number | null;
            /**
             * Format: int64
             * @description Its declared length.
             */
            enclosure_length_bytes?: number | null;
            /** @description Its declared media type. */
            enclosure_type?: string | null;
            /**
             * Format: uri
             * @description The enclosure the bytes came from.
             */
            enclosure_url?: string | null;
            /** @description Feed GUID. */
            guid?: string | null;
            /** @description Episode identifier. */
            id: components["schemas"]["Ulid"];
            /**
             * @description The stable identity key (`guid:…`, `enclosure:…`, …) — what a
             *     rebuild matches on when the identifier itself is unknown.
             */
            identity_key: string;
            /** @description Where that key came from. */
            identity_source?: string | null;
            /**
             * Format: uri
             * @description Episode web page.
             */
            link?: string | null;
            /**
             * Format: int32
             * @description Episode number within the season.
             */
            number?: number | null;
            /**
             * Format: date-time
             * @description Publication time, when the feed carried a usable one.
             */
            published_at?: string | null;
            /**
             * Format: int32
             * @description Season number.
             */
            season?: number | null;
            /** @description Title. */
            title: string;
            /** @description Transcripts as the feed declared them, not downloaded. */
            transcripts?: components["schemas"]["TranscriptRef"][];
        };
        /** @description The podcast half of a [`Sidecar`]. */
        SidecarPodcast: {
            /** @description Author, when the feed named one. */
            author?: string | null;
            /** @description Categories, in feed order. */
            categories?: string[];
            /**
             * Format: uri
             * @description Feed URL of the current source.
             */
            feed_url?: string | null;
            /** @description Podcast identifier. */
            id: components["schemas"]["Ulid"];
            /** @description BCP 47 language tag. */
            language?: string | null;
            /** @description Publisher / owner. */
            publisher?: string | null;
            /** @description Title as Uguisu knows it. */
            title: string;
        };
        /**
         * @description What was received, as opposed to what is on disk now.
         *
         *     After a tag write the two differ: `SidecarArchive::hash_value` follows
         *     the file, this does not. It is immutable **for the life of one
         *     download** — a re-download of the same episode replaces it, because it
         *     then describes different bytes.
         */
        SidecarSource: {
            /** @description Hash algorithm. */
            hash_algo: string;
            /** @description Hash of the bytes as received. */
            hash_value: string;
            /** @description Where they came from: an enclosure URL, or an import's source path. */
            origin_detail?: string | null;
            /**
             * Format: int64
             * @description Length as received.
             */
            size_bytes: number;
        };
        /** @description Where a sidecar went. */
        SidecarWriteBody: {
            episode_id: components["schemas"]["Ulid"];
            path: string;
        };
        /** @description One signal in an explanation. */
        Signal: {
            /**
             * Format: float
             * @description `weight × value`.
             */
            contribution: number;
            /** @description Signal name (stable identifier). */
            name: string;
            /** @description Short human-readable note. */
            note: string;
            /**
             * Format: float
             * @description Observed value in 0..1.
             */
            value: number;
            /**
             * Format: float
             * @description Configured weight.
             */
            weight: number;
        };
        /** @description An episode a bulk enqueue left out. */
        SkippedEpisode: {
            /** @description The episode. */
            episode_id: components["schemas"]["Ulid"];
            /** @description `no_enclosure`, `duplicate_candidate`, `skipped`, `removed_from_feed`. */
            reason: string;
        };
        /** @description A soundbite (`podcast:soundbite`). */
        Soundbite: {
            /**
             * Format: double
             * @description Duration in seconds.
             */
            duration: number;
            /**
             * Format: double
             * @description Start time in seconds.
             */
            start_time: number;
            /** @description Title. */
            title?: string | null;
        };
        /** @description Counts per verification state. */
        StatsBody: {
            by_state: {
                [key: string]: number;
            };
            /** Format: int64 */
            total: number;
        };
        /** @description What the whole service is doing, in one request. */
        Status: {
            downloads: components["schemas"]["DownloadStats"];
            /** Format: int64 */
            podcasts: number;
            scheduler: components["schemas"]["SchedulerStatus"];
            search: components["schemas"]["SearchIndexStatus"];
            /** @description Stored settings that are being kept but not used. */
            settings_problems: number;
            version: string;
        };
        /**
         * @description Kind of a resolution step.
         * @enum {string}
         */
        StepKind: "classify" | "provider_lookup" | "fetch" | "sniff" | "autodiscovery" | "platform_pattern" | "well_known_path" | "validate" | "canonicalize" | "https_upgrade";
        /**
         * @description Which managed fields a tag write may change.
         *
         *     Two modes, and both preserve every tag Uguisu does not manage. ADR 0012
         *     sketched five; the other three (`overwrite`, `existing_wins`, `custom`)
         *     stay unbuilt rather than half-built, which ADR 0026 records.
         * @enum {string}
         */
        TagMode: "fill_missing" | "sync";
        /**
         * @description How far a tag write got.
         *
         *     `Pending` is written **before** the media file is touched, which is the
         *     whole point of the column: after a crash between the atomic replace and
         *     the record update, "Uguisu retagged this" and "someone tampered with
         *     this" are indistinguishable from the bytes alone. A marker a crash
         *     cannot forge tells them apart, and recovery is bounded to the rows that
         *     carry it. Without it, an interrupted retag would look exactly like
         *     corruption for ever.
         * @enum {string}
         */
        TagState: "untagged" | "pending" | "written" | "unsupported" | "failed";
        /** @description The tags a file currently carries. */
        TagsBody: {
            has_cover: boolean;
            values: {
                [key: string]: string;
            };
        };
        /** @description Body of `POST /api/v1/archive/{episode_id}/tags/write`. */
        TagsRequest: {
            /** @description `fill_missing` or `sync`; the configured default when absent. */
            mode?: string | null;
        };
        /** @description What a tag write did. */
        TagsWriteBody: {
            cover_written: boolean;
            detail?: string | null;
            episode_id: components["schemas"]["Ulid"];
            fields: string[];
            hash_value: string;
            mode: components["schemas"]["TagMode"];
            not_embeddable: string[];
            path: string;
            state: string;
        };
        TokenList: {
            tokens: components["schemas"]["ApiToken"][];
        };
        /** @description Body of `POST /api/v1/auth/tokens`. */
        TokenRequest: {
            /** Format: date-time */
            expires_at?: string | null;
            name: string;
            /** @description `read` or `write`; `write` when absent. */
            scope?: string | null;
        };
        /** @description A transcript reference (`podcast:transcript`). */
        TranscriptRef: {
            /** @description Language. */
            language?: string | null;
            /** @description MIME type. */
            mime_type?: string | null;
            /** @description `rel` (e.g. `captions`). */
            rel?: string | null;
            /** @description URL. */
            url: string;
        };
        /** @description A `podcast:txt` record. */
        Txt: {
            /** @description Purpose. */
            purpose?: string | null;
            /** @description Value. */
            value: string;
        };
        /** @description Error returned by engine services. */
        UguisuError: {
            /** @description Database failure. */
            details: string;
            /** @enum {string} */
            kind: "storage";
        } | {
            /** @description Network failure outside a refresh (e.g. while adding a podcast). */
            details: {
                /** @description Detail. */
                detail: string;
                /** @description Classification. */
                kind: components["schemas"]["FetchErrorKind"];
            };
            /** @enum {string} */
            kind: "network";
        } | {
            /** @description The feed could not be used. */
            details: {
                /** @description Detail. */
                detail: string;
                /** @description Classification. */
                kind: components["schemas"]["FetchErrorKind"];
            };
            /** @enum {string} */
            kind: "feed";
        } | {
            /** @description The input could not be resolved to a feed. */
            details: {
                /** @description Detail. */
                detail: string;
                /** @description The input. */
                input: string;
            };
            /** @enum {string} */
            kind: "unresolvable";
        } | {
            /** @description Refused by the network policy. */
            details: string;
            /** @enum {string} */
            kind: "blocked_by_policy";
        } | {
            /** @description An entity does not exist. */
            details: {
                /** @description Entity name. */
                entity: string;
                /** @description Identifier. */
                id: string;
            };
            /** @enum {string} */
            kind: "not_found";
        } | {
            /** @description The operation conflicts with existing state. */
            details: string;
            /** @enum {string} */
            kind: "conflict";
        } | {
            /** @description Invalid input. */
            details: string;
            /** @enum {string} */
            kind: "invalid";
        } | {
            /** @description Configuration problem. */
            details: string;
            /** @enum {string} */
            kind: "config";
        } | {
            /** @description Another process holds the data directory. */
            details: string;
            /** @enum {string} */
            kind: "locked";
        } | {
            /** @description Cancelled or timed out. */
            details: string;
            /** @enum {string} */
            kind: "cancelled";
        } | {
            /** @description A file system operation failed. */
            details: {
                /** @description Detail. */
                detail: string;
                /** @description Path involved. */
                path: string;
            };
            /** @enum {string} */
            kind: "io";
        } | {
            /** @description The media file system has no room for the download. */
            details: {
                /**
                 * Format: int64
                 * @description Bytes available.
                 */
                available: number;
                /**
                 * Format: int64
                 * @description Bytes the operation needs (including the reserve).
                 */
                needed: number;
                /** @description Directory checked. */
                path: string;
            };
            /** @enum {string} */
            kind: "disk_full";
        } | {
            /** @description An archive artifact or a path could not be used. */
            details: {
                /** @description Detail. */
                detail: string;
                /** @description Classification. */
                kind: components["schemas"]["ArchiveErrorKind"];
            };
            /** @enum {string} */
            kind: "archive";
        } | {
            /** @description Unexpected failure. */
            details: string;
            /** @enum {string} */
            kind: "internal";
        };
        /**
         * @description A ULID: 26 characters of Crockford base32, time-ordered.
         * @example 01J8Z9WQ7K2M4N6P8R0T2V4X6Z
         */
        Ulid: string;
        /** @description A stored value that is kept but not used, and the reason. */
        UnusedSetting: {
            /** @description The `UGUISU_*` key, or whatever was stored under that name. */
            key: string;
            /** @description `unknown`, `not persistable` or `pinned by the environment`. */
            reason: string;
            /** @description The value as stored. */
            value: string;
        };
        /**
         * @description What the last verification learned about the file on disk.
         *
         *     This describes the **artifact**, never the transfer: a job's
         *     [`DownloadState`](crate::download::DownloadState) says how the bytes
         *     arrived, this says whether they are still there and still correct.
         * @enum {string}
         */
        VerificationState: "unchecked" | "verified" | "missing" | "invalid";
        /** @description One artifact with the verification that was just run on it. */
        VerifiedFile: {
            /** @description How deep the check looked. */
            depth: components["schemas"]["VerifyDepth"];
            /** @description Detail when the check could not complete or could not decide. */
            detail?: string | null;
            /** @description The record as it now stands. */
            file: components["schemas"]["ArchiveFile"];
            /** @description Why, from `uguisu_core::archive::reason`. */
            reason: string;
            /**
             * @description The record's state after the check: what it found, or what the
             *     record already said when the check could not decide.
             */
            state: components["schemas"]["VerificationState"];
        };
        /** @description Body of the verification endpoints. */
        VerifyBody: {
            /** @description How hard to look (default `light`). */
            depth?: string | null;
            /** @description Only artifacts of this podcast (the bulk endpoint). */
            podcast?: string | null;
            /** @description Only artifacts in this state (the bulk endpoint). */
            state?: string | null;
        };
        /**
         * @description How hard a verification pass looks.
         * @enum {string}
         */
        VerifyDepth: "existence" | "light" | "full";
        /** @description What a verification run found, in one place. */
        VerifySummary: {
            /**
             * Format: int64
             * @description Artifacts checked.
             */
            checked: number;
            /** @description How deep the run looked. */
            depth: components["schemas"]["VerifyDepth"];
            /**
             * Format: int64
             * @description Artifacts whose file is there but wrong.
             */
            invalid: number;
            /**
             * Format: int64
             * @description Artifacts whose file is gone.
             */
            missing: number;
            /**
             * Format: int64
             * @description Artifacts that could not be checked (permissions, I/O).
             */
            unchecked: number;
            /**
             * Format: int64
             * @description Artifacts found intact.
             */
            verified: number;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    archive_list: {
        parameters: {
            query?: {
                /** @description Only artifacts in this verification state. */
                state?: string;
                /** @description Only artifacts of this podcast. */
                podcast?: string;
                /** @description Only artifacts whose feed now points at different audio. */
                source_changed?: boolean;
                /** @description Last artifact id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArchivePage"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Unknown state, cursor or limit */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    import: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ImportRequest"];
            };
        };
        responses: {
            /** @description The plan, or what applying it did */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ImportBody"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The layout could not be matched */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    invalid: {
        parameters: {
            query?: {
                /** @description Only artifacts in this verification state. */
                state?: string;
                /** @description Only artifacts of this podcast. */
                podcast?: string;
                /** @description Only artifacts whose feed now points at different audio. */
                source_changed?: boolean;
                /** @description Last artifact id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArchivePage"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    manifest_status: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ManifestPage"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    manifests_write_all: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Every stale manifest, rewritten */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ManifestWriteBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    manifest_verify: {
        parameters: {
            query?: {
                /** @description Re-read the files instead of comparing against the index. */
                rehash?: boolean;
            };
            header?: never;
            path: {
                /** @description Podcast id */
                podcast_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ManifestCheckBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    manifest_verify_post: {
        parameters: {
            query?: {
                /** @description Re-read the files instead of comparing against the index. */
                rehash?: boolean;
            };
            header?: never;
            path: {
                /** @description Podcast id */
                podcast_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ManifestCheckBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    manifest_write: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                podcast_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ManifestWriteBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    missing: {
        parameters: {
            query?: {
                /** @description Only artifacts in this verification state. */
                state?: string;
                /** @description Only artifacts of this podcast. */
                podcast?: string;
                /** @description Only artifacts whose feed now points at different audio. */
                source_changed?: boolean;
                /** @description Last artifact id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArchivePage"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    orphans: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description What nothing owns under the media directory */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OrphansBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    policies: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PolicyList"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    rebuild: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Optional; a dry run without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["RebuildRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RebuildBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    archive_reconcile: {
        parameters: {
            query?: {
                /** @description Also verify every artifact (size, not hash). */
                deep?: boolean;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArchiveReconcileReport"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    restore: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RestoreRequest"];
            };
        };
        responses: {
            /** @description The plan, or what applying it did */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RestoreBody"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    archive_stats: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StatsBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    verify_all: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Optional; every artifact at `light` depth without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["VerifyBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VerifySummary"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    archive_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArchiveFile"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    media: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The archived file */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description The requested range */
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description Unchanged since the given validator */
            304: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The range cannot be satisfied */
            416: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    path_preview: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Where this episode would be filed, without touching anything */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PathPreview"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    redownload: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Queued again, or a job that already exists */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueOutcome"] & components["schemas"]["Envelope"];
                };
            };
            /** @description A new job, for a file an import or a rebuild archived */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueOutcome"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not an episode id, or an episode without a usable enclosure */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Something is at the record's path or the old download's `.part`, the episode has no archived file, or it is a candidate, skipped or gone from the feed */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    relocate: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        /** @description Optional; moves the file without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["RelocateBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Relocation"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The target is taken or the record disagrees with the disk */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    sidecar_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The sidecar document as it is on disk, with its own `schema` */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Sidecar"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    sidecar_write: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SidecarWriteBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    tags_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagsBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    tags_write: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        /** @description Optional; the configured mode without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["TagsRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagsWriteBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The container carries no tags */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    verify_one: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                episode_id: string;
            };
            cookie?: never;
        };
        /** @description Optional; `light` depth without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["VerifyBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["VerifiedFile"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    auth_exchange: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description A session was opened; its cookie is in `Set-Cookie` */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ExchangeBody"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not a write token, or not from this machine */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    login: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LoginRequest"];
            };
        };
        responses: {
            /** @description The session cookie is in `Set-Cookie` */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LoginBody"] & components["schemas"]["Envelope"];
                };
            };
            /** @description One message for both an unknown user and a wrong password */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Too many failures from this peer; `Retry-After` says how long to wait */
            429: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    logout: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description This session is revoked and the cookie cleared */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
        };
    };
    password: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PasswordRequest"];
            };
        };
        responses: {
            /** @description Set; every other session is revoked */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description A credential exists and this request carries none */
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description `current_password` is missing or wrong */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    session: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description What this request is, and what the server needs */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SessionBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    list_tokens: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Records only: never a secret, never a digest */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TokenList"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    create_token: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TokenRequest"];
            };
        };
        responses: {
            /** @description The only answer that carries the secret */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["NewTokenBody"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    revoke_token: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Token id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Revoked, and kept so it can be shown as revoked */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    revoke_token_post: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Token id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Revoked, and kept so it can be shown as revoked */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    backup_database: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DbBackup"] & components["schemas"]["Envelope"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    check_database: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DbCheck"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    vacuum_database: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DbVacuum"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    providers: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ProvidersResponse"];
                };
            };
        };
    };
    list_records: {
        parameters: {
            query?: {
                /** @description How many to return (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RecordList"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    show_record: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Discovery record id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DiscoveryRecord"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    resolve_get: {
        parameters: {
            query?: {
                /** @description Feed URL, website URL or directory page URL. */
                input?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolvedFeed"];
                };
            };
            /** @description Not a URL, with the steps taken; an empty `input` answers the plain error envelope */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Blocked by the network policy */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Not resolvable, with the steps taken */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Cancelled */
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description The origin or a provider failed */
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description The resolution budget ran out */
            504: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
        };
    };
    resolve_post: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ResolveBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolvedFeed"];
                };
            };
            /** @description Not a URL, with the steps taken; an empty `input` answers the plain error envelope */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Blocked by the network policy */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Not resolvable, with the steps taken */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description Cancelled */
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description The origin or a provider failed */
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
            /** @description The resolution budget ran out */
            504: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveFailureBody"];
                };
            };
        };
    };
    discovery_search: {
        parameters: {
            query?: {
                /** @description Query text. */
                q?: string;
                /** @description Comma-separated provider ids. */
                providers?: string;
                /** @description Result limit. */
                limit?: number;
                /** @description Storefront country. */
                country?: string;
                /** @description Bypass the cache. */
                no_cache?: boolean;
                /** @description Stream snapshots as Server-Sent Events. */
                stream?: boolean;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description One snapshot, or an SSE stream of them when `stream=true` */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SearchResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    download_list: {
        parameters: {
            query?: {
                /** @description Only jobs in this state. */
                state?: string;
                /** @description Only jobs of this podcast. */
                podcast?: string;
                /** @description Last job id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobPage"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Unknown state, cursor or limit */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    enqueue: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["EnqueueBody"];
            };
        };
        responses: {
            /** @description Existing, re-queued or already complete */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueOutcome"] & components["schemas"]["Envelope"];
                };
            };
            /** @description A new job */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueOutcome"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not an episode id, a malformed body, or an episode without a usable enclosure */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The episode cannot be downloaded: a candidate, skipped, gone from the feed, or its file, from an import or a rebuild, is in place */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    pause_all: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ControlBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    download_reconcile: {
        parameters: {
            query?: {
                /** @description Also check every completed target and report unexpected files. */
                deep?: boolean;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReconcileReport"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    resume_all: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ControlBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    retry_failed: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RetryFailedBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    download_stats: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DownloadStats"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    download_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Job id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobDetail"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    download_cancel: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Job id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The job's state does not allow it, or (resume, retry) the episode's file, from an import or a rebuild, is in place */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    download_pause: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Job id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The job's state does not allow it, or (resume, retry) the episode's file, from an import or a rebuild, is in place */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    download_resume: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Job id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The job's state does not allow it, or (resume, retry) the episode's file, from an import or a rebuild, is in place */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    download_retry: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Job id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The job's state does not allow it, or (resume, retry) the episode's file, from an import or a rebuild, is in place */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    list_duplicates: {
        parameters: {
            query?: {
                /** @description Only this podcast's candidates. */
                podcast?: string;
                /** @description Last candidate id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DuplicatePage"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Bad cursor or limit */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Unknown podcast */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    show_episode: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Episode id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EpisodeDetail"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    resolve_duplicate: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Candidate episode id */
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ResolutionRequest"];
            };
        };
        responses: {
            /** @description What the resolution did */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DuplicateResolved"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not an episode id */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Not a candidate, or a merge that would drop a file or join two items of the current feed */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Not a known resolution */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    events: {
        parameters: {
            query?: {
                /** @description Stored events after this id (JSON list instead of a live stream). */
                after?: string;
                /** @description Page size for the list; without `after`, the newest this many. */
                limit?: number;
                /**
                 * @description Comma-separated event kinds to leave out of the live stream
                 *     (`download.progress` is the usual one).
                 */
                exclude?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Stored events when `after` or `limit` is given; otherwise a `text/event-stream` of live events */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EventList"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    inspect: {
        parameters: {
            query?: {
                /** @description Feed URL. */
                url?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Parsed without storing anything */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Inspection"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    feed_status: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Source id */
                source_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["FeedStatus"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    health: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The process can answer */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Health"];
                };
            };
        };
    };
    list_podcasts: {
        parameters: {
            query?: {
                /** @description Only podcasts in this status. */
                status?: string;
                /** @description Only podcasts whose title contains this, case-insensitively (at most 200 characters). */
                q?: string;
                /** @description `title` (default), `added`, `refreshed` or `episodes`. */
                sort?: string;
                /** @description Last podcast id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastPage"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Unknown status, sort or cursor, or `q` too long */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    add_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["AddBody"];
            };
        };
        responses: {
            /** @description Already in the library */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AddOutcome"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Added and refreshed */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AddOutcome"] & components["schemas"]["Envelope"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Nothing resolvable at that input */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    export_opml: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Every podcast as a flat OPML 2.0 file */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/x-opml": string;
                };
            };
        };
    };
    import_opml: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["OpmlImportRequest"];
            };
        };
        responses: {
            /** @description The plan, or what applying it did */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OpmlImport"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not an OPML document, or an unknown policy mode */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    refresh_all: {
        parameters: {
            query?: {
                /** @description Ignore validators and the body fingerprint. */
                force?: boolean;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RefreshAllBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    show_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastDetail"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    remove_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastRemoval"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    archive_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastSchedule"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    artwork_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArtworkBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    artwork_fetch: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        /** @description Optional; conditional without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["ArtworkRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArtworkFetchBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The bytes are not a usable image */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    artwork_image: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The image bytes */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description The requested range */
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description Unchanged since the given validator */
            304: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    enqueue_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        /** @description Optional; `normal` priority without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["EnqueuePodcastBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueSummary"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    list_episodes: {
        parameters: {
            query?: {
                /** @description Last episode id of the previous page. */
                after?: string;
                /** @description Page size (1..=500). */
                limit?: number;
            };
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EpisodePage"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Bad cursor or limit */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    move_feed: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MoveFeedRequest"];
            };
        };
        responses: {
            /** @description What the check found and whether the podcast moved; an unverified feed without force is not moved */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["FeedMove"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Not a podcast id, or not an http(s) URL */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description Refused by the network policy */
            403: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The URL is another podcast's feed, or the feed carries another podcast's podcast:guid */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The URL does not serve a podcast feed */
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    pause_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastSchedule"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    policy_show: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PolicyBody"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    policy_set: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PolicyUpdate"];
            };
        };
        responses: {
            /** @description The policy as it now stands */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PolicyBody"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    policy_set_post: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PolicyUpdate"];
            };
        };
        responses: {
            /** @description The policy as it now stands */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PolicyBody"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    policy_clear_delete: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClearOutcome"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    policy_clear: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClearOutcome"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    refresh_podcast: {
        parameters: {
            query?: {
                /** @description Ignore validators and the body fingerprint. */
                force?: boolean;
            };
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RefreshReport"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The feed could not be fetched or parsed */
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    remove_podcast_post: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastRemoval"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    resume_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PodcastSchedule"] & components["schemas"]["Envelope"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    schedule_podcast: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description Podcast id */
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ScheduleBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SchedulerStatus"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    scheduler: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SchedulerStatus"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    run_maintenance: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["MaintenanceReport"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    scheduler_pause: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** @description Optional; no reason recorded without one */
        requestBody: {
            content: {
                "application/json": components["schemas"]["PauseBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SchedulerControl"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    scheduler_resume: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SchedulerControl"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    run_pass: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The pass was started */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PassBody"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    library_search: {
        parameters: {
            query?: {
                /** @description What to search for. */
                q?: string;
                /** @description Hits of each kind (1..=200). */
                limit?: number;
                /** @description Whether the last word matches as a prefix. */
                prefix?: boolean;
                /** @description Which kinds to search: `podcasts`, `episodes` or both (default). */
                kind?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SearchResults"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    reindex: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReindexReport"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    list_settings: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Every key, with secret values redacted */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SettingsReport"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
    set_setting: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description The `UGUISU_*` name */
                key: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SettingBody"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["KeyDescription"] & components["schemas"]["Envelope"];
                };
            };
            /** @description Unknown key, unstorable key or invalid value */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
            /** @description The environment pins this key */
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    clear_setting: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                /** @description The `UGUISU_*` name */
                key: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Cleared"] & components["schemas"]["Envelope"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    status: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Status"] & components["schemas"]["Envelope"];
                };
            };
        };
    };
}
