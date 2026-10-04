//! Bounded attachment replay before the interactive relay workers start.
use super::{DomainStream, RESPONSE_TIMEOUT, ServerFrame, read_frame_with_timeout};
use anyhow::Context;
use std::io::Write;
use std::time::{Duration, Instant};

// A snapshot can include the complete bounded root/animation image cache.
// Give progressing transfers more time than one ordinary response, but never
// let repeated complete fragments keep attachment initialization alive forever.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub(super) fn replay(
    stream: &mut DomainStream,
    output: &mut impl Write,
    text: &[u8],
) -> anyhow::Result<()> {
    replay_with_limits(stream, output, text, RESPONSE_TIMEOUT, SNAPSHOT_TIMEOUT)
}

fn replay_with_limits(
    stream: &mut DomainStream,
    output: &mut impl Write,
    text: &[u8],
    frame_timeout: Duration,
    snapshot_timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + snapshot_timeout;
    output.write_all(text)?;
    output.flush()?;
    loop {
        let read_budget = frame_timeout.min(remaining(deadline)?);
        let frame = read_frame_with_timeout::<ServerFrame>(stream, read_budget)
            .context("read domain attachment snapshot")?;
        remaining(deadline)?;
        match frame {
            ServerFrame::SnapshotOutput { data } => output.write_all(data.as_bytes())?,
            ServerFrame::SnapshotComplete => break,
            ServerFrame::Error { message } => anyhow::bail!("{message}"),
            frame => anyhow::bail!("unexpected frame in domain attachment snapshot {frame:?}"),
        }
    }
    output.flush()?;
    // Synchronous output is not cancellable. Detect a late write/flush once it
    // returns, rather than reporting an expired replay as successful.
    remaining(deadline)?;
    Ok(())
}

fn remaining(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "domain attachment snapshot deadline elapsed",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::protocol;
    use std::os::unix::net::UnixStream;
    use std::thread;

    const FRAME: Duration = Duration::from_millis(40);
    const TOTAL: Duration = Duration::from_millis(160);

    fn pair() -> (DomainStream, UnixStream) {
        let (client, peer) = UnixStream::pair().unwrap();
        (DomainStream::Local(client), peer)
    }

    #[test]
    fn snapshot_trickled_header_and_payload_cannot_extend_a_frame() {
        for prefix in [1, 4] {
            let (mut client, mut peer) = pair();
            let mut frame = Vec::new();
            protocol::write_frame(
                &mut frame,
                &ServerFrame::SnapshotOutput {
                    data: "fixture".repeat(100),
                },
            )
            .unwrap();
            peer.write_all(&frame[..prefix]).unwrap();
            let sender = thread::spawn(move || {
                for byte in &frame[prefix..] {
                    thread::sleep(Duration::from_millis(2));
                    if peer.write_all(&[*byte]).is_err() {
                        return;
                    }
                }
                let _ = protocol::write_frame(&mut peer, &ServerFrame::SnapshotComplete);
            });
            let mut output = Vec::new();
            let result = replay_with_limits(
                &mut client,
                &mut output,
                b"text",
                FRAME,
                Duration::from_secs(5),
            );
            drop(client);
            sender.join().unwrap();
            let error = result.expect_err("partial frame renewed its read deadline");
            assert!(
                error
                    .chain()
                    .any(|cause| cause.to_string().contains("deadline elapsed")),
                "{error:#}"
            );
            assert_eq!(
                output, b"text",
                "partial JSON must never reach terminal output"
            );
        }
    }

    #[test]
    fn complete_snapshot_fragments_cannot_renew_the_total_budget() {
        let (mut client, mut peer) = pair();
        let sender = thread::spawn(move || {
            for _ in 0..80 {
                if protocol::write_frame(
                    &mut peer,
                    &ServerFrame::SnapshotOutput { data: "g".into() },
                )
                .is_err()
                {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let _ = protocol::write_frame(&mut peer, &ServerFrame::SnapshotComplete);
        });
        let mut output = Vec::new();
        let result = replay_with_limits(
            &mut client,
            &mut output,
            b"text",
            Duration::from_secs(1),
            TOTAL,
        );
        drop(client);
        sender.join().unwrap();
        assert!(
            result.is_err(),
            "complete fragments renewed the snapshot budget"
        );
        assert!(
            output.starts_with(b"textgg"),
            "fixture made no complete-frame progress"
        );
        assert!(output.len() < 84);
    }

    #[test]
    fn snapshot_completion_preserves_bytes_and_leaves_live_output_unconsumed() {
        let (mut client, mut peer) = pair();
        let live = ServerFrame::Output {
            sequence: 42,
            data: b"later".to_vec(),
        };
        let mut wire = Vec::new();
        for frame in [
            ServerFrame::SnapshotOutput {
                data: "\x1b_Gfixture\x1b\\".into(),
            },
            ServerFrame::SnapshotComplete,
            live.clone(),
        ] {
            protocol::write_frame(&mut wire, &frame).unwrap();
        }
        for bytes in wire.chunks(3) {
            peer.write_all(bytes).unwrap();
        }
        let mut output = Vec::new();
        replay_with_limits(
            &mut client,
            &mut output,
            b"text",
            Duration::from_secs(1),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output, b"text\x1b_Gfixture\x1b\\");
        assert_eq!(
            protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
            live
        );
    }

    #[test]
    fn snapshot_reports_server_and_out_of_phase_frames_without_forwarding_them() {
        for frame in [
            ServerFrame::Error {
                message: "fixture rejection".into(),
            },
            ServerFrame::Output {
                sequence: 1,
                data: b"not snapshot".to_vec(),
            },
        ] {
            let (mut client, mut peer) = pair();
            protocol::write_frame(&mut peer, &frame).unwrap();
            let mut output = Vec::new();
            assert!(replay_with_limits(&mut client, &mut output, b"text", FRAME, TOTAL).is_err());
            assert_eq!(output, b"text");
        }
    }

    #[test]
    fn delayed_final_flush_cannot_be_reported_as_on_time_completion() {
        struct SlowFlush {
            flushes: usize,
        }
        impl Write for SlowFlush {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                if self.flushes == 2 {
                    thread::sleep(Duration::from_millis(80));
                }
                Ok(())
            }
        }
        let (mut client, mut peer) = pair();
        protocol::write_frame(&mut peer, &ServerFrame::SnapshotComplete).unwrap();
        let mut output = SlowFlush { flushes: 0 };
        assert!(
            replay_with_limits(
                &mut client,
                &mut output,
                b"text",
                Duration::from_secs(1),
                FRAME
            )
            .is_err()
        );
        assert_eq!(output.flushes, 2);
    }
}
