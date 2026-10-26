//! Per-provider snapshots of the last successfully fetched quota.
//!
//! Cache files contain only usage values, timestamps, and a credential scope
//! fingerprint. They are intentionally ignored when the active credential is
//! unavailable or does not match the saved scope.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::{UsageData, UsageSection};
use crate::poller;

const CACHE_VERSION: u8 = 1;
const MAX_CACHE_BYTES: usize = 16 * 1024;
const PROVIDER_COUNT: usize = 3;

pub struct CachedUsage {
    pub data: UsageData,
    pub updated_at: SystemTime,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCache {
    version: u8,
    scope: String,
    updated_at_unix_ms: u64,
    data: StoredUsageData,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredUsageData {
    session: StoredUsageSection,
    weekly: StoredUsageSection,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredUsageSection {
    percentage: f64,
    resets_at_unix_secs: Option<u64>,
}

/// Load the last successful quota snapshot for one provider.
///
/// Missing credentials, an unreadable file, or any invalid cache data returns
/// `None` so startup can continue without network access.
pub fn load(id: usize) -> Option<CachedUsage> {
    let scope = poller::cache_scope(id)?;
    load_from(&cache_path(id)?, &scope, SystemTime::now())
}

/// Atomically save the last successful quota snapshot for one provider.
pub fn save(
    id: usize,
    data: &UsageData,
    updated_at: SystemTime,
    expected_scope: &str,
) -> io::Result<()> {
    let current_scope = poller::cache_scope(id);
    save_if_scope_matches(
        &cache_path(id).ok_or_else(invalid_provider)?,
        current_scope.as_deref(),
        expected_scope,
        data,
        updated_at,
    )
}

fn cache_path(id: usize) -> Option<PathBuf> {
    if id >= PROVIDER_COUNT {
        return None;
    }
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("CodexUsage")
            .join(format!("quota-{id}.json")),
    )
}

fn invalid_provider() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "unknown quota provider")
}

fn load_from(path: &Path, scope: &str, now: SystemTime) -> Option<CachedUsage> {
    if !valid_scope(scope) {
        return None;
    }

    let stored = read_stored_cache(path)?;
    let updated_at_unix_ms = valid_snapshot_timestamp(&stored, scope, now)?;
    let updated_at = UNIX_EPOCH.checked_add(Duration::from_millis(updated_at_unix_ms))?;
    let data = from_stored_data(stored.data)?;
    Some(CachedUsage { data, updated_at })
}

fn save_to(path: &Path, scope: &str, data: &UsageData, updated_at: SystemTime) -> io::Result<()> {
    if !valid_scope(scope) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "quota cache scope must be a SHA-256 fingerprint",
        ));
    }
    validate_data(data)?;

    let now = SystemTime::now();
    if updated_at > now {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "quota update time cannot be in the future",
        ));
    }
    let updated_at_unix_ms = updated_at
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "quota update time is invalid"))?
        .as_millis()
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "quota update time is invalid"))?;
    let stored = StoredCache {
        version: CACHE_VERSION,
        scope: scope.to_owned(),
        updated_at_unix_ms,
        data: to_stored_data(data)?,
    };

    let expected_scope = scope.to_owned();
    crate::settings_store::save(path, || {
        let now = SystemTime::now();
        let should_keep_existing = read_stored_cache(path)
            .as_ref()
            .and_then(|existing| valid_snapshot_timestamp(existing, &expected_scope, now))
            .is_some_and(|existing_time| existing_time >= updated_at_unix_ms);
        if should_keep_existing {
            None
        } else {
            Some(stored)
        }
    })
}

fn read_stored_cache(path: &Path) -> Option<StoredCache> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take((MAX_CACHE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_CACHE_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn valid_snapshot_timestamp(stored: &StoredCache, scope: &str, now: SystemTime) -> Option<u64> {
    if stored.version != CACHE_VERSION
        || stored.scope != scope
        || !valid_scope(&stored.scope)
        || !valid_stored_data(&stored.data)
    {
        return None;
    }
    let updated_at = UNIX_EPOCH.checked_add(Duration::from_millis(stored.updated_at_unix_ms))?;
    if updated_at > now {
        return None;
    }
    Some(stored.updated_at_unix_ms)
}

fn save_if_scope_matches(
    path: &Path,
    current_scope: Option<&str>,
    expected_scope: &str,
    data: &UsageData,
    updated_at: SystemTime,
) -> io::Result<()> {
    let current_scope = current_scope.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "cannot save quota without an active credential scope",
        )
    })?;
    if current_scope != expected_scope {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credential scope changed while quota was being fetched",
        ));
    }
    save_to(path, current_scope, data, updated_at)
}

fn valid_scope(scope: &str) -> bool {
    scope.len() == 64 && scope.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_data(data: &UsageData) -> io::Result<()> {
    for section in [&data.session, &data.weekly] {
        if !section.percentage.is_finite() || !(0.0..=100.0).contains(&section.percentage) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "quota percentage must be a finite value between 0 and 100",
            ));
        }
        if let Some(reset) = section.resets_at {
            reset.duration_since(UNIX_EPOCH).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "quota reset time is invalid")
            })?;
        }
    }
    Ok(())
}

fn to_stored_data(data: &UsageData) -> io::Result<StoredUsageData> {
    fn to_stored_section(section: &UsageSection) -> io::Result<StoredUsageSection> {
        let resets_at_unix_secs = section
            .resets_at
            .map(|reset| {
                reset
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "quota reset time is invalid")
                    })
            })
            .transpose()?;
        Ok(StoredUsageSection {
            percentage: section.percentage,
            resets_at_unix_secs,
        })
    }

    Ok(StoredUsageData {
        session: to_stored_section(&data.session)?,
        weekly: to_stored_section(&data.weekly)?,
    })
}

fn from_stored_data(data: StoredUsageData) -> Option<UsageData> {
    fn from_stored_section(section: StoredUsageSection) -> Option<UsageSection> {
        if !section.percentage.is_finite() || !(0.0..=100.0).contains(&section.percentage) {
            return None;
        }
        let resets_at = match section.resets_at_unix_secs {
            Some(secs) => Some(UNIX_EPOCH.checked_add(Duration::from_secs(secs))?),
            None => None,
        };
        Some(UsageSection {
            percentage: section.percentage,
            resets_at,
        })
    }

    Some(UsageData {
        session: from_stored_section(data.session)?,
        weekly: from_stored_section(data.weekly)?,
    })
}

fn valid_stored_data(data: &StoredUsageData) -> bool {
    fn valid_section(section: &StoredUsageSection) -> bool {
        section.percentage.is_finite()
            && (0.0..=100.0).contains(&section.percentage)
            && section
                .resets_at_unix_secs
                .map(|secs| UNIX_EPOCH.checked_add(Duration::from_secs(secs)).is_some())
                .unwrap_or(true)
    }

    valid_section(&data.session) && valid_section(&data.weekly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "codex-usage-quota-cache-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("quota.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scope(byte: char) -> String {
        std::iter::repeat(byte).take(64).collect()
    }

    fn sample_data() -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: 24.5,
                resets_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            },
            weekly: UsageSection {
                percentage: 61.0,
                resets_at: None,
            },
        }
    }

    #[test]
    fn cache_round_trip_preserves_usage_and_timestamps() {
        let directory = TempDir::new();
        let updated_at = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let expected = sample_data();

        save_to(&directory.file(), &scope('a'), &expected, updated_at).unwrap();
        let loaded = load_from(
            &directory.file(),
            &scope('a'),
            UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )
        .expect("valid cache should load");

        assert_eq!(loaded.data.session.percentage, 24.5);
        assert_eq!(loaded.data.session.resets_at, expected.session.resets_at);
        assert_eq!(loaded.data.weekly.percentage, 61.0);
        assert_eq!(loaded.data.weekly.resets_at, None);
        assert_eq!(loaded.updated_at, updated_at);
    }

    #[test]
    fn older_or_equal_timestamp_cannot_replace_a_newer_snapshot() {
        let directory = TempDir::new();
        let path = directory.file();
        let scope = scope('a');
        let newer_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut newer_data = sample_data();
        newer_data.session.percentage = 21.0;
        save_to(&path, &scope, &newer_data, newer_time).unwrap();

        let mut stale_data = sample_data();
        stale_data.session.percentage = 88.0;
        save_to(
            &path,
            &scope,
            &stale_data,
            newer_time - Duration::from_secs(1),
        )
        .unwrap();
        save_to(&path, &scope, &stale_data, newer_time).unwrap();

        let loaded = load_from(
            &path,
            &scope,
            UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )
        .expect("newer snapshot should remain readable");
        assert_eq!(loaded.data.session.percentage, 21.0);
        assert_eq!(loaded.updated_at, newer_time);
    }

    #[test]
    fn invalid_or_future_cache_data_is_ignored() {
        let directory = TempDir::new();
        let path = directory.file();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let good = StoredCache {
            version: CACHE_VERSION,
            scope: scope('a'),
            updated_at_unix_ms: 1_799_999_999_000,
            data: to_stored_data(&sample_data()).unwrap(),
        };

        std::fs::write(&path, b"{broken").unwrap();
        assert!(load_from(&path, &scope('a'), now).is_none());

        let mut future = good;
        future.updated_at_unix_ms = 1_800_000_001_000;
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(load_from(&path, &scope('a'), now).is_none());

        future.updated_at_unix_ms = 1_799_999_999_000;
        future.data.session.percentage = 100.1;
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(load_from(&path, &scope('a'), now).is_none());

        std::fs::write(&path, vec![b'x'; MAX_CACHE_BYTES + 1]).unwrap();
        assert!(load_from(&path, &scope('a'), now).is_none());
    }

    #[test]
    fn cache_is_not_reused_for_a_different_credential_scope() {
        let directory = TempDir::new();
        let path = directory.file();
        let now = SystemTime::now();
        save_to(
            &path,
            &scope('a'),
            &sample_data(),
            now - Duration::from_secs(1),
        )
        .unwrap();

        assert!(load_from(&path, &scope('b'), now).is_none());
    }

    #[test]
    fn stale_request_scope_does_not_replace_the_current_account_snapshot() {
        let directory = TempDir::new();
        let path = directory.file();
        let now = SystemTime::now();
        save_to(
            &path,
            &scope('b'),
            &sample_data(),
            now - Duration::from_secs(1),
        )
        .unwrap();
        let original = std::fs::read(&path).unwrap();

        assert!(
            save_if_scope_matches(&path, Some(&scope('b')), &scope('a'), &sample_data(), now,)
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn invalid_save_does_not_replace_a_valid_snapshot() {
        let directory = TempDir::new();
        let path = directory.file();
        let now = SystemTime::now();
        save_to(
            &path,
            &scope('a'),
            &sample_data(),
            now - Duration::from_secs(1),
        )
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut invalid = sample_data();
        invalid.session.percentage = f64::NAN;

        assert!(save_to(&path, &scope('a'), &invalid, now).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn failed_atomic_replacement_keeps_the_previous_snapshot() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = TempDir::new();
        let path = directory.file();
        let now = SystemTime::now();
        save_to(
            &path,
            &scope('a'),
            &sample_data(),
            now - Duration::from_secs(1),
        )
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();

        assert!(save_to(&path, &scope('a'), &sample_data(), now).is_err());
        drop(locked);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let loaded = load_from(&path, &scope('a'), SystemTime::now())
            .expect("previous snapshot should remain readable");
        assert_eq!(loaded.data.session.percentage, 24.5);
    }
}
