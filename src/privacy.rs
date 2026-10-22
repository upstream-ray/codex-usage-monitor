//! Central controls for operations that can invoke a model or install software.
//!
//! Initialize before any credential discovery or update helper runs. Machine
//! policy is read at each side-effect boundary, so user preferences can tighten
//! it but cannot override it. Update helpers also reread persisted preferences.

use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use windows::core::PWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

static PASSIVE: AtomicBool = AtomicBool::new(true);
static UPDATES: AtomicBool = AtomicBool::new(false);
static UPDATE_HELPER: AtomicBool = AtomicBool::new(false);

pub const fn default_passive_monitoring() -> bool {
    true
}

pub const fn default_updates_enabled() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Preferences {
    #[serde(default = "default_passive_monitoring")]
    pub passive_monitoring: bool,
    #[serde(default = "default_updates_enabled")]
    pub updates_enabled: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            passive_monitoring: true,
            updates_enabled: true,
        }
    }
}

impl Preferences {
    const RESTRICTED: Self = Self {
        passive_monitoring: true,
        updates_enabled: false,
    };

    fn with_policy(self, policy: Policy) -> Self {
        Self {
            passive_monitoring: self.passive_monitoring || policy.force_passive_monitoring,
            updates_enabled: self.updates_enabled && !policy.disable_updates,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub force_passive_monitoring: bool,
    pub disable_updates: bool,
    #[serde(skip)]
    pub read_error: bool,
}

impl Policy {
    const UNREADABLE: Self = Self {
        force_passive_monitoring: true,
        disable_updates: true,
        read_error: true,
    };
}

pub fn settings_path() -> PathBuf {
    appdata_path("CodexUsage")
}

pub fn legacy_settings_path() -> PathBuf {
    appdata_path("ClaudeCodeUsageMonitor")
}

fn appdata_path(directory: &str) -> PathBuf {
    // Avoid treating the launch directory as a trusted settings location.
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"))
        .join(directory)
        .join("settings.json")
}

struct KnownFolderString(PWSTR);

impl Drop for KnownFolderString {
    fn drop(&mut self) {
        // SHGetKnownFolderPath allocates with the COM task allocator.
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

pub fn policy_path() -> io::Result<PathBuf> {
    // PROGRAMDATA is a mutable process environment variable. Resolve the
    // machine folder through Windows so a custom launch environment cannot
    // redirect policy lookup to an empty user-controlled directory.
    let allocated = KnownFolderString(
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, HANDLE::default()) }
            .map_err(|error| io::Error::other(format!("Unable to resolve ProgramData: {error}")))?,
    );
    if allocated.0.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned no ProgramData path",
        ));
    }
    // Preserve all Windows path characters without requiring valid Unicode.
    let directory = PathBuf::from(unsafe { std::ffi::OsString::from_wide(allocated.0.as_wide()) });
    if !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned a non-absolute ProgramData path",
        ));
    }
    Ok(directory.join("CodexUsage").join("policy.json"))
}

fn read_preferences(current: &Path, legacy: &Path) -> io::Result<Option<Preferences>> {
    let content = match std::fs::read_to_string(current) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::read_to_string(legacy) {
                Ok(content) => content,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn load_preferences(current: &Path, legacy: &Path, helper: bool) -> Preferences {
    match read_preferences(current, legacy) {
        Ok(Some(preferences)) => preferences,
        Ok(None) if !helper => Preferences::default(),
        Ok(None) | Err(_) => Preferences::RESTRICTED,
    }
}

fn read_policy(path: &Path) -> Policy {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Policy::UNREADABLE),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Policy::default(),
        Err(_) => Policy::UNREADABLE,
    }
}

pub fn policy() -> Policy {
    policy_with(policy_path, read_policy)
}

fn policy_with(
    resolve_path: impl FnOnce() -> io::Result<PathBuf>,
    read: impl FnOnce(&Path) -> Policy,
) -> Policy {
    match resolve_path() {
        Ok(path) => read(&path),
        Err(_) => Policy::UNREADABLE,
    }
}

fn is_update_helper(args: &[String]) -> bool {
    args.get(1)
        .is_some_and(|arg| matches!(arg.as_str(), "--apply-update" | "--check-update-policy"))
}

pub fn initialize(args: &[String]) {
    let helper = is_update_helper(args);
    UPDATE_HELPER.store(helper, Ordering::SeqCst);
    let preferences = load_preferences(&settings_path(), &legacy_settings_path(), helper);
    set_preferences(preferences);
}

pub fn preferences() -> Preferences {
    Preferences {
        passive_monitoring: PASSIVE.load(Ordering::SeqCst),
        updates_enabled: UPDATES.load(Ordering::SeqCst),
    }
}

pub fn set_preferences(preferences: Preferences) {
    PASSIVE.store(preferences.passive_monitoring, Ordering::SeqCst);
    UPDATES.store(preferences.updates_enabled, Ordering::SeqCst);
}

pub fn passive_monitoring() -> bool {
    preferences().with_policy(policy()).passive_monitoring
}

pub fn updates_enabled() -> bool {
    let preferences = if UPDATE_HELPER.load(Ordering::SeqCst) {
        load_preferences(&settings_path(), &legacy_settings_path(), true)
    } else {
        preferences()
    };
    preferences.with_policy(policy()).updates_enabled
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-usage-privacy-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn legacy_preferences_default_to_passive_and_preserve_update_behavior() {
        let legacy: Preferences = serde_json::from_str(r#"{"show_codex":true}"#).unwrap();
        assert_eq!(legacy, Preferences::default());
    }

    #[test]
    fn policy_can_only_tighten_preferences() {
        for passive in [false, true] {
            for updates in [false, true] {
                let requested = Preferences {
                    passive_monitoring: passive,
                    updates_enabled: updates,
                };
                assert_eq!(requested.with_policy(Policy::default()), requested);
                assert_eq!(
                    requested.with_policy(Policy::UNREADABLE),
                    Preferences::RESTRICTED
                );
            }
        }
    }

    #[test]
    fn malformed_or_unreadable_policy_fails_closed_and_reports_error() {
        let dir = TempDir::new();
        let path = dir.0.join("policy.json");
        assert_eq!(read_policy(&path), Policy::default());
        for content in [
            "{",
            r#"{"disable_updates":"false"}"#,
            r#"{"disable_update":true}"#,
        ] {
            std::fs::write(&path, content).unwrap();
            assert_eq!(read_policy(&path), Policy::UNREADABLE);
        }
        // A directory cannot be read as a policy file, including under an admin test user.
        assert_eq!(read_policy(&dir.0), Policy::UNREADABLE);
    }

    #[test]
    fn unresolved_machine_folder_blocks_side_effects_without_fallback_lookup() {
        let requested = Preferences {
            passive_monitoring: false,
            updates_enabled: true,
        };
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
        ] {
            let policy = policy_with(
                || Err(io::Error::new(kind, "synthetic known-folder failure")),
                |_| panic!("failed machine-folder lookup must not read an alternate path"),
            );
            assert!(policy.read_error);
            assert_eq!(requested.with_policy(policy), Preferences::RESTRICTED);
        }
    }

    #[test]
    fn resolved_machine_folder_reads_policy_at_the_exact_resolved_path() {
        let dir = TempDir::new();
        let path = dir.0.join("policy.json");
        std::fs::write(
            &path,
            r#"{"force_passive_monitoring":true,"disable_updates":true}"#,
        )
        .unwrap();
        let policy = policy_with(
            || Ok(path.clone()),
            |resolved| {
                assert_eq!(resolved, path);
                read_policy(resolved)
            },
        );
        assert!(!policy.read_error);
        assert_eq!(
            Preferences {
                passive_monitoring: false,
                updates_enabled: true,
            }
            .with_policy(policy),
            Preferences::RESTRICTED
        );
    }

    #[test]
    fn both_update_helper_commands_require_persisted_update_consent() {
        let dir = TempDir::new();
        let current = dir.0.join("settings.json");
        let legacy = dir.0.join("legacy.json");
        for command in ["--apply-update", "--check-update-policy"] {
            let args = vec!["monitor.exe".into(), command.into()];
            assert_eq!(
                load_preferences(&current, &legacy, is_update_helper(&args)),
                Preferences::RESTRICTED
            );
        }
        // A normal first launch may use the interactive defaults; a helper
        // must never turn missing settings into implicit permission to update.
        assert_eq!(
            load_preferences(&current, &legacy, is_update_helper(&["monitor.exe".into()])),
            Preferences::default()
        );
    }

    #[test]
    fn helper_observes_settings_changes_and_missing_or_corrupt_files_block_updates() {
        let dir = TempDir::new();
        let current = dir.0.join("settings.json");
        let legacy = dir.0.join("legacy.json");
        assert!(!load_preferences(&current, &legacy, true).updates_enabled);
        std::fs::write(&current, r#"{"updates_enabled":true}"#).unwrap();
        assert!(load_preferences(&current, &legacy, true).updates_enabled);
        std::fs::write(&current, r#"{"updates_enabled":false}"#).unwrap();
        assert!(!load_preferences(&current, &legacy, true).updates_enabled);
        std::fs::write(&legacy, r#"{"updates_enabled":true}"#).unwrap();
        std::fs::write(&current, "{").unwrap();
        assert_eq!(
            load_preferences(&current, &legacy, true),
            Preferences::RESTRICTED
        );
        assert_eq!(
            load_preferences(&current, &legacy, false),
            Preferences::RESTRICTED
        );
    }

    #[test]
    fn policy_changes_apply_without_restarting() {
        let dir = TempDir::new();
        let path = dir.0.join("policy.json");
        let requested = Preferences {
            passive_monitoring: false,
            updates_enabled: true,
        };
        assert_eq!(requested.with_policy(read_policy(&path)), requested);
        std::fs::write(
            &path,
            r#"{"force_passive_monitoring":true,"disable_updates":true}"#,
        )
        .unwrap();
        assert_eq!(
            requested.with_policy(read_policy(&path)),
            Preferences::RESTRICTED
        );
    }
}
