//! Native Linux serial transport used by serial terminal profiles.

use crate::config::{SerialFlowControl, SerialParity, SerialProfileConfig};
use anyhow::Context;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::PathBuf;

const INTERNAL_COMMAND: &str = "__serial-bridge";
const RELAY_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeOptions {
    pub(crate) config: SerialProfileConfig,
}

impl BridgeOptions {
    pub(crate) fn parse(arguments: &[OsString]) -> anyhow::Result<Self> {
        if arguments.len() != 6 {
            anyhow::bail!("internal serial bridge received an invalid argument count");
        }
        let utf8 = |index: usize, name: &str| -> anyhow::Result<&str> {
            arguments[index]
                .to_str()
                .with_context(|| format!("serial {name} must be valid UTF-8"))
        };
        let parity = match utf8(3, "parity")? {
            "none" => SerialParity::None,
            "even" => SerialParity::Even,
            "odd" => SerialParity::Odd,
            value => anyhow::bail!("unsupported serial parity {value:?}"),
        };
        let flow_control = match utf8(5, "flow control")? {
            "none" => SerialFlowControl::None,
            "software" => SerialFlowControl::Software,
            "hardware" => SerialFlowControl::Hardware,
            value => anyhow::bail!("unsupported serial flow control {value:?}"),
        };
        let config = SerialProfileConfig {
            device: PathBuf::from(&arguments[0]),
            baud: utf8(1, "baud rate")?
                .parse()
                .context("serial baud rate must be an unsigned integer")?,
            data_bits: utf8(2, "data bits")?
                .parse()
                .context("serial data bits must be an unsigned integer")?,
            parity,
            stop_bits: utf8(4, "stop bits")?
                .parse()
                .context("serial stop bits must be an unsigned integer")?,
            flow_control,
        };
        config.validate("internal bridge")?;
        Ok(Self { config })
    }
}

pub(crate) fn bridge_command(
    config: &SerialProfileConfig,
) -> anyhow::Result<(String, Vec<String>)> {
    config.validate("launch")?;
    let executable = std::env::current_exe().context("locate the Termeric executable")?;
    let executable = executable
        .to_str()
        .context("Termeric executable path must be valid UTF-8")?
        .to_string();
    let device = config
        .device
        .to_str()
        .context("serial device path must be valid UTF-8")?;
    Ok((
        executable,
        vec![
            INTERNAL_COMMAND.into(),
            device.into(),
            config.baud.to_string(),
            config.data_bits.to_string(),
            config.parity.as_str().into(),
            config.stop_bits.to_string(),
            config.flow_control.as_str().into(),
        ],
    ))
}

pub(crate) fn is_internal_command(argument: Option<&OsString>) -> bool {
    argument.and_then(|value| value.to_str()) == Some(INTERNAL_COMMAND)
}

pub(crate) fn run_bridge(options: BridgeOptions) -> anyhow::Result<()> {
    let device = ConfiguredDevice::open(&options.config)?;
    let _stdin_mode = crate::raw_terminal::RawTerminal::enter(libc::STDIN_FILENO)
        .context("put the terminal side of the serial bridge into raw mode")?;
    // Unbuffered handles on the bridge's own stdin and stdout, so a poll
    // that reports input always finds it in the descriptor.
    let terminal_input = File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
    let terminal_output = File::from(std::io::stdout().as_fd().try_clone_to_owned()?);
    relay_between(
        &terminal_input,
        &terminal_output,
        device.file(),
        device.file(),
    )
    .with_context(|| {
        format!(
            "serial connection to {} ended",
            options.config.device.display()
        )
    })
}

struct ConfiguredDevice {
    file: File,
    original: libc::termios,
}

impl ConfiguredDevice {
    fn open(config: &SerialProfileConfig) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&config.device)
            .with_context(|| format!("open serial device {}", config.device.display()))?;
        if !file.metadata()?.file_type().is_char_device() {
            anyhow::bail!(
                "serial device {} is not a character device",
                config.device.display()
            );
        }
        let fd = file.as_raw_fd();
        // SAFETY: fd is a live descriptor. TIOCEXCL asks the kernel to
        // reject competing opens until this descriptor closes.
        if unsafe { libc::ioctl(fd, libc::TIOCEXCL) } != 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "claim serial device {} exclusively",
                    config.device.display()
                )
            });
        }
        // SAFETY: F_GETFL/F_SETFL operate on the live owned descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error())
                .context("put the serial device into blocking mode");
        }
        let original = termios(fd).context("read serial device attributes")?;
        configure(fd, original, config).context("configure serial device")?;
        Ok(Self { file, original })
    }

    fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for ConfiguredDevice {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains owned by self while Drop runs.
        if unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original) } != 0 {
            log::warn!(
                "Could not restore serial device attributes: {}",
                io::Error::last_os_error()
            );
        }
    }
}

fn termios(fd: RawFd) -> io::Result<libc::termios> {
    // SAFETY: zeroed termios is valid output storage for tcgetattr.
    let mut value = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: fd is live and value is valid writable storage.
    if unsafe { libc::tcgetattr(fd, &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn configure(fd: RawFd, mut value: libc::termios, config: &SerialProfileConfig) -> io::Result<()> {
    // SAFETY: value points to initialized writable termios storage.
    unsafe { libc::cfmakeraw(&mut value) };
    value.c_cflag |= libc::CLOCAL | libc::CREAD;
    value.c_cflag &= !(libc::CSIZE | libc::PARENB | libc::PARODD | libc::CSTOPB | libc::CRTSCTS);
    value.c_cflag |= match config.data_bits {
        5 => libc::CS5,
        6 => libc::CS6,
        7 => libc::CS7,
        8 => libc::CS8,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid data bits",
            ));
        }
    };
    match config.parity {
        SerialParity::None => {}
        SerialParity::Even => value.c_cflag |= libc::PARENB,
        SerialParity::Odd => value.c_cflag |= libc::PARENB | libc::PARODD,
    }
    if config.stop_bits == 2 {
        value.c_cflag |= libc::CSTOPB;
    }
    value.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    match config.flow_control {
        SerialFlowControl::None => {}
        SerialFlowControl::Software => value.c_iflag |= libc::IXON | libc::IXOFF,
        SerialFlowControl::Hardware => value.c_cflag |= libc::CRTSCTS,
    }
    value.c_cc[libc::VMIN] = 1;
    value.c_cc[libc::VTIME] = 0;
    let speed = baud_constant(config.baud)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsupported baud rate"))?;
    // SAFETY: value is initialized and speed is a platform termios rate.
    if unsafe { libc::cfsetispeed(&mut value, speed) } != 0
        || unsafe { libc::cfsetospeed(&mut value, speed) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is live and value contains validated attributes.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn baud_constant(baud: u32) -> Option<libc::speed_t> {
    Some(match baud {
        50 => libc::B50,
        75 => libc::B75,
        110 => libc::B110,
        134 => libc::B134,
        150 => libc::B150,
        200 => libc::B200,
        300 => libc::B300,
        600 => libc::B600,
        1_200 => libc::B1200,
        1_800 => libc::B1800,
        2_400 => libc::B2400,
        4_800 => libc::B4800,
        9_600 => libc::B9600,
        19_200 => libc::B19200,
        38_400 => libc::B38400,
        57_600 => libc::B57600,
        115_200 => libc::B115200,
        230_400 => libc::B230400,
        460_800 => libc::B460800,
        500_000 => libc::B500000,
        576_000 => libc::B576000,
        921_600 => libc::B921600,
        1_000_000 => libc::B1000000,
        1_152_000 => libc::B1152000,
        1_500_000 => libc::B1500000,
        2_000_000 => libc::B2000000,
        2_500_000 => libc::B2500000,
        3_000_000 => libc::B3000000,
        3_500_000 => libc::B3500000,
        4_000_000 => libc::B4000000,
        _ => return None,
    })
}

fn relay_between(
    mut terminal_input: &File,
    mut terminal_output: &File,
    mut device_input: &File,
    mut device_output: &File,
) -> io::Result<()> {
    let mut buffer = [0u8; RELAY_BUFFER_BYTES];
    loop {
        let mut polls = [
            libc::pollfd {
                fd: terminal_input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: device_input.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: polls is valid for its exact two-element length.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as _, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        // An interrupted read polls again; write_all retries on its own.
        if polls[0].revents & libc::POLLIN != 0 {
            let read = match terminal_input.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                read => read?,
            };
            if read == 0 {
                return Ok(());
            }
            device_output.write_all(&buffer[..read])?;
        }
        if polls[1].revents & libc::POLLIN != 0 {
            let read = match device_input.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                read => read?,
            };
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "serial device disconnected",
                ));
            }
            terminal_output.write_all(&buffer[..read])?;
        }
        if polls[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(());
        }
        if polls[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "serial device disconnected",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_arguments_round_trip_without_shell_parsing() {
        let config = SerialProfileConfig {
            device: PathBuf::from("/dev/ttyUSB0"),
            baud: 115_200,
            data_bits: 7,
            parity: SerialParity::Even,
            stop_bits: 2,
            flow_control: SerialFlowControl::Hardware,
        };
        let (_, arguments) = bridge_command(&config).unwrap();
        assert_eq!(arguments[0], INTERNAL_COMMAND);
        let parsed = BridgeOptions::parse(
            &arguments[1..]
                .iter()
                .map(OsString::from)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(parsed.config, config);
    }

    #[test]
    fn serial_open_rejects_regular_files_before_configuration() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let config = SerialProfileConfig {
            device: temp.path().to_path_buf(),
            ..SerialProfileConfig::default()
        };
        let error = ConfiguredDevice::open(&config)
            .err()
            .expect("regular file must be rejected");
        assert!(error.to_string().contains("not a character device"));
    }

    #[test]
    fn relay_is_binary_transparent_in_both_directions() {
        use std::io::{Read, Write};
        use std::os::fd::{AsRawFd, FromRawFd};

        let pipe = || {
            let mut fds = [-1; 2];
            // SAFETY: fds points to two integers populated by pipe2.
            assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
            // SAFETY: each returned descriptor is uniquely adopted once.
            unsafe {
                (
                    std::fs::File::from_raw_fd(fds[0]),
                    std::fs::File::from_raw_fd(fds[1]),
                )
            }
        };
        let (bridge_terminal_input, mut client_terminal_output) = pipe();
        let (mut client_terminal_input, bridge_terminal_output) = pipe();
        let (bridge_device_input, mut client_device_output) = pipe();
        let (mut client_device_input, bridge_device_output) = pipe();
        let assert_readable = |fd| {
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll points to one initialized pollfd for this call.
            assert_eq!(unsafe { libc::poll(&mut poll, 1, 1_000) }, 1);
            assert_ne!(poll.revents & libc::POLLIN, 0);
        };
        let relay = std::thread::spawn(move || {
            relay_between(
                &bridge_terminal_input,
                &bridge_terminal_output,
                &bridge_device_input,
                &bridge_device_output,
            )
        });

        client_terminal_output
            .write_all(b"to-device\0\xff")
            .unwrap();
        assert_readable(client_device_input.as_raw_fd());
        let mut device_bytes = [0u8; 11];
        client_device_input.read_exact(&mut device_bytes).unwrap();
        assert_eq!(&device_bytes, b"to-device\0\xff");

        client_device_output.write_all(b"to-terminal\x1b").unwrap();
        assert_readable(client_terminal_input.as_raw_fd());
        let mut terminal_bytes = [0u8; 12];
        client_terminal_input
            .read_exact(&mut terminal_bytes)
            .unwrap();
        assert_eq!(&terminal_bytes, b"to-terminal\x1b");

        drop(client_terminal_output);
        relay.join().unwrap().unwrap();
    }
}
