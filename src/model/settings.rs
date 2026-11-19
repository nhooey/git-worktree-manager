//! A repository's own settings, from `worktreeManager.*` in its git config.

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::names::SettingKey;
use crate::time::parse_duration;

string_enum! {
    /// What a sweep the hook starts (`--unattended`) does.
    pub enum Unattended {
        /// Plans and writes the report, and changes nothing else.
        Report => "report",
        /// Acts.
        Sweep => "sweep",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// `worktreeManager.unattended`, default `report`.
    pub unattended: Unattended,
    /// `worktreeManager.idle`, default `1h`: how long a worktree's `HEAD`,
    /// `logs/HEAD` and index must be unchanged to pass `idle`.
    pub idle: SignedDuration,
    /// `worktreeManager.lockReport`, default `1d`: how long a landed
    /// worktree may stay locked before it needs attention.
    pub lock_report: SignedDuration,
    /// `worktreeManager.localOnlyReport`, default `7d`: how old a
    /// `local-only` branch's tip may be before it needs attention.
    pub local_only_report: SignedDuration,
    /// `worktreeManager.disposable`, default `preview/*`: branch patterns,
    /// one per value, in git's glob syntax.
    pub disposable: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unattended: Unattended::Report,
            idle: SignedDuration::from_hours(1),
            lock_report: SignedDuration::from_hours(24),
            local_only_report: SignedDuration::from_hours(24 * 7),
            disposable: vec!["preview/*".to_owned()],
        }
    }
}

impl Settings {
    /// The settings from the values git config holds for each key, in the
    /// order git reads them. The last value wins, but `disposable` takes
    /// every non-empty one. A value that can't be read leaves the default.
    pub fn from_config(values: &[(SettingKey, String)]) -> Self {
        let defaults = Self::default();
        let last = |key: SettingKey| {
            values
                .iter()
                .rev()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.trim())
        };
        let duration = |key: SettingKey, default: SignedDuration| {
            last(key).and_then(parse_duration).unwrap_or(default)
        };
        let disposable: Vec<&(SettingKey, String)> = values
            .iter()
            .filter(|(k, _)| *k == SettingKey::Disposable)
            .collect();
        Self {
            unattended: last(SettingKey::Unattended)
                .and_then(|v| Unattended::parse(&v.to_ascii_lowercase()))
                .unwrap_or(defaults.unattended),
            idle: duration(SettingKey::Idle, defaults.idle),
            lock_report: duration(SettingKey::LockReport, defaults.lock_report),
            local_only_report: duration(SettingKey::LocalOnlyReport, defaults.local_only_report),
            disposable: if disposable.is_empty() {
                defaults.disposable
            } else {
                disposable
                    .iter()
                    .map(|(_, v)| v.trim())
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(key: SettingKey, text: &str) -> (SettingKey, String) {
        (key, text.to_owned())
    }

    #[test]
    fn test_defaults_without_config() {
        assert_eq!(Settings::from_config(&[]), Settings::default());
    }

    #[test]
    fn test_settings_from_git_config() {
        let settings = Settings::from_config(&[
            value(SettingKey::Idle, "1h"),
            value(SettingKey::Idle, "0s"),
            value(SettingKey::Unattended, "Sweep"),
            value(SettingKey::LockReport, "junk"),
            value(SettingKey::Disposable, "tmp/*"),
            value(SettingKey::Disposable, "scratch-*"),
        ]);
        assert_eq!(
            settings,
            Settings {
                unattended: Unattended::Sweep,
                idle: SignedDuration::ZERO,
                lock_report: SignedDuration::from_hours(24),
                local_only_report: SignedDuration::from_hours(168),
                disposable: vec!["tmp/*".to_owned(), "scratch-*".to_owned()],
            }
        );
    }

    #[test]
    fn test_an_empty_disposable_value_clears_the_default() {
        assert!(
            Settings::from_config(&[value(SettingKey::Disposable, "")])
                .disposable
                .is_empty()
        );
    }
}
