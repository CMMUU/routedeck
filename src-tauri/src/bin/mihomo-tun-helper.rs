#[cfg(target_os = "macos")]
fn main() {
    serylane_lib::tun_service::daemon::run()
}

#[cfg(windows)]
fn main() {
    eprintln!("Windows TUN runs inside the elevated Serylane application session. No TUN helper service is installed; quit the app from its tray menu and choose Run as administrator.");
    std::process::exit(1);
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("mihomo-tun-helper is only available on macOS");
    std::process::exit(1);
}
