use crate::{error::AppResult, models::AppSettings};
use tauri::AppHandle;

pub const AUTOSTART_ARG: &str = "--autostart";

#[cfg(any(windows, test))]
pub fn installer_cleanup_requested(args: &[String]) -> bool {
    args == ["--installer-remove-login"]
}

// The MSI uninstall hook runs this before removing our executable. No Tauri
// window, proxy, user settings or other installation's login entries are touched.
#[cfg(windows)]
pub fn remove_owned_login_entries() -> std::io::Result<()> {
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
        Err(error) => return Err(error),
    };
    let executable = std::env::current_exe()?;
    for name in ["Serylane", "RouteDeck"] {
        let command: Option<String> = key.get_value(name).ok();
        if command
            .as_deref()
            .is_some_and(|command| owns_legacy_entry(command, &executable))
        {
            match key.delete_value(name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub fn show_initial_window(settings: &AppSettings, args: &[String]) -> bool {
    !(settings.silent_startup && args.iter().any(|arg| arg == AUTOSTART_ARG))
}

// Migrate only our own existing login entry. Never enable a missing/externally
// disabled entry at startup, or delete an entry pointing at a different copy.
#[cfg(windows)]
pub fn migrate_login_entry(_app: &AppHandle, settings: &AppSettings) -> AppResult<()> {
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
    let existing: Option<String> = key.get_value("Serylane").ok();
    if existing
        .as_deref()
        .is_some_and(|command| !owns_legacy_entry(command, &executable))
    {
        return Ok(());
    }
    // auto-launch 0.5 wrote unquoted paths. Normalize only an existing owned
    // entry; do not create or enable a missing OS registration here.
    if existing.is_some() {
        key.set_value("Serylane", &quoted_login_command(&executable))?;
    }
    let legacy: Option<String> = key.get_value("RouteDeck").ok();
    if legacy
        .as_deref()
        .is_some_and(|value| owns_legacy_entry(value, &executable))
    {
        if settings.launch_at_login {
            let approvals = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
                KEY_READ | KEY_SET_VALUE,
            );
            match approvals {
                Ok(approvals) => {
                    // Keep an existing new-name approval untouched. Otherwise
                    // copy the old disabled/enabled bytes without calling
                    // auto-launch.enable(), which would reset them to enabled.
                    match approvals.get_raw_value("Serylane") {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            match approvals.get_raw_value("RouteDeck") {
                                Ok(value) => approvals.set_raw_value("Serylane", &value)?,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            key.set_value("Serylane", &quoted_login_command(&executable))?;
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
fn quoted_login_command(executable: &std::path::Path) -> String {
    format!("\"{}\" {AUTOSTART_ARG}", executable.display())
}

#[cfg(any(windows, test))]
fn owns_legacy_entry(command: &str, executable: &std::path::Path) -> bool {
    let Some(directory) = executable.parent() else {
        return false;
    };
    ["routedeck.exe", "serylane.exe"].iter().any(|name| {
        let path = directory.join(name).display().to_string();
        // Exact serializations used by our current and legacy autostart
        // dependencies, including the old no-argument trailing space. Never
        // accept prefixes, arbitrary arguments or another installation.
        [
            format!("\"{path}\""),
            format!("\"{path}\" {AUTOSTART_ARG}"),
            path.clone(),
            format!("{path} "),
            format!("{path} {AUTOSTART_ARG}"),
        ]
        .iter()
        .any(|expected| command.eq_ignore_ascii_case(expected))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uninstall_command_is_exact_and_never_matches_normal_launch() {
        assert!(installer_cleanup_requested(&[
            "--installer-remove-login".into()
        ]));
        assert!(!installer_cleanup_requested(&[]));
        assert!(!installer_cleanup_requested(&[AUTOSTART_ARG.into()]));
        assert!(!installer_cleanup_requested(&[
            "--installer-remove-login-other".into()
        ]));
        assert!(!installer_cleanup_requested(&[
            "--installer-remove-login".into(),
            "extra".into()
        ]));
    }
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
        let current_command = format!("\"{}\" {AUTOSTART_ARG}", current.display());
        assert!(owns_legacy_entry(&current_command, &current));
        assert!(!owns_legacy_entry(
            &format!("{current_command} extra"),
            &current
        ));
    }
    #[test]
    fn old_unquoted_login_paths_with_spaces_are_normalized_without_extra_arguments() {
        let directory = std::path::Path::new("fixture").join("Program Files");
        let current = directory.join("serylane.exe");
        let old = directory.join("routedeck.exe").display().to_string();
        for command in [&old, &format!("{old} "), &format!("{old} {AUTOSTART_ARG}")] {
            assert!(owns_legacy_entry(command, &current));
        }
        assert_eq!(
            quoted_login_command(&current),
            format!("\"{}\" --autostart", current.display())
        );
        assert!(!owns_legacy_entry(
            &format!("{old}.other --autostart"),
            &current
        ));
        assert!(!owns_legacy_entry(
            &format!("{old} --autostart extra"),
            &current
        ));
        assert!(!owns_legacy_entry(&format!("{old} --other"), &current));
    }
}
