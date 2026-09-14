//! Explicit environment-only stdin ingestion; never child terminal input.

use super::{
    ControlCommand,
    protocol::{self, EnvironmentVariable, Request},
};
use serde::Deserializer;
use serde::de::{self, MapAccess, Visitor};
use std::fmt;

const MAX_INPUT_BYTES: usize = 256 * 1024;
#[cfg(target_os = "linux")]
const INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) fn prepare(command: &mut ControlCommand) -> anyhow::Result<()> {
    let (environment, policy) = match command {
        ControlCommand::LaunchTab {
            env_stdin: true,
            environment,
            environment_policy,
            ..
        }
        | ControlCommand::LaunchWindow {
            env_stdin: true,
            environment,
            environment_policy,
            ..
        } => (environment, environment_policy),
        ControlCommand::LaunchAndWait {
            env_stdin: true,
            request,
            ..
        } => match request {
            Request::LaunchTab {
                environment,
                environment_policy,
                ..
            }
            | Request::LaunchWindow {
                environment,
                environment_policy,
                ..
            } => (environment, environment_policy),
            _ => anyhow::bail!("--env-stdin requires an ordinary command launch"),
        },
        _ => return Ok(()),
    };
    let input = read_stdin()?;
    merge(environment, policy, &input)
}

fn merge(
    environment: &mut Vec<EnvironmentVariable>,
    policy: &termeric_core::pty::EnvironmentPolicy,
    input: &[u8],
) -> anyhow::Result<()> {
    let mut combined = environment.clone();
    combined.extend(parse(input)?);
    protocol::validate_control_launch_environment(&combined, policy)
        .map_err(|_| anyhow::anyhow!("invalid or over-limit combined --env-stdin environment"))?;
    *environment = combined;
    Ok(())
}

fn parse(input: &[u8]) -> anyhow::Result<Vec<EnvironmentVariable>> {
    if input.len() > MAX_INPUT_BYTES {
        anyhow::bail!("--env-stdin exceeds 256 KiB encoded input limit");
    }
    struct EnvironmentVisitor;
    impl<'de> Visitor<'de> for EnvironmentVisitor {
        type Value = Vec<EnvironmentVariable>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an environment object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut entries: Vec<EnvironmentVariable> = Vec::new();
            while let Some(name) = map.next_key::<String>()? {
                if entries.len() >= protocol::MAX_CONTROL_LAUNCH_ENVIRONMENT_ENTRIES
                    || name.len() > protocol::MAX_CONTROL_LAUNCH_ENVIRONMENT_NAME_BYTES
                    || entries.iter().any(|entry| entry.name == name)
                {
                    return Err(de::Error::custom("invalid environment entry"));
                }
                let value = map.next_value::<String>()?;
                if value.len() > protocol::MAX_CONTROL_LAUNCH_ENVIRONMENT_VALUE_BYTES {
                    return Err(de::Error::custom("invalid environment value"));
                }
                entries.push(EnvironmentVariable { name, value });
            }
            Ok(entries)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(input);
    let entries = decoder.deserialize_map(EnvironmentVisitor).map_err(|_| {
        anyhow::anyhow!(
            "--env-stdin requires one bounded JSON object of unique string names and string values"
        )
    })?;
    decoder
        .end()
        .map_err(|_| anyhow::anyhow!("--env-stdin contains invalid trailing input"))?;
    Ok(entries)
}

#[cfg(not(target_os = "linux"))]
fn read_stdin() -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("--env-stdin currently requires Linux");
}

#[cfg(target_os = "linux")]
fn read_stdin() -> anyhow::Result<Vec<u8>> {
    use std::os::fd::AsFd;
    let input = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| anyhow::anyhow!("cannot acquire --env-stdin input"))?;
    read_file(std::fs::File::from(input), INPUT_TIMEOUT)
}

#[cfg(target_os = "linux")]
fn read_file(mut input: std::fs::File, timeout: std::time::Duration) -> anyhow::Result<Vec<u8>> {
    use std::io::{IsTerminal, Read};
    use std::os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, OpenOptionsExt},
    };
    use std::time::Instant;
    if input.is_terminal() {
        anyhow::bail!("--env-stdin requires redirected input, not an interactive terminal");
    }
    // SAFETY: F_GETFL inspects flags on our owned, live input descriptor.
    let flags = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 || flags & libc::O_ACCMODE == libc::O_WRONLY {
        anyhow::bail!("--env-stdin requires a readable input descriptor");
    }
    let metadata = input
        .metadata()
        .map_err(|_| anyhow::anyhow!("cannot inspect --env-stdin input"))?;
    if metadata.file_type().is_fifo() {
        // Reopen this pinned pipe through procfs to obtain a separate open-file
        // description. NONBLOCK must not change the invoking shell's stdin flags.
        input = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
            .open(format!("/proc/self/fd/{}", input.as_raw_fd()))
            .map_err(|_| anyhow::anyhow!("cannot acquire nonblocking --env-stdin pipe"))?;
    } else if !metadata.is_file() {
        anyhow::bail!("--env-stdin requires a regular file or pipe");
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| anyhow::anyhow!("--env-stdin input timed out before launch"))?;
        let mut descriptor = libc::pollfd {
            fd: input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized poll record refers to our owned live input fd.
        let ready = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                remaining.as_millis().clamp(1, i32::MAX as u128) as i32,
            )
        };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            anyhow::bail!("cannot poll --env-stdin input");
        }
        if ready == 0 {
            continue;
        }
        let mut chunk = [0u8; 4096];
        let limit = chunk.len().min(MAX_INPUT_BYTES + 1 - bytes.len());
        let result = input.read(&mut chunk[..limit]);
        if Instant::now() >= deadline {
            anyhow::bail!("--env-stdin input timed out before launch");
        }
        match result {
            Ok(0) => return Ok(bytes),
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(_) => anyhow::bail!("cannot read --env-stdin input"),
        }
        if bytes.len() > MAX_INPUT_BYTES {
            anyhow::bail!("--env-stdin exceeds 256 KiB encoded input limit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn pipe() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let mut fds = [-1; 2];
        // SAFETY: pipe2 initializes two descriptors in the supplied array.
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        // SAFETY: adopt each newly allocated descriptor exactly once.
        unsafe {
            (
                std::fs::File::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pipe_eof_deadline_and_open_flags_are_bounded_and_local() {
        use std::{
            io::Write,
            os::fd::AsRawFd,
            time::{Duration, Instant},
        };
        let (reader, mut writer) = pipe();
        assert!(read_file(writer.try_clone().unwrap(), Duration::from_secs(1)).is_err());
        // SAFETY: fcntl reads flags from a live, owned fixture descriptor.
        let original_flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
        writer.write_all(b"{}").unwrap();
        let start = Instant::now();
        let error = read_file(reader.try_clone().unwrap(), Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
        // SAFETY: the original pipe descriptor is still alive.
        assert_eq!(
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) },
            original_flags
        );
        drop(writer);
        assert!(
            read_file(reader, Duration::from_secs(1))
                .unwrap()
                .is_empty()
        );
        let (reader, mut writer) = pipe();
        writer.write_all(br#"{"VALUE":"exact"}"#).unwrap();
        drop(writer);
        assert_eq!(
            read_file(reader, Duration::from_secs(1)).unwrap(),
            br#"{"VALUE":"exact"}"#
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn regular_file_offset_limits_and_special_input_refusal() {
        use std::{
            io::{Seek, SeekFrom, Write},
            time::Duration,
        };
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"prefix{}").unwrap();
        file.seek(SeekFrom::Start(6)).unwrap();
        assert_eq!(read_file(file, Duration::from_secs(1)).unwrap(), b"{}");
        for count in [MAX_INPUT_BYTES, MAX_INPUT_BYTES + 1] {
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(&vec![b' '; count]).unwrap();
            file.rewind().unwrap();
            assert_eq!(
                read_file(file, Duration::from_secs(1)).is_ok(),
                count == MAX_INPUT_BYTES
            );
        }
        assert!(
            read_file(
                std::fs::File::open("/dev/null").unwrap(),
                Duration::from_secs(1)
            )
            .is_err()
        );
        let directory = tempfile::tempdir().unwrap();
        assert!(
            read_file(
                std::fs::File::open(directory.path()).unwrap(),
                Duration::from_secs(1)
            )
            .is_err()
        );
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let descriptor: std::os::fd::OwnedFd = socket.into();
        assert!(read_file(std::fs::File::from(descriptor), Duration::from_secs(1)).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn interactive_terminal_is_refused_before_reading() {
        use std::os::fd::FromRawFd;
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes two descriptors; null optional arguments
        // select defaults and no caller-provided pathname storage.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // SAFETY: adopt each initialized fixture descriptor once.
        let (_master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        };
        let error = read_file(slave, std::time::Duration::from_secs(1)).unwrap_err();
        assert!(error.to_string().contains("interactive terminal"));
    }

    #[test]
    fn exact_strings_merge_without_expansion_and_errors_do_not_mutate() {
        let mut environment = vec![EnvironmentVariable {
            name: "LITERAL".into(),
            value: "first".into(),
        }];
        let policy = termeric_core::pty::EnvironmentPolicy::default();
        merge(
            &mut environment,
            &policy,
            br#"{"TOKEN":"$literal=\n\t\u263a","EMPTY":""}"#,
        )
        .unwrap();
        assert_eq!(environment[1].value, "$literal=\n\t☺");
        assert_eq!(environment[2].value, "");
        let original = environment.clone();
        for bad in [
            br#"{"LITERAL":"synthetic-private"}"#.as_slice(),
            br#"{"TERM":"synthetic-private"}"#,
            br#"{"BAD-NAME":"synthetic-private"}"#,
            br#"{"NUL":"synthetic-private\u0000"}"#,
        ] {
            let error = merge(&mut environment, &policy, bad).unwrap_err();
            assert!(!format!("{error:#}").contains("synthetic-private"));
            assert_eq!(environment, original);
        }
        let debug = format!("{environment:?}");
        assert!(!debug.contains("$literal"));
        assert!(!debug.contains("TOKEN"));
    }

    #[test]
    fn malformed_duplicate_typed_and_trailing_documents_are_refused() {
        for bad in [
            b"".as_slice(),
            b"[]",
            b"null",
            br#"{"A":1}"#,
            br#"{"A":true}"#,
            br#"{"A":{}}"#,
            br#"{"A":[]}"#,
            br#"{"A":"x","A":"y"}"#,
            br#"{"A":"x","\u0041":"y"}"#,
            b"{} {}",
            b"{}synthetic-private",
            b"{\"A\":\"\xff\"}",
        ] {
            assert!(parse(bad).is_err(), "input unexpectedly accepted");
        }
        assert!(parse(&vec![b' '; MAX_INPUT_BYTES + 1]).is_err());
        assert!(parse(format!("{{\"A\":\"{}\"}}", "x".repeat(8193)).as_bytes()).is_err());
        assert!(parse(format!("{{\"{}\":\"x\"}}", "A".repeat(129)).as_bytes()).is_err());
        assert!(parse(b"{}\n ").unwrap().is_empty());
        let padded = format!("{{}}{}", " ".repeat(MAX_INPUT_BYTES - 2));
        assert!(parse(padded.as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn combined_limits_removals_and_clear_policy_are_preserved() {
        let document = |count| {
            serde_json::to_vec(
                &(0..count)
                    .map(|i| (format!("N{i}"), "v"))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            )
            .unwrap()
        };
        assert!(parse(&document(64)).is_ok());
        assert!(parse(&document(65)).is_err());
        let policy = termeric_core::pty::EnvironmentPolicy::default();
        let mut environment = parse(&document(64)).unwrap();
        assert!(merge(&mut environment, &policy, br#"{"NEW":"v"}"#).is_err());
        let removed = termeric_core::pty::EnvironmentPolicy {
            clear: false,
            unset: vec!["A".into()],
        };
        assert!(merge(&mut Vec::new(), &removed, br#"{"A":"v"}"#).is_err());
        let cleared = termeric_core::pty::EnvironmentPolicy {
            clear: true,
            unset: Vec::new(),
        };
        assert!(merge(&mut Vec::new(), &cleared, br#"{"A":"v"}"#).is_ok());
        let large = serde_json::to_vec(
            &(0..8)
                .map(|i| (format!("N{i}"), "v".repeat(8192)))
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .unwrap();
        assert!(merge(&mut Vec::new(), &policy, &large).is_err());
    }
}
