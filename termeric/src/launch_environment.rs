//! Completing the environment of a Termeric that the Finder, the Dock, or
//! `open` started. Such an app inherits launchd's environment rather than a
//! login shell's: no locale, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and `/` as
//! its directory. Termeric's own shells start as login shells and set
//! themselves up, but everything else it starts (agents, plugins, launch
//! profiles' programs, Git) would miss Homebrew and the person's tools, and
//! text would not be UTF-8.

use std::ffi::OsString;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the login shell may take to report its `PATH`. A slow or
/// interactive profile must not keep Termeric from starting.
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Completes this process's environment for a GUI launch and says what it
/// changed, for the log once there is one.
///
/// # Safety
///
/// No other thread may exist: this changes the process environment, which
/// another thread could be reading.
pub(crate) unsafe fn complete() -> Vec<String> {
    debug_assert_eq!(
        thread_count(),
        Some(1),
        "the environment changes before any thread"
    );
    let mut changes = Vec::new();
    if let Some((name, value)) = missing_locale() {
        // SAFETY: the caller guarantees this is the only thread.
        unsafe { std::env::set_var(name, &value) };
        changes.push(format!("set {name}={value} from the system locale"));
    }
    if !started_by_launchd() {
        return changes;
    }
    if let Some(path) = login_shell_path() {
        // SAFETY: as above.
        unsafe { std::env::set_var("PATH", &path) };
        changes.push("took PATH from the login shell".to_owned());
    }
    if std::env::current_dir().is_ok_and(|directory| directory == Path::new("/"))
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && home.is_dir()
        && std::env::set_current_dir(&home).is_ok()
    {
        changes.push(format!("started in {}", home.display()));
    }
    changes
}

/// Raise the soft limit on open files: launchd starts apps with 256, which
/// many tabs, their terminals, and agents' pipes can use up. macOS refuses a
/// soft limit above `OPEN_MAX` (10240), whatever the hard limit says.
pub(crate) fn raise_open_file_limit() -> Option<String> {
    const OPEN_MAX: libc::rlim_t = 10240;
    // SAFETY: an all-zero rlimit is valid output storage.
    let mut limit = unsafe { std::mem::zeroed::<libc::rlimit>() };
    // SAFETY: getrlimit writes the limit it was given.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    let wanted = limit.rlim_max.min(OPEN_MAX);
    if limit.rlim_cur >= wanted {
        return None;
    }
    let previous = limit.rlim_cur;
    limit.rlim_cur = wanted;
    // SAFETY: setrlimit reads the limit it was given.
    (unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } == 0)
        .then(|| format!("raised the open-file limit from {previous} to {wanted}"))
}

/// The locale variable to set when none is: `LANG` for the system's
/// language and region when macOS has that locale, else `en_US.UTF-8`, so
/// text is UTF-8 either way. Only `LANG` is ever set: `ssh` sends `LC_*`
/// variables to other machines, where a macOS-only value such as
/// `LC_CTYPE=UTF-8` would break their locale.
fn missing_locale() -> Option<(&'static str, String)> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(set) {
        return None;
    }
    let locale = system_locale()
        .and_then(|tag| posix_locale(&tag))
        .filter(|locale| Path::new("/usr/share/locale").join(locale).is_dir())
        .unwrap_or_else(|| "en_US.UTF-8".to_owned());
    Some(("LANG", locale))
}

/// The system's language and region (`AppleLocale`, such as `en_US` or
/// `de_DE@currency=EUR`), read by the `defaults` tool rather than in this
/// process: CoreFoundation's preferences start threads of their own, and the
/// environment must change before there are any.
fn system_locale() -> Option<String> {
    let output = Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let locale = String::from_utf8(output.stdout).ok()?;
    let locale = locale.trim().split('@').next()?.to_owned();
    (output.status.success() && !locale.is_empty()).then_some(locale)
}

/// How many threads this process has.
fn thread_count() -> Option<i32> {
    let pid = libc::c_int::try_from(std::process::id()).ok()?;
    // SAFETY: proc_taskinfo is plain old data, for which zeroes are valid.
    let mut info = unsafe { std::mem::zeroed::<libc::proc_taskinfo>() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_taskinfo>()).ok()?;
    // SAFETY: the buffer is writable for exactly `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    (written == size).then_some(info.pti_threadnum)
}

/// A POSIX locale name for a BCP 47 tag such as `en-US` or `zh-Hans-CN`:
/// its language and region with UTF-8, as `en_US.UTF-8`.
fn posix_locale(tag: &str) -> Option<String> {
    let mut subtags = tag.split(['-', '_']);
    let language = subtags
        .next()
        .filter(|language| (2..=3).contains(&language.len()))?;
    let region = subtags.find(|subtag| {
        subtag.len() == 2 && subtag.bytes().all(|byte| byte.is_ascii_alphabetic())
    })?;
    Some(format!(
        "{}_{}.UTF-8",
        language.to_ascii_lowercase(),
        region.to_ascii_uppercase()
    ))
}

/// The app bundle to start a quick-access terminal from, when this process
/// was started by something other than launchd (the Shortcuts app, a
/// script, a terminal) from inside one. Starting it through LaunchServices
/// instead makes it an app of its own rather than that program's child, so
/// it outlives the command, and macOS asks for permissions on Termeric's
/// behalf rather than the caller's.
pub(crate) fn quick_access_relaunch_bundle() -> Option<PathBuf> {
    if started_by_launchd() {
        return None;
    }
    let executable = crate::paths::executable().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Start `bundle` anew through LaunchServices with this process's
/// arguments, and wait only for `open` itself.
pub(crate) fn relaunch_through_launch_services(bundle: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let status = Command::new("/usr/bin/open")
        .args(["-n", "-a"])
        .arg(bundle)
        .arg("--args")
        .args(std::env::args_os().skip(1))
        .status()
        .context("start the quick-access terminal through open")?;
    if !status.success() {
        anyhow::bail!("open could not start {} ({status})", bundle.display());
    }
    Ok(())
}

/// Whether launchd started this process directly, as it does for the
/// Finder, the Dock, and `open`, rather than a shell or another program.
fn started_by_launchd() -> bool {
    // SAFETY: getppid has no preconditions.
    unsafe { libc::getppid() == 1 }
}

/// The `PATH` the person's login shell sets up, asked for without starting a
/// thread (see [`complete`]).
fn login_shell_path() -> Option<OsString> {
    login_path_of(&crate::terminal::shell_integration::default_shell_program()?)
}

/// The `PATH` `shell` sets up as an interactive login shell.
fn login_path_of(shell: &str) -> Option<OsString> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(shell)
        // Interactive too, so what `~/.zshrc` adds (nvm, pyenv) counts, as
        // in the editors that do the same.
        .args(["-l", "-i", "-c", "exec /usr/bin/printenv PATH"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // A group of its own, so whatever a profile left running ends with
        // the shell rather than holding the output open.
        .process_group(0)
        .spawn()
        .ok()?;
    let output = child
        .stdout
        .take()
        .and_then(|stdout| read_until(stdout, Instant::now() + LOGIN_SHELL_TIMEOUT));
    // The group is signalled before the shell is reaped, so its ID can't
    // belong to anything else yet.
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill has no memory preconditions; the group is the child's.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.wait();
    login_path_from_output(&output?)
}

/// Everything `stdout` gives until it ends, or `None` if `deadline` comes
/// first.
fn read_until(mut stdout: std::process::ChildStdout, deadline: Instant) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let millis = libc::c_int::try_from(left.as_millis().max(1)).unwrap_or(libc::c_int::MAX);
        let [ready] = crate::deadline_io::ready_to_read([stdout.as_raw_fd()], millis).ok()?;
        if !ready {
            continue;
        }
        match stdout.read(&mut buffer) {
            Ok(0) => return Some(output),
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
        // A PATH is far smaller; a profile that floods the output is not
        // waited out.
        if output.len() > 1024 * 1024 {
            return None;
        }
    }
}

/// The path from the login shell's output. Profiles may print before the
/// command runs, so it is the last line, and it must look like a `PATH`.
fn login_path_from_output(output: &[u8]) -> Option<OsString> {
    let text = std::str::from_utf8(output).ok()?;
    let path = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?
        .trim();
    (path.starts_with('/') && !path.contains('\0')).then(|| path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_locale_reads_as_a_posix_locale() {
        // A fresh account may have no AppleLocale yet; then en_US.UTF-8 is
        // used, which the other tests cover.
        if let Some(locale) = system_locale() {
            assert!(posix_locale(&locale).is_some(), "{locale}");
        }
    }

    #[test]
    fn locale_tags_become_utf8_posix_locales() {
        assert_eq!(posix_locale("en-US").as_deref(), Some("en_US.UTF-8"));
        assert_eq!(posix_locale("de_DE").as_deref(), Some("de_DE.UTF-8"));
        assert_eq!(posix_locale("zh-Hans-CN").as_deref(), Some("zh_CN.UTF-8"));
        assert_eq!(posix_locale("en"), None);
        assert_eq!(posix_locale("x-private"), None);
    }

    #[test]
    fn the_login_path_is_the_last_line_the_shell_prints() {
        assert_eq!(
            login_path_from_output(b"Welcome!\n/opt/homebrew/bin:/usr/bin:/bin\n\n"),
            Some("/opt/homebrew/bin:/usr/bin:/bin".into())
        );
        assert_eq!(login_path_from_output(b"no path here\n"), None);
        assert_eq!(login_path_from_output(b""), None);
    }

    #[test]
    fn a_login_shell_reports_its_path() {
        // The system's own sh, whose profile (`path_helper`) every Mac has,
        // rather than the test user's shell and its dotfiles.
        let path = login_path_of("/bin/sh").expect("a login sh reports PATH");
        assert!(
            path.to_str()
                .unwrap()
                .split(':')
                .any(|entry| entry == "/usr/bin")
        );
    }

    #[test]
    fn a_reader_stops_at_its_deadline() {
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let output = read_until(
            child.stdout.take().unwrap(),
            Instant::now() + Duration::from_millis(100),
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(output, None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
