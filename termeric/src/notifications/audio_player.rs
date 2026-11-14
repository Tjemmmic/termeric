//! The audible bell on Linux: the freedesktop bell sound, played by
//! `paplay`, one sound at a time, each watched until it ends.

use super::FailureReport;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A bell sound still playing after this long is stopped and reported, so a
/// hung player cannot silence the bell for good.
const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the waiter checks whether the player has exited.
const AUDIO_PLAYER_POLL: Duration = Duration::from_millis(20);

pub(super) struct BellSound {
    command: (String, Vec<String>),
    timeout: Duration,
    /// The waiter for the sound playing now, which returns how it ended. One
    /// at a time: a bell during it stays silent.
    player: Option<JoinHandle<Result<(), String>>>,
    failure: FailureReport,
}

impl BellSound {
    pub(super) fn new() -> Self {
        Self {
            command: (
                "paplay".into(),
                vec!["/usr/share/sounds/freedesktop/stereo/bell.oga".into()],
            ),
            timeout: AUDIO_PLAYER_TIMEOUT,
            player: None,
            failure: FailureReport::new("Audio bell unavailable"),
        }
    }

    /// Starts the bell sound unless one is still playing. Returns a failure
    /// to show: how the last sound ended, or why this one could not start. A
    /// sound that played through is what clears an earlier failure.
    pub(super) fn play(&mut self) -> Option<String> {
        if self
            .player
            .as_ref()
            .is_some_and(|player| !player.is_finished())
        {
            return None;
        }
        let mut failure = None;
        if let Some(player) = self.player.take() {
            match player.join() {
                Ok(Ok(())) => self.failure.recovered(),
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some("the audio bell waiter panicked".to_owned()),
            }
        }
        let (program, args) = &self.command;
        match start_audio_player(program, args, self.timeout) {
            Ok(player) => self.player = Some(player),
            Err(error) => failure = Some(error),
        }
        failure.and_then(|error| self.failure.report(&error))
    }

    /// The audible bell was turned back on, so a player failure is shown
    /// again.
    pub(super) fn reenabled(&mut self) {
        self.failure.recovered();
    }

    #[cfg(test)]
    pub(super) fn set_command_for_test(&mut self, program: &str, args: &[&str]) {
        self.command = (
            program.into(),
            args.iter().map(|arg| (*arg).into()).collect(),
        );
    }
}

/// Plays one bell sound. A waiter thread owns the player: it reaps it, even
/// after the `Bell` is gone, and stops it if it is still running after
/// `timeout`. The player's stdio is null, so its messages never reach the
/// terminal Termeric was started from.
fn start_audio_player(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<JoinHandle<Result<(), String>>, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let name = program.to_owned();
    let (started, result) = mpsc::sync_channel(1);
    // Create the waiter before spawning its player, so a thread creation
    // failure cannot leave a process nobody reaps.
    let waiter = std::thread::Builder::new()
        .name("termeric-bell".into())
        .spawn(move || match command.spawn() {
            Ok(player) => {
                let _ = started.send(Ok(()));
                wait_for_audio_player(player, &name, timeout)
            }
            Err(error) => {
                let _ = started.send(Err(error));
                Ok(())
            }
        })
        .map_err(|error| format!("could not start the audio bell waiter: {error}"))?;
    match result.recv() {
        Ok(Ok(())) => Ok(waiter),
        Ok(Err(error)) => Err(format!("could not start {program}: {error}")),
        Err(_) => Err(format!(
            "the audio bell waiter stopped before starting {program}"
        )),
    }
}

/// Waits for `player` to exit, stopping it once `timeout` has passed, and
/// returns a failure unless it played through.
fn wait_for_audio_player(
    mut player: Child,
    program: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match player.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited unsuccessfully ({status})")),
            Ok(None) if Instant::now() >= deadline => break,
            Ok(None) => std::thread::sleep(AUDIO_PLAYER_POLL),
            Err(error) => {
                log::warn!("Could not check on the audio bell player {program}: {error}");
                break;
            }
        }
    }
    player
        .kill()
        .and_then(|()| player.wait())
        .map_err(|error| format!("could not stop {program}: {error}"))?;
    Err(format!(
        "{program} was still playing after {timeout:?} and was stopped"
    ))
}

#[cfg(test)]
mod tests {
    use crate::config::BellConfigFile;
    use crate::notifications::Bell;
    use std::time::Duration;

    fn audio_bell() -> Bell {
        let mut bell = Bell::new(BellConfigFile {
            visual: false,
            audio: true,
            ..BellConfigFile::default()
        });
        bell.rate_limit = Duration::ZERO;
        bell
    }

    #[test]
    fn audio_player_failures_are_bounded_deduplicated_and_recoverable() {
        let mut bell = audio_bell();
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
        let first = bell.ring().unwrap();
        assert!(first.contains("Audio bell unavailable"));
        assert!(first.contains("termeric-audio-helper"));
        assert!(bell.ring().is_none());

        // Starting a player is not enough: one that plays through clears
        // the failure, so the next one is shown again.
        bell.set_audio_command_for_test("/bin/true", &[]);
        assert!(bell.ring().is_none());
        wait_for_player(&bell);
        bell.set_audio_command_for_test("/definitely/missing/termeric-audio-helper", &[]);
        assert!(bell.ring().is_some());
    }

    /// Waits for the playing sound's waiter to finish, without joining it, so
    /// the next bell is the one that collects how it ended.
    fn wait_for_player(bell: &Bell) {
        let player = bell.sound.player.as_ref().expect("a sound is playing");
        assert!(
            crate::test_support::poll_until(Duration::from_secs(10), || player.is_finished()),
            "the audio bell waiter did not finish"
        );
    }

    /// Lines the fake player appended to `log`, once there are `count`.
    fn wait_for_lines(log: &std::path::Path, count: usize) -> Vec<String> {
        let read = || {
            std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert!(
            crate::test_support::poll_until(Duration::from_secs(10), || read().len() >= count),
            "the fake player did not start"
        );
        read()
    }

    fn assert_reaped(pid: &str) {
        let pid: libc::pid_t = pid.parse().unwrap();
        let mut status = 0;
        // SAFETY: The PID names a fake player this process started, status
        // is writable, and WNOHANG never blocks.
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        let error = std::io::Error::last_os_error();
        assert_eq!(result, -1, "the player was not reaped by its waiter");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    /// A fake player: it appends its PID and whether its stdin, stdout, and
    /// stderr lead to `/dev/null` to the log in `$1`, then plays until `$2`
    /// exists. Each descriptor is checked before the log line is written:
    /// dash applies `>&3` to the shell itself while the command runs, so its
    /// own stdout would read as the log.
    const FAKE_PLAYER: &str = r#"exec 3>>"$1"
echo "$$" >&3
for fd in 0 1 2; do
  if [ "/dev/fd/$fd" -ef /dev/null ]; then target=/dev/null; else target=other; fi
  echo "$target" >&3
done
exec 3>&-
while [ ! -e "$2" ]; do sleep 0.01; done"#;

    #[test]
    fn one_sound_plays_at_a_time_without_stdio_and_is_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("player.log");
        let release = directory.path().join("release");
        let mut bell = audio_bell();
        bell.set_audio_command_for_test(
            "/bin/sh",
            &[
                "-c",
                FAKE_PLAYER,
                "player",
                log.to_str().unwrap(),
                release.to_str().unwrap(),
            ],
        );
        assert!(bell.ring().is_none());
        let lines = wait_for_lines(&log, 4);
        assert_eq!(lines[1..], ["/dev/null", "/dev/null", "/dev/null"]);
        let waiter = bell.sound.player.as_ref().unwrap().thread().id();

        // A bell during the sound stays silent instead of starting another.
        assert!(bell.ring().is_none());
        assert_eq!(bell.sound.player.as_ref().unwrap().thread().id(), waiter);
        bell.update();
        assert!(bell.sound.player.is_some());

        std::fs::write(&release, b"").unwrap();
        wait_for_player(&bell);
        assert_reaped(&lines[0]);
        assert!(bell.ring().is_none());
        assert_ne!(bell.sound.player.as_ref().unwrap().thread().id(), waiter);
        let lines = wait_for_lines(&log, 8);
        assert_ne!(lines[4], lines[0]);
        wait_for_player(&bell);
        assert_reaped(&lines[4]);
    }

    #[test]
    fn a_hung_player_is_stopped_reaped_and_reported_once() {
        let directory = tempfile::tempdir().unwrap();
        let pid = directory.path().join("pid");
        let mut bell = audio_bell();
        bell.sound.timeout = Duration::from_secs(1);
        bell.set_audio_command_for_test(
            "/bin/sh",
            &[
                "-c",
                r#"echo "$$" > "$1"; exec sleep 30"#,
                "player",
                pid.to_str().unwrap(),
            ],
        );
        assert!(bell.ring().is_none());
        wait_for_player(&bell);
        let first = std::fs::read_to_string(&pid).unwrap();
        assert_reaped(first.trim());

        let report = bell.ring().unwrap();
        assert!(report.contains("Audio bell unavailable"), "{report}");
        assert!(report.contains("was still playing after 1s"), "{report}");
        // That bell started another player, which hangs the same way.
        wait_for_player(&bell);
        let second = std::fs::read_to_string(&pid).unwrap();
        assert_ne!(second, first);
        assert_reaped(second.trim());
        assert!(bell.ring().is_none(), "the same failure is shown once");
        wait_for_player(&bell);
    }
}
