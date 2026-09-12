//! Shared terminal-event draining for every PTY owner.
//!
//! Protocol replies are consumed beside the terminal/PTY pair that generated
//! them. Only app-facing events leave this bridge, and each poll has a hard
//! forwarding ceiling so a hostile bell/notification stream cannot grow an
//! intermediate application queue without bound.

use std::io;
use termeric_core::{Terminal, TerminalEvent};

pub(crate) const MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL: usize = 1_024;

#[derive(Debug, Default)]
pub(crate) struct TerminalEventDrain {
    pub(crate) saw_event: bool,
    pub(crate) protocol_replies: usize,
    pub(crate) dropped_app_events: usize,
    pub(crate) reply_error: Option<io::Error>,
}

/// Drain all currently pending terminal events.
///
/// `PtyWrite` never enters `forwarded`: it is written through `reply` while the
/// originating PTY is still borrowed. Remaining events are forwarded up to
/// `max_forwarded`; events above that ceiling are deliberately discarded and
/// reported in the returned accounting.
pub(crate) fn drain_terminal_events(
    terminal: &mut Terminal,
    forwarded: &mut Vec<TerminalEvent>,
    max_forwarded: usize,
    mut reply: impl FnMut(&[u8]) -> io::Result<()>,
) -> TerminalEventDrain {
    let mut result = TerminalEventDrain::default();

    for event in terminal.drain_events() {
        result.saw_event = true;
        match event {
            TerminalEvent::PtyWrite(data) => {
                result.protocol_replies += 1;
                if result.reply_error.is_none() {
                    if let Err(error) = reply(&data) {
                        result.reply_error = Some(error);
                    }
                }
            }
            event if forwarded.len() < max_forwarded => forwarded.push(event),
            _ => result.dropped_app_events += 1,
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_replies_are_consumed_even_when_app_event_budget_is_full() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(b"\x07\x07\x1b[6n");
        let mut forwarded = Vec::new();
        let mut replies = Vec::new();

        let result = drain_terminal_events(&mut terminal, &mut forwarded, 1, |data| {
            replies.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(replies, b"\x1b[1;1R");
        assert_eq!(forwarded.len(), 1);
        assert!(matches!(forwarded[0], TerminalEvent::Bell));
        assert_eq!(result.protocol_replies, 1);
        assert_eq!(result.dropped_app_events, 1);
        assert!(terminal.drain_events().is_empty());
    }

    #[test]
    fn simultaneous_terminal_replies_stay_with_their_origin_sink() {
        let mut first = Terminal::new(80, 24);
        let mut second = Terminal::new(80, 24);
        first.process(b"\x1b[5;7H\x1b[6n");
        second.process(b"\x1b[10;20H\x1b[6n");
        let mut first_reply = Vec::new();
        let mut second_reply = Vec::new();

        drain_terminal_events(&mut first, &mut Vec::new(), 8, |data| {
            first_reply.extend_from_slice(data);
            Ok(())
        });
        drain_terminal_events(&mut second, &mut Vec::new(), 8, |data| {
            second_reply.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(first_reply, b"\x1b[5;7R");
        assert_eq!(second_reply, b"\x1b[10;20R");
    }

    #[test]
    fn kitty_graphics_acknowledgement_is_consumed_by_the_owner_bridge() {
        let mut terminal = Terminal::new(80, 24);
        // A real validated query acknowledgement must be consumed by the
        // terminal owner's PTY bridge, never forwarded as an app event.
        terminal.handle_graphics_command(b"a=q,f=32,s=1,v=1,i=7;AQIDBA==");
        let mut replies = Vec::new();

        let result = drain_terminal_events(&mut terminal, &mut Vec::new(), 8, |data| {
            replies.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(result.protocol_replies, 1);
        assert_eq!(replies, b"\x1b_Gi=7;OK\x1b\\");
        assert!(terminal.drain_events().is_empty());
    }

    #[test]
    fn pointer_shape_changes_are_forwarded_but_queries_stay_with_the_owner() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(b"\x1b]22;pointer\x1b\\\x1b]22;?__current__\x1b\\");
        let mut forwarded = Vec::new();
        let mut replies = Vec::new();

        let result = drain_terminal_events(&mut terminal, &mut forwarded, 8, |data| {
            replies.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(forwarded, vec![TerminalEvent::PointerShapeChanged]);
        assert_eq!(replies, b"\x1b]22;pointer\x1b\\");
        assert_eq!(result.protocol_replies, 1);
        assert_eq!(result.dropped_app_events, 0);
    }

    #[test]
    fn title_changes_are_forwarded_but_title_queries_stay_with_the_owner() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(b"\x1b]0;application-title\x1b\\\x1b[20t\x1b[21t\x1b[#S");
        let mut forwarded = Vec::new();
        let mut replies = Vec::new();

        let result = drain_terminal_events(&mut terminal, &mut forwarded, 8, |data| {
            replies.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(
            forwarded,
            vec![TerminalEvent::TitleChanged("application-title".into())]
        );
        assert_eq!(
            replies,
            b"\x1b]Lapplication-title\x1b\\\x1b]lapplication-title\x1b\\\x1b[0;10#S"
        );
        assert_eq!(result.protocol_replies, 3);
        assert_eq!(result.dropped_app_events, 0);
    }

    #[test]
    fn osc99_notifications_are_forwarded_but_queries_stay_with_the_owner() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(
            b"\x1b]99;i=capability:p=?;\x1b\\\x1b G\x1b]99;i=job:d=0:p=buttons:a=report;Open\xe2\x80\xa8Ignore\x1b\\\x1b]99;i=job:c=1:f=YnVpbGQtYWdlbnQ=:u=2;finished\x1b\\\x1b]99;i=job:p=close;\x1b\\\x1b]99;i=probe:p=alive;\x1b\\",
        );
        let mut forwarded = Vec::new();
        let mut replies = Vec::new();

        let result = drain_terminal_events(&mut terminal, &mut forwarded, 8, |data| {
            replies.extend_from_slice(data);
            Ok(())
        });

        assert_eq!(
            forwarded,
            vec![
                TerminalEvent::Notification(termeric_core::NotificationRequest {
                    title: Some("finished".into()),
                    body: String::new(),
                    identifier: Some("job".into()),
                    application_name: Some("build-agent".into()),
                    urgency: Some(termeric_core::NotificationUrgency::Critical),
                    report_close: true,
                    report_activation: true,
                    buttons: vec!["Open".into(), "Ignore".into()],
                    reply_uses_8bit_controls: true,
                    ..termeric_core::NotificationRequest::default()
                }),
                TerminalEvent::NotificationClose {
                    identifier: "job".into(),
                },
                #[cfg(target_os = "linux")]
                TerminalEvent::NotificationAliveQuery {
                    identifier: "probe".into(),
                    reply_uses_8bit_controls: true,
                },
            ]
        );
        #[cfg(target_os = "linux")]
        let expected_reply =
            b"\x1b]99;i=capability:p=?;p=title,body,close,alive,buttons:a=report:c=1:o=always:u=0,1,2:w=1\x1b\\"
                .as_slice();
        #[cfg(not(target_os = "linux"))]
        let expected_reply = b"\x1b]99;i=capability:p=?;p=title,body:o=always\x1b\\".as_slice();
        assert_eq!(replies, expected_reply);
        assert_eq!(result.protocol_replies, 1);
        assert_eq!(result.dropped_app_events, 0);
    }

    #[test]
    fn repeated_app_events_are_fully_drained_into_a_bounded_batch() {
        let mut terminal = Terminal::new(80, 24);
        let mut stream = Vec::new();
        for _ in 0..MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL * 2 {
            stream.push(b'\x07');
            stream.extend_from_slice(b"\x1b]9;notice\x07");
        }
        terminal.process(&stream);
        let mut forwarded = Vec::new();

        let result = drain_terminal_events(
            &mut terminal,
            &mut forwarded,
            MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL,
            |_| Ok(()),
        );

        assert_eq!(forwarded.len(), MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL);
        assert_eq!(
            result.dropped_app_events,
            MAX_FORWARDED_TERMINAL_EVENTS_PER_POLL * 3
        );
        assert!(
            forwarded
                .iter()
                .any(|event| matches!(event, TerminalEvent::Bell))
        );
        assert!(
            forwarded
                .iter()
                .any(|event| matches!(event, TerminalEvent::Notification(_)))
        );
        assert!(terminal.drain_events().is_empty());
    }
}
