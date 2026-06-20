//! Browser-launch argv (arch §7.3) — the only conditional compilation in brazen,
//! reduced to DATA. `browser_argv` returns the argv vector to open `url`; it does
//! NOT exec (the `Command::spawn` is the `bz` shim's one uncovered line). Keeping
//! the OS→argv map a pure function of an injected `os` string lets all three
//! targets be asserted on a single Linux runner (arch §9.4); the public entry
//! just feeds it `std::env::consts::OS`, the compile-time target.

/// The argv to open `url` in the user's default browser on the build target
/// (arch §7.3). A thin pass of the compile-time OS to the pure `argv_for`; the
/// caller (`bz login --browser`) `Command::spawn`s the result.
pub fn browser_argv(url: &str) -> Vec<String> {
    argv_for(std::env::consts::OS, url)
}

/// The OS→argv map as pure data (arch §7.3, §9.4): macOS `open`, Windows
/// `cmd /C start "" <url>` (the empty `""` is the title arg `start` consumes so a
/// quoted URL is not mistaken for one), everything else `xdg-open`. Tested for
/// all three `os` values regardless of the runner, since `os` is a parameter.
fn argv_for(os: &str, url: &str) -> Vec<String> {
    match os {
        "macos" => vec!["open".into(), url.into()],
        "windows" => vec![
            "cmd".into(),
            "/C".into(),
            "start".into(),
            "".into(),
            url.into(),
        ],
        _ => vec!["xdg-open".into(), url.into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_open() {
        assert_eq!(argv_for("macos", "https://x"), vec!["open", "https://x"]);
    }

    #[test]
    fn windows_uses_cmd_start_with_empty_title() {
        assert_eq!(
            argv_for("windows", "https://x"),
            vec!["cmd", "/C", "start", "", "https://x"]
        );
    }

    #[test]
    fn other_uses_xdg_open() {
        assert_eq!(
            argv_for("linux", "https://x"),
            vec!["xdg-open", "https://x"]
        );
        assert_eq!(
            argv_for("freebsd", "https://x"),
            vec!["xdg-open", "https://x"]
        );
    }

    #[test]
    fn public_entry_feeds_compile_time_os() {
        // Exercises the one-line wrapper; the value is the runner's OS arm.
        let argv = browser_argv("https://x");
        assert_eq!(argv.last().map(String::as_str), Some("https://x"));
        assert!(!argv.is_empty());
    }
}
