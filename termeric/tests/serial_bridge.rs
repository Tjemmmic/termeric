#![cfg(target_os = "linux")]

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn pty_pair() -> (File, File, PathBuf) {
    let mut master = -1;
    let mut slave = -1;
    let mut name = [0 as libc::c_char; 256];
    // SAFETY: every pointer references valid writable storage and null
    // termios/winsize asks the kernel for defaults.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                name.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    for fd in [master, slave] {
        // SAFETY: fd is live and F_SETFD accepts the close-on-exec bit.
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    // SAFETY: openpty wrote one NUL-terminated pathname into name.
    let path = unsafe { CStr::from_ptr(name.as_ptr()) }
        .to_str()
        .unwrap()
        .into();
    // SAFETY: each descriptor is uniquely adopted once.
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave), path) }
}

fn read_exact_before(file: &mut File, bytes: &mut [u8], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out after reading {offset} bytes"
        );
        let millis = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll references one initialized descriptor record.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        assert!(ready > 0, "timed out after reading {offset} bytes");
        offset += file.read(&mut bytes[offset..]).unwrap();
    }
}

fn wait_for_raw_mode(file: &File, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: zeroed termios is valid output storage for tcgetattr.
        let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
        // SAFETY: the PTY master is live and attributes is writable.
        assert_eq!(
            unsafe { libc::tcgetattr(file.as_raw_fd(), &mut attributes) },
            0
        );
        if attributes.c_lflag & (libc::ICANON | libc::ECHO) == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "serial bridge never entered raw mode"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn attributes(file: &File) -> libc::termios {
    // SAFETY: zeroed termios is valid output storage for tcgetattr.
    let mut value = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: file owns a live PTY descriptor and value is writable.
    assert_eq!(unsafe { libc::tcgetattr(file.as_raw_fd(), &mut value) }, 0);
    value
}

#[test]
fn executable_bridge_configures_a_pseudoterminal_and_relays_exact_bytes() {
    let (mut terminal_master, terminal_slave, _) = pty_pair();
    let (mut device_master, device_slave, device_path) = pty_pair();
    let original_device_attributes = attributes(&device_slave);

    let stdin = terminal_slave.try_clone().unwrap();
    let stdout = terminal_slave.try_clone().unwrap();
    let stderr = terminal_slave;
    let mut child = Command::new(env!("CARGO_BIN_EXE_termeric"))
        .args([
            "__serial-bridge",
            device_path.to_str().unwrap(),
            "115200",
            "8",
            "none",
            "1",
            "none",
        ])
        .env_remove("TERMERIC_REAL_CODEX_CONTRACT")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();

    wait_for_raw_mode(&terminal_master, Duration::from_secs(2));
    let configured = attributes(&device_slave);
    assert_eq!(configured.c_lflag & (libc::ICANON | libc::ECHO), 0);
    assert_eq!(configured.c_cflag & libc::CSIZE, libc::CS8);
    // SAFETY: configured is initialized termios storage.
    assert_eq!(unsafe { libc::cfgetispeed(&configured) }, libc::B115200);
    // SAFETY: configured is initialized termios storage.
    assert_eq!(unsafe { libc::cfgetospeed(&configured) }, libc::B115200);
    terminal_master.write_all(b"host-to-device\0\xff").unwrap();
    let mut device_bytes = [0u8; 16];
    read_exact_before(
        &mut device_master,
        &mut device_bytes,
        Duration::from_secs(2),
    );
    assert_eq!(&device_bytes, b"host-to-device\0\xff");

    device_master.write_all(b"device-to-host\x1b").unwrap();
    let mut terminal_bytes = [0u8; 15];
    read_exact_before(
        &mut terminal_master,
        &mut terminal_bytes,
        Duration::from_secs(2),
    );
    assert_eq!(&terminal_bytes, b"device-to-host\x1b");

    drop(terminal_master);
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("serial bridge did not exit after its terminal closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "serial bridge exited with {status}");

    let restored = attributes(&device_slave);
    assert_eq!(restored.c_iflag, original_device_attributes.c_iflag);
    assert_eq!(restored.c_oflag, original_device_attributes.c_oflag);
    assert_eq!(restored.c_cflag, original_device_attributes.c_cflag);
    assert_eq!(restored.c_lflag, original_device_attributes.c_lflag);
}
