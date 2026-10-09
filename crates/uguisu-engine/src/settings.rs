//! The effective configuration, and the settings that outlive a process
//! (ADR 0028).
//!
//! Two layers under the command line: the `settings` table and the
//! environment, in that order — an environment variable an operator put
//! in a unit file wins, and a stored value it shadows is reported as
//! *pinned* rather than silently ignored. Writing such a key is refused,
//! because a write that would have no effect is worse than no write.
//!
//! There is one parser: [`Config::with_stored`] re-assembles the whole
//! configuration for every change, so a stored value is validated exactly
//! the way an environment variable is, including the cross-field rules
//! (`per_host <= global`) that no per-key check could express.
//!
//! **A stored value never prevents a start, and is never deleted to
//! achieve that.** A value that no longer parses is *quarantined*: the row
//! stays exactly as it was written, the engine opens without it, a
//! `settings.rejected` event records it, and the API and CLI show it with
//! the parser's message. Refusing to start would brick the daemon whose
//! API is the only way to fix the row; deleting the row would throw away
//! the only copy of what the user meant.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uguisu_core::config::{Config, KeyDescription, Origin};
use uguisu_core::settings::{self, Setting};
use uguisu_core::{Event, EventKind, UguisuError};
use uguisu_storage::{events, settings as settings_repo};

use crate::Engine;

/// A stored setting the engine is not applying, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RejectedSetting {
    /// The `UGUISU_*` key.
    pub key: String,
    /// The value as stored, unchanged.
    pub value: String,
    /// What the parser said about it.
    pub message: String,
}

/// A stored value that is kept but not used, and the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UnusedSetting {
    /// The `UGUISU_*` key, or whatever was stored under that name.
    pub key: String,
    /// The value as stored.
    pub value: String,
    /// `unknown`, `not persistable` or `pinned by the environment`.
    pub reason: String,
}

/// Everything `uguisu config validate`, `GET /api/v1/settings` and the
/// status page need to explain the configuration in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SettingsReport {
    /// Every key Uguisu knows, with its value, origin and provenance.
    pub keys: Vec<KeyDescription>,
    /// Stored values that do not parse, kept and ignored.
    pub rejected: Vec<RejectedSetting>,
    /// Stored values that parse but are not in force.
    pub unused: Vec<UnusedSetting>,
}

impl SettingsReport {
    /// Whether anything about the stored layer needs a person's attention.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        !self.rejected.is_empty() || !self.unused.is_empty()
    }
}

/// The configuration in force plus the stored rows it could not use.
#[derive(Debug, Clone)]
pub(crate) struct Effective {
    pub(crate) config: std::sync::Arc<Config>,
    pub(crate) rejected: Vec<RejectedSetting>,
}

/// Applies stored settings to `base`, one key at a time.
///
/// One key at a time rather than all at once, because that is what makes
/// quarantine precise: each step validates the *whole* merged
/// configuration, so the key that breaks it is the key that gets named. A
/// single merge could only report that something, somewhere, is wrong.
///
/// Keys the engine will not read from the database — secrets, the
/// directories, the SSRF allowlist — are still applied here, and still
/// inert: [`Config::from_layers`] refuses them, and
/// [`Config::unused_stored`] is what reports them. The same holds for a
/// key nobody knows and for one the environment pins. Only a value that
/// does not parse is dropped, and only from the snapshot — never from the
/// table.
pub(crate) fn merge_stored(base: &Config, stored: &[Setting]) -> Effective {
    let mut config = base.clone();
    let mut rejected = Vec::new();
    for row in stored {
        match config.with_stored(&row.key, Some(&row.value)) {
            Ok(next) => config = next,
            Err(e) => {
                tracing::warn!(key = %row.key, message = %e.message, "stored setting ignored");
                rejected.push(RejectedSetting {
                    value: uguisu_core::config::redacted(&row.key, &row.value),
                    key: row.key.clone(),
                    message: e.message,
                });
            }
        }
    }
    Effective {
        config: std::sync::Arc::new(config),
        rejected,
    }
}

impl Engine {
    /// Re-reads the `settings` table, swaps in the configuration it
    /// produces, and records anything it had to quarantine. Called after
    /// every write.
    pub(crate) async fn reload_settings(&self) -> Result<(), UguisuError> {
        let stored = {
            let mut reader = self.storage().reader().await?;
            settings_repo::list(&mut reader).await?
        };
        let base = self.inner.base_config.clone();
        self.install_settings(merge_stored(&base, &stored)).await
    }

    /// Makes `effective` the configuration in force and records its
    /// rejections.
    ///
    /// A rejection is recorded once per load rather than once ever: a row
    /// Uguisu ignored on this start is a fact about this start, and an
    /// operator reading the log after a restart should find it there
    /// rather than having to know that it was reported months ago.
    pub(crate) async fn install_settings(&self, effective: Effective) -> Result<(), UguisuError> {
        let events: Vec<Event> = effective
            .rejected
            .iter()
            .map(|r| {
                Event::now(
                    None,
                    None,
                    EventKind::SettingsRejected {
                        key: r.key.clone(),
                        message: r.message.clone(),
                    },
                )
            })
            .collect();
        *self
            .inner
            .effective
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effective;
        self.record(&events).await
    }

    /// Appends events and publishes them, in their own transaction.
    ///
    /// Settings changes have no other state to be atomic with — the row
    /// is already written, or was already read — so this is the whole
    /// commit, and the bus still only ever sees what reached the disk.
    async fn record(&self, events: &[Event]) -> Result<(), UguisuError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut tx = self.storage().begin().await?;
        events::insert_all(&mut tx, events).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(events);
        Ok(())
    }

    /// Stored settings the engine is keeping but not using.
    #[must_use]
    pub fn rejected_settings(&self) -> Vec<RejectedSetting> {
        self.inner
            .effective
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .rejected
            .clone()
    }

    /// The configuration in force, with its provenance.
    #[must_use]
    pub fn settings_report(&self) -> SettingsReport {
        let config = self.config();
        SettingsReport {
            keys: config.describe(),
            rejected: self.rejected_settings(),
            unused: config
                .unused_stored()
                .into_iter()
                .map(|(key, value, reason)| UnusedSetting {
                    key,
                    value,
                    reason: reason.to_owned(),
                })
                .collect(),
        }
    }

    /// Writes a stored setting, after checking that it would take effect
    /// and that the result is a configuration Uguisu can run.
    ///
    /// Refuses, without writing anything:
    /// - a key Uguisu does not know (a typo is not a feature request);
    /// - a key that may never be stored — the two secrets and where they
    ///   are sent, the two directories and the SSRF allowlist
    ///   (`docs/CONFIGURATION.md`);
    /// - a key the environment sets, because the write would be stored
    ///   and then ignored. The remedy is to unset the variable, so there
    ///   is no `--force`;
    /// - a value that does not parse, or that makes the whole
    ///   configuration invalid, quoting the message an environment
    ///   variable would have produced.
    pub async fn set_setting(
        &self,
        key: &str,
        value: &str,
        updated_by: Option<&str>,
    ) -> Result<KeyDescription, UguisuError> {
        let spec = check_writable(&self.config(), key)?;
        let candidate = self
            .config()
            .with_stored(key, Some(value))
            .map_err(|e| UguisuError::Invalid(format!("{}: {}", e.key, e.message)))?;
        // Parsed from the merged view, so what is written is what would be
        // read back — a trailing space or a `TRUE` is stored as given and
        // means what the parser said it means.
        drop(candidate);
        let mut tx = self.storage().begin().await?;
        settings_repo::set(&mut tx, key, value, updated_by, OffsetDateTime::now_utc()).await?;
        let changed = vec![Event::now(
            None,
            None,
            EventKind::SettingsChanged {
                key: key.to_owned(),
                removed: false,
                restart_required: !spec.live,
            },
        )];
        events::insert_all(&mut tx, &changed).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(&changed);
        self.reload_settings().await?;
        tracing::info!(key, restart_required = !spec.live, "setting stored");
        self.described(key)
    }

    /// Removes a stored setting, whatever state it is in.
    ///
    /// This is the only thing that clears a quarantined row, and it is the
    /// one operation that does not care whether the key parses, is pinned
    /// or is even known: "forget what I stored" must always work.
    /// `false` when there was nothing stored.
    pub async fn unset_setting(
        &self,
        key: &str,
        _updated_by: Option<&str>,
    ) -> Result<bool, UguisuError> {
        let mut tx = self.storage().begin().await?;
        let existed = settings_repo::delete(&mut tx, key).await?;
        if !existed {
            tx.rollback()
                .await
                .map_err(uguisu_storage::StorageError::from)?;
            return Ok(false);
        }
        let restart_required = settings::spec(key).is_none_or(|s| !s.live);
        let changed = vec![Event::now(
            None,
            None,
            EventKind::SettingsChanged {
                key: key.to_owned(),
                removed: true,
                restart_required,
            },
        )];
        events::insert_all(&mut tx, &changed).await?;
        tx.commit()
            .await
            .map_err(uguisu_storage::StorageError::from)?;
        self.bus().publish(&changed);
        self.reload_settings().await?;
        tracing::info!(key, "setting cleared");
        Ok(true)
    }

    /// One key as [`SettingsReport`] describes it.
    fn described(&self, key: &str) -> Result<KeyDescription, UguisuError> {
        self.config()
            .describe()
            .into_iter()
            .find(|d| d.key == key)
            .ok_or_else(|| UguisuError::NotFound {
                entity: "setting".to_owned(),
                id: key.to_owned(),
            })
    }
}

/// Whether `key` may be written at all, and its specification if so.
fn check_writable(
    config: &Config,
    key: &str,
) -> Result<&'static settings::SettingSpec, UguisuError> {
    let Some(spec) = settings::spec(key) else {
        return Err(UguisuError::Invalid(format!("unknown setting `{key}`")));
    };
    if !spec.persistable {
        return Err(UguisuError::Invalid(format!(
            "{key} cannot be stored in the database; set it in the environment"
        )));
    }
    if config.is_pinned(key) {
        return Err(UguisuError::Conflict(format!(
            "{key} is set in the environment ({}) and a stored value would be ignored; \
             unset the environment variable to manage it here",
            Origin::Env
        )));
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::collections::BTreeMap;
    use uguisu_core::config::Layers;

    fn stored(rows: &[(&str, &str)]) -> Vec<Setting> {
        rows.iter()
            .map(|(k, v)| Setting {
                key: (*k).to_owned(),
                value: (*v).to_owned(),
                updated_at: OffsetDateTime::now_utc(),
                updated_by: None,
            })
            .collect()
    }

    fn base(env: &[(&str, &str)]) -> Config {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_layers(&Layers::new(BTreeMap::new(), move |k: &str| {
            env.get(k).cloned()
        }))
        .unwrap()
    }

    #[test]
    fn an_unparsable_value_is_quarantined() {
        let effective = merge_stored(
            &base(&[]),
            &stored(&[
                ("UGUISU_FEED_REFRESH_CONCURRENCY", "16"),
                ("UGUISU_ARCHIVE_MAX_BACKLOG", "not a number"),
                ("UGUISU_ARCHIVE_AUTO_DOWNLOAD", "true"),
            ]),
        );
        assert_eq!(effective.config.feed.refresh_concurrency, 16);
        assert!(effective.config.archive.auto_download);
        assert_eq!(effective.rejected.len(), 1);
        assert_eq!(effective.rejected[0].key, "UGUISU_ARCHIVE_MAX_BACKLOG");
        assert_eq!(
            effective.rejected[0].value, "not a number",
            "the value is reported as written, so it can be corrected"
        );
        assert!(effective.rejected[0].message.contains("integer"));
    }

    #[test]
    fn a_cross_field_failure_names_it() {
        // Each value is fine alone; together they are not. The key that
        // broke the merge is the one reported, which is only possible
        // because the merge happens one key at a time.
        let effective = merge_stored(
            &base(&[]),
            &stored(&[
                ("UGUISU_DOWNLOAD_GLOBAL_CONCURRENCY", "2"),
                ("UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY", "4"),
            ]),
        );
        assert_eq!(effective.config.download.global_concurrency, 2);
        assert_eq!(effective.config.download.per_host_concurrency, 1);
        assert_eq!(
            effective.rejected[0].key,
            "UGUISU_DOWNLOAD_PER_HOST_CONCURRENCY"
        );
    }

    #[test]
    fn the_environment_wins_and_says_so() {
        let effective = merge_stored(
            &base(&[("UGUISU_FEED_REFRESH_CONCURRENCY", "2")]),
            &stored(&[("UGUISU_FEED_REFRESH_CONCURRENCY", "16")]),
        );
        assert_eq!(effective.config.feed.refresh_concurrency, 2);
        assert!(effective.rejected.is_empty(), "shadowed is not rejected");
        let unused = effective.config.unused_stored();
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].2, "pinned by the environment");
        assert!(
            effective
                .config
                .is_pinned("UGUISU_FEED_REFRESH_CONCURRENCY")
        );
    }

    #[test]
    fn unknown_keys_stay_inert_and_reported() {
        let effective = merge_stored(
            &base(&[]),
            &stored(&[
                ("UGUISU_PODCASTINDEX_KEY", "secret"),
                ("UGUISU_TYPO_NOBODY_READS", "1"),
            ]),
        );
        assert!(effective.rejected.is_empty());
        assert!(effective.config.discovery.podcastindex.key.is_none());
        let reasons: Vec<&str> = effective
            .config
            .unused_stored()
            .iter()
            .map(|(_, _, reason)| *reason)
            .collect();
        // Reported by key, in the order the table holds them.
        assert_eq!(reasons, vec!["not persistable", "unknown"]);
    }

    #[test]
    fn a_write_is_refused_before_it_happens() {
        let config = base(&[("UGUISU_FEED_REFRESH_CONCURRENCY", "2")]);
        let err = check_writable(&config, "UGUISU_NOT_A_KEY").unwrap_err();
        assert_eq!(err.kind(), "invalid");
        let err = check_writable(&config, "UGUISU_PODCASTINDEX_SECRET").unwrap_err();
        assert_eq!(err.kind(), "invalid");
        let err = check_writable(&config, "UGUISU_HTTP_ALLOW_PRIVATE_HOSTS").unwrap_err();
        assert_eq!(err.kind(), "invalid");
        let err = check_writable(&config, "UGUISU_FEED_REFRESH_CONCURRENCY").unwrap_err();
        assert_eq!(err.kind(), "conflict", "the environment pins it");
        assert!(check_writable(&config, "UGUISU_ARCHIVE_MAX_BACKLOG").is_ok());
    }
}
