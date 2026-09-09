use crate::{error::AppResult, models::AppSettings};
use tauri::AppHandle;
#[cfg(windows)]
use tauri_plugin_autostart::ManagerExt;

pub const AUTOSTART_ARG: &str = "--autostart";

pub fn show_initial_window(settings: &AppSettings, args: &[String]) -> bool {
    !(settings.silent_startup && args.iter().any(|arg| arg == AUTOSTART_ARG))
}

// Migrate only our own existing login entry. Never enable a missing/externally
// disabled entry at startup, or delete an entry pointing at a different copy.
#[cfg(windows)]
pub fn migrate_login_entry(app: &AppHandle, settings: &AppSettings) -> AppResult<()> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
        RegKey,
    };
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_READ | KEY_SET_VALUE,
    ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let executable = std::env::current_exe()?;
    let legacy: Option<String> = key.get_value("RouteDeck").ok();
    if legacy
        .as_deref()
        .is_some_and(|value| owns_legacy_entry(value, &executable))
    {
        let existing: Option<String> = key.get_value("Serylane").ok();
        if existing
            .as_deref()
            .is_some_and(|command| !owns_legacy_entry(command, &executable))
        {
            return Ok(());
        }
        if settings.launch_at_login {
            let approvals = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
                KEY_READ | KEY_SET_VALUE,
            );
            match approvals {
                Ok(approvals) => match approvals.get_raw_value("RouteDeck") {
                    Ok(value) => {
                        approvals.set_raw_value("Serylane", &value)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            app.autolaunch()
                .enable()
                .map_err(|e| crate::error::AppError::Platform(e.to_string()))?;
        }
        key.delete_value("RouteDeck")?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn migrate_login_entry(_app: &AppHandle, _settings: &AppSettings) -> AppResult<()> {
    Ok(())
}

#[cfg(any(windows, test))]
fn owns_legacy_entry(command: &str, executable: &std::path::Path) -> bool {
    let Some(directory) = executable.parent() else {
        return false;
    };
    ["routedeck.exe", "serylane.exe"].iter().any(|name| {
        let expected = format!("\"{}\"", directory.join(name).display());
        command.eq_ignore_ascii_case(&expected)
            || command.eq_ignore_ascii_case(&format!("{expected} {AUTOSTART_ARG}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silent_login_does_not_hide_manual_launch() {
        let mut settings = AppSettings::default();
        assert!(show_initial_window(&settings, &[AUTOSTART_ARG.into()]));
        settings.silent_startup = true;
        assert!(!show_initial_window(&settings, &[AUTOSTART_ARG.into()]));
        assert!(show_initial_window(&settings, &[]));
        assert!(show_initial_window(
            &settings,
            &["--autostart-unknown".into()]
        ));
    }
    #[test]
    fn migration_matches_exact_owned_executable_not_prefixes_or_other_copies() {
        let directory = std::path::Path::new("fixture").join("app");
        let current = directory.join("serylane.exe");
        let old = format!("\"{}\"", directory.join("routedeck.exe").display());
        assert!(owns_legacy_entry(&old, &current));
        assert!(owns_legacy_entry(&format!("{old} --autostart"), &current));
        assert!(!owns_legacy_entry(&format!("{old} --unexpected"), &current));
        assert!(!owns_legacy_entry("\"other/routedeck.exe\"", &current));
    }
}
