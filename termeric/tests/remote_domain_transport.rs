#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct RemoteHarness {
    binary: PathBuf,
    fake_bin: PathBuf,
    runtime: PathBuf,
    ssh_log: PathBuf,
}

impl RemoteHarness {
    fn command(&self, arguments: &[&str]) -> Output {
        let mut path = OsString::from(&self.fake_bin);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        Command::new(&self.binary)
            .args(arguments)
            .stdin(Stdio::null())
            .env("PATH", path)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("TERMERIC_TEST_BINARY", &self.binary)
            .env("TERMERIC_SSH_LOG", &self.ssh_log)
            .output()
            .unwrap()
    }

    fn successful(&self, arguments: &[&str]) -> Output {
        let output = self.command(arguments);
        assert!(
            output.status.success(),
            "command {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for RemoteHarness {
    fn drop(&mut self) {
        let list = self.command(&["domain", "--host", "test-host", "list", "--json"]);
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&list.stdout)
            && let Some(domains) = value.get("domains").and_then(serde_json::Value::as_array)
        {
            for domain in domains {
                if let Some(id) = domain.get("id").and_then(serde_json::Value::as_str) {
                    let _ = self.command(&["domain", "--host", "test-host", "kill", id]);
                }
            }
        }
        let _ = self.command(&["domain", "--host", "test-host", "stop-server"]);
    }
}

fn harness(root: &Path) -> RemoteHarness {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_termeric"));
    let fake_bin = root.join("bin");
    let runtime = root.join("runtime");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let ssh = fake_bin.join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$TERMERIC_SSH_LOG\"\nexec \"$TERMERIC_TEST_BINARY\" domain proxy-stdio\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    RemoteHarness {
        binary,
        fake_bin,
        runtime,
        ssh_log: root.join("ssh-args.log"),
    }
}

#[test]
fn protocol_version_probe_is_offline_and_machine_readable() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = temporary.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_termeric"))
        .args(["domain", "protocol-version", "--json"])
        .env("XDG_RUNTIME_DIR", &runtime)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["protocol_version"], 1);
    assert!(
        fs::read_dir(&runtime).unwrap().next().is_none(),
        "an offline version probe must not create a daemon socket or token"
    );
}

#[test]
fn fake_ssh_controls_a_remote_daemon_without_network_or_token_export() {
    let temporary = tempfile::tempdir().unwrap();
    let probe = temporary.path().join("socket-probe");
    match std::os::unix::net::UnixListener::bind(&probe) {
        Ok(listener) => drop(listener),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skip remote-domain integration: sandbox denies AF_UNIX");
            return;
        }
        Err(error) => panic!("could not probe Unix-domain socket support: {error}"),
    }
    let harness = harness(temporary.path());
    let remote_cwd = temporary.path().join("remote-cwd");
    fs::create_dir(&remote_cwd).unwrap();
    let create = harness.successful(&[
        "domain",
        "--host",
        "test-host",
        "create",
        "Remote fixture",
        "--cwd",
        remote_cwd.to_str().unwrap(),
        "--json",
        "--",
        "/bin/sh",
        "-c",
        "printf REMOTE_STDIO_RELAY; sleep 3",
    ]);
    let created: serde_json::Value = serde_json::from_slice(&create.stdout).unwrap();
    let domain = created["domain"]["id"].as_str().unwrap().to_string();

    let list = harness.successful(&["domain", "--host", "test-host", "list", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(listed["domains"][0]["id"], domain);
    assert_eq!(listed["domains"][0]["panes"], 1);

    let source = temporary.path().join("local artifact.bin");
    let source_bytes = (0..150_000)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    fs::write(&source, &source_bytes).unwrap();
    let upload = harness.successful(&[
        "domain",
        "--host",
        "test-host",
        "upload",
        &domain,
        "0",
        source.to_str().unwrap(),
        "--name",
        "remote artifact.bin",
        "--json",
    ]);
    let uploaded: serde_json::Value = serde_json::from_slice(&upload.stdout).unwrap();
    assert_eq!(uploaded["result"], "file_uploaded");
    assert_eq!(uploaded["bytes"], source_bytes.len() as u64);
    assert_eq!(
        fs::read(remote_cwd.join("remote artifact.bin")).unwrap(),
        source_bytes
    );
    let directory_source = temporary.path().join("local project");
    fs::create_dir(&directory_source).unwrap();
    fs::create_dir(directory_source.join("empty")).unwrap();
    fs::write(directory_source.join("README.md"), b"remote directory\n").unwrap();
    let directory_upload = harness.successful(&[
        "domain",
        "--host",
        "test-host",
        "upload",
        &domain,
        "0",
        directory_source.to_str().unwrap(),
        "--name",
        "remote project",
        "--json",
    ]);
    let uploaded_directory: serde_json::Value =
        serde_json::from_slice(&directory_upload.stdout).unwrap();
    assert_eq!(uploaded_directory["result"], "directory_uploaded");
    assert_eq!(uploaded_directory["entries"], 2);
    assert_eq!(
        fs::read(remote_cwd.join("remote project/README.md")).unwrap(),
        b"remote directory\n"
    );
    assert!(remote_cwd.join("remote project/empty").is_dir());
    let retained_upload = harness.successful(&[
        "domain",
        "--host",
        "test-host",
        "upload",
        &domain,
        "0",
        source.to_str().unwrap(),
        "--name",
        "retained option.bin",
        "--retain",
        "--json",
    ]);
    let retained_id = String::from_utf8_lossy(&retained_upload.stderr)
        .split_whitespace()
        .nth(3)
        .unwrap()
        .to_string();
    assert!(retained_id.len() == 32 && retained_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let uploads = harness.successful(&["domain", "--host", "test-host", "uploads", "--json"]);
    let uploads: serde_json::Value = serde_json::from_slice(&uploads.stdout).unwrap();
    assert_eq!(uploads["result"], "uploads");
    assert_eq!(uploads["uploads"].as_array().unwrap().len(), 0);
    let completed_cancel = harness.command(&[
        "domain",
        "--host",
        "test-host",
        "cancel-upload",
        &retained_id,
    ]);
    assert!(!completed_cancel.status.success());
    assert!(String::from_utf8_lossy(&completed_cancel.stderr).contains("already complete"));
    let overwrite = harness.command(&[
        "domain",
        "--host",
        "test-host",
        "upload",
        &domain,
        "0",
        source.to_str().unwrap(),
        "--name",
        "remote artifact.bin",
    ]);
    assert!(!overwrite.status.success());
    assert!(String::from_utf8_lossy(&overwrite.stderr).contains("never overwrite"));

    let relay = harness.successful(&["domain", "--host", "test-host", "relay", &domain, "0"]);
    assert!(String::from_utf8_lossy(&relay.stdout).contains("REMOTE_STDIO_RELAY"));

    let live_shutdown = harness.command(&["domain", "--host", "test-host", "stop-server"]);
    assert!(!live_shutdown.status.success());
    assert!(
        String::from_utf8_lossy(&live_shutdown.stderr)
            .contains("refusing to stop the server while live domains exist")
    );
    harness.successful(&["domain", "--host", "test-host", "kill", &domain]);
    harness.successful(&["domain", "--host", "test-host", "stop-server"]);

    let ssh_arguments = fs::read_to_string(&harness.ssh_log).unwrap();
    for required in [
        "-T",
        "-oBatchMode=yes",
        "-oStrictHostKeyChecking=yes",
        "-oClearAllForwardings=yes",
        "--",
        "test-host",
        "termeric",
        "domain",
        "proxy-stdio",
    ] {
        assert!(ssh_arguments.lines().any(|argument| argument == required));
    }
    assert!(!ssh_arguments.contains("token"));
}

#[test]
fn stdio_bridge_rejects_daemon_tokens_before_opening_a_socket() {
    let binary = env!("CARGO_BIN_EXE_termeric");
    let mut child = Command::new(binary)
        .args(["domain", "proxy-stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::to_vec(&serde_json::json!({
        "type": "hello",
        "version": 1,
        "token": "must-not-cross-ssh"
    }))
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(payload.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&payload).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.len() >= 4);
    let length = u32::from_be_bytes(output.stdout[..4].try_into().unwrap()) as usize;
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout[4..4 + length]).unwrap();
    assert_eq!(response["type"], "error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("does not accept daemon tokens")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-cross-ssh"));
}

#[test]
fn ssh_disconnect_before_negotiation_fails_without_hanging() {
    let temporary = tempfile::tempdir().unwrap();
    let fake_bin = temporary.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let ssh = fake_bin.join("ssh");
    fs::write(&ssh, "#!/bin/sh\nexit 23\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let mut path = OsString::from(&fake_bin);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_termeric"))
        .args(["domain", "--host", "test-host", "list"])
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("frame length") || stderr.contains("negotiation"));
}

#[test]
fn saved_host_inventory_and_resolution_use_validated_config_without_network() {
    let temporary = tempfile::tempdir().unwrap();
    let config_dir = temporary.path().join("config");
    let termeric_config_dir = config_dir.join("termeric");
    let fake_bin = temporary.path().join("bin");
    let ssh_log = temporary.path().join("profile-ssh.log");
    fs::create_dir_all(&termeric_config_dir).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        termeric_config_dir.join("config.toml"),
        "[domains.ssh_hosts.build]\ndestination = 'developer@build.example'\ndescription = 'Build farm'\n",
    )
    .unwrap();

    let binary = env!("CARGO_BIN_EXE_termeric");
    let inventory = Command::new(binary)
        .args(["domain", "hosts", "--json"])
        .env("XDG_CONFIG_HOME", &config_dir)
        .output()
        .unwrap();
    assert!(inventory.status.success());
    let inventory: serde_json::Value = serde_json::from_slice(&inventory.stdout).unwrap();
    assert_eq!(inventory["build"]["destination"], "developer@build.example");
    assert_eq!(inventory["build"]["description"], "Build farm");

    let ssh = fake_bin.join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$TERMERIC_SSH_LOG\"\nexit 23\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let mut path = OsString::from(&fake_bin);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let remote = Command::new(binary)
        .args(["domain", "--host-profile", "build", "list"])
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("PATH", path)
        .env("TERMERIC_SSH_LOG", &ssh_log)
        .output()
        .unwrap();
    assert!(!remote.status.success());
    let arguments = fs::read_to_string(&ssh_log).unwrap();
    assert!(
        arguments
            .lines()
            .any(|argument| argument == "developer@build.example")
    );
    assert!(
        arguments
            .lines()
            .any(|argument| argument == "-oStrictHostKeyChecking=yes")
    );

    fs::remove_file(&ssh_log).unwrap();
    let unknown = Command::new(binary)
        .args(["domain", "--host-profile", "missing", "list"])
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("PATH", fake_bin)
        .env("TERMERIC_SSH_LOG", &ssh_log)
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(
        !ssh_log.exists(),
        "unknown profiles must fail before ssh launch"
    );
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown SSH host profile"));
}
