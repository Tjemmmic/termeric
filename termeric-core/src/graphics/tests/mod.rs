fn fifo_fixture(directory: &Path) -> tempfile::TempPath {
    let file = tempfile::Builder::new()
        .prefix("termeric-tty-graphics-protocol-fifo-")
        .tempfile_in(directory)
        .unwrap();
    replace_fixture_with_fifo(file).0
}

fn replace_fixture_with_fifo(file: tempfile::NamedTempFile) -> (tempfile::TempPath, std::fs::File) {
    // Holding the original descriptor prevents inode reuse during rechecks.
    let original = file.as_file().try_clone().unwrap();
    let path = file.into_temp_path();
    // Replace only our exclusively created fixture, retaining RAII cleanup.
    std::fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a live C string in the fixture's selected directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    (path, original)
}

/// A regression must fail rather than hanging the entire test lane. Open a
/// private rescue peer only if the production operation did not return.
fn check_fifo_open_does_not_wait<T: Send + 'static>(
    path: &Path,
    operation: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(operation());
    });
    let (result, waited) = match rx.recv_timeout(std::time::Duration::from_millis(300)) {
        Ok(result) => (result, false),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _rescue = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(path)
                .unwrap();
            (
                rx.recv_timeout(std::time::Duration::from_secs(2))
                    .expect("FIFO regression did not release with a rescue peer"),
                true,
            )
        }
        Err(error) => panic!("FIFO fixture worker failed: {error}"),
    };
    worker.join().unwrap();
    assert!(
        !waited,
        "graphics object open waited for a FIFO peer before type validation"
    );
    result
}

#[test]
fn ordinary_graphics_fifo_is_rejected_without_waiting_for_a_writer() {
    let path = fifo_fixture(&std::env::temp_dir());
    let encoded = path.as_os_str().as_bytes().to_vec();
    let result =
        check_fifo_open_does_not_wait(&path, move || read_ordinary_file(&encoded, 0, Some(4)));
    assert_eq!(result.unwrap_err().code, "EINVAL");
    assert!(path.exists(), "rejected FIFO was deleted");
}

#[test]
fn shared_memory_graphics_fifo_is_rejected_without_waiting_for_a_writer() {
    let path = fifo_fixture(Path::new("/dev/shm"));
    let name = path.file_name().unwrap().as_bytes().to_vec();
    let result = check_fifo_open_does_not_wait(&path, move || {
        read_and_unlink_shared_memory(&name, 0, Some(4))
    });
    assert_eq!(result.unwrap_err().code, "EINVAL");
    assert!(path.exists(), "rejected FIFO was unlinked");
}

#[test]
fn temporary_graphics_open_rejects_fifo_and_symlink_substitution() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let (path, original) = replace_fixture_with_fifo(file);
    let before = original.metadata().unwrap();
    let probe_path = path.to_path_buf();
    let result = check_fifo_open_does_not_wait(&path, move || {
        open_unchanged_temporary_file(&probe_path, &before)
    });
    assert_eq!(result.unwrap_err().code, "EINVAL");
    assert!(path.exists());

    let target = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(target.path(), b"private fixture pixels").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(target.path(), &path).unwrap();
    assert!(open_unchanged_temporary_file(&path, &original.metadata().unwrap()).is_err());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read(target.path()).unwrap(),
        b"private fixture pixels"
    );
}

#[test]
fn shared_memory_cleanup_rejects_substituted_fifo_without_waiting_or_unlinking() {
    let file = tempfile::Builder::new()
        .prefix("termeric-graphics-cleanup-")
        .tempfile_in("/dev/shm")
        .unwrap();
    let (path, original) = replace_fixture_with_fifo(file);
    let before = original.metadata().unwrap();
    let name =
        std::ffi::CString::new([b"/", path.file_name().unwrap().as_bytes()].concat()).unwrap();
    let result = check_fifo_open_does_not_wait(&path, move || {
        unlink_shared_memory_if_unchanged(&name, &before)
    });
    assert_eq!(result.unwrap_err().code, "EINVAL");
    assert!(path.exists(), "cleanup removed a substituted FIFO");
}

#[test]
fn fifo_protocol_errors_preserve_quiet_policy_and_next_image_progress() {
    let path = fifo_fixture(Path::new("/dev/shm"));
    let name = path.file_name().unwrap().as_bytes().to_vec();
    check_fifo_open_does_not_wait(&path, move || {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let mut manager = GraphicsManager::new();
        for quiet in [0, 1, 2] {
            let payload = format!(
                "a=t,t=s,f=32,s=1,v=1,S=4,i=243,q={quiet};{}",
                STANDARD.encode(&name)
            );
            let result = manager.handle_command(command(payload.as_bytes()), false);
            if quiet == 2 {
                assert!(result.response.is_none());
            } else {
                assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
            }
            assert!(manager.get_image(ImageId(243)).is_none());
        }
        let next = manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=244;AQIDBA=="), false);
        assert!(next.response.unwrap().ok);
        assert!(manager.get_image(ImageId(244)).is_some());
    });
    assert!(path.exists());
}
use super::placement_geometry::crop_axis;
use super::transmission::{
    ensure_ordinary_file_unchanged, open_unchanged_temporary_file, read_and_unlink_shared_memory,
    read_ordinary_file, unlink_shared_memory_if_unchanged,
};
use super::*;

fn command(value: &[u8]) -> KittyCommand {
    KittyCommand::parse(value).unwrap()
}

fn raw_command(controls: &str, pixels: &[u8]) -> KittyCommand {
    command(format!("{controls};{}", STANDARD.encode(pixels)).as_bytes())
}

#[test]
fn raw_query_validates_without_storing() {
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(b"a=q,f=32,s=1,v=1,i=7;AQIDBA=="), false);
    assert!(result.response.unwrap().ok);
    assert!(manager.storage.is_empty());
}

#[test]
fn disabled_file_media_and_animations_report_truthfully() {
    let mut manager = GraphicsManager::new();
    let media = manager.handle_command(command(b"a=q,t=f,i=1;L3RtcC9pbWFnZQ=="), false);
    assert_eq!(media.response.unwrap().error_code, Some("EPERM"));
    assert!(!manager.file_transmission_allowed());
    // A temporary file falls under the same policy, so a client probing it
    // (kitten icat) doesn't go on to send ordinary files that are refused.
    let temporary = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    let probe = format!(
        "a=q,t=t,f=24,s=1,v=1,i=2;{}",
        encoded_path(temporary.path())
    );
    let media = manager.handle_command(command(probe.as_bytes()), false);
    assert_eq!(media.response.unwrap().error_code, Some("EPERM"));
    assert!(temporary.path().exists(), "a refused file is left alone");
    let animation = manager.handle_command(command(b"a=f,i=1"), false);
    assert_eq!(animation.response.unwrap().error_code, Some("ENOENT"));
}

#[test]
fn animation_frames_load_edit_blend_replace_and_inherit_a_base() {
    let mut manager = GraphicsManager::new();
    let root = [
        10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
    ];
    manager.handle_command(raw_command("a=t,f=32,s=2,v=2,i=7", &root), false);

    let second = [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
    let loaded = manager.handle_command(raw_command("a=f,f=32,s=2,v=2,i=7,z=75", &second), false);
    let response = loaded.response.unwrap();
    assert!(response.ok);
    assert_eq!(response.frame_number, Some(2));
    assert!(response.to_escape_sequence().contains(",r=2;OK"));
    assert_eq!(manager.animations[&ImageId(7)].frames[0].gap_ms, 75);

    // Transparent source-over leaves the destination unchanged.
    manager.handle_command(
        raw_command("a=f,f=32,s=1,v=1,i=7,r=2,x=1", &[250, 0, 0, 0]),
        false,
    );
    assert_eq!(
        &manager.frame_pixels(ImageId(7), 1).unwrap()[4..8],
        &[4, 5, 6, 255]
    );

    // Replacement mode copies transparent pixels exactly.
    manager.handle_command(
        raw_command("a=f,f=32,s=1,v=1,i=7,r=2,x=1,X=1", &[250, 0, 0, 0]),
        false,
    );
    assert_eq!(
        &manager.frame_pixels(ImageId(7), 1).unwrap()[4..8],
        &[250, 0, 0, 0]
    );

    // A new frame based on frame two preserves that frame outside the
    // transmitted rectangle and receives the default 40 ms gap.
    let inherited = manager.handle_command(
        raw_command(
            "a=f,f=32,s=1,v=1,i=7,c=2,x=0,y=1,X=1",
            &[200, 201, 202, 255],
        ),
        false,
    );
    assert_eq!(inherited.response.unwrap().frame_number, Some(3));
    let third = manager.frame_pixels(ImageId(7), 2).unwrap();
    assert_eq!(
        &third[..8],
        &manager.frame_pixels(ImageId(7), 1).unwrap()[..8]
    );
    assert_eq!(&third[8..12], &[200, 201, 202, 255]);
    assert_eq!(manager.animations[&ImageId(7)].frames[1].gap_ms, 40);

    let colored = manager.handle_command(
        raw_command(
            "a=f,f=32,s=1,v=1,i=7,x=1,y=1,X=1,Y=16909060,z=-1",
            &[9, 8, 7, 6],
        ),
        false,
    );
    assert_eq!(colored.response.unwrap().frame_number, Some(4));
    let fourth = manager.frame_pixels(ImageId(7), 3).unwrap();
    assert_eq!(&fourth[..4], &[1, 2, 3, 4]);
    assert_eq!(&fourth[12..], &[9, 8, 7, 6]);
    assert_eq!(manager.animations[&ImageId(7)].frames[2].gap_ms, 0);
    assert_eq!(manager.memory_usage(), root.len() + second.len() * 3);

    let oversized = manager.handle_command(
        raw_command(
            "a=f,f=32,s=3,v=1,i=7",
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ),
        false,
    );
    assert_eq!(oversized.response.unwrap().error_code, Some("EINVAL"));
    assert_eq!(manager.animations[&ImageId(7)].frame_count(), 4);
}

#[test]
fn animation_frame_chunks_retain_first_command_metadata() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=9", &[0, 0, 0, 255]), false);
    let encoded = STANDARD.encode([10, 20, 30, 255]);
    let split = 4;
    let first = format!("a=f,f=32,s=1,v=1,i=9,z=61,m=1;{}", &encoded[..split]);
    assert!(
        manager
            .handle_command(command(first.as_bytes()), false)
            .response
            .is_none()
    );
    let last = format!("m=0;{}", &encoded[split..]);
    let response = manager
        .handle_command(command(last.as_bytes()), false)
        .response
        .unwrap();
    assert_eq!(response.frame_number, Some(2));
    assert_eq!(manager.animations[&ImageId(9)].frames[0].gap_ms, 61);
    assert_eq!(
        manager.frame_pixels(ImageId(9), 1).unwrap().as_ref(),
        &[10, 20, 30, 255]
    );

    let interrupted = format!(
        "a=f,f=32,s=1,v=1,i=9,m=1;{}",
        &STANDARD.encode([1, 2, 3, 4])[..4]
    );
    manager.handle_command(command(interrupted.as_bytes()), false);
    let error = manager.handle_command(command(b"a=a,i=9,s=1"), false);
    assert_eq!(error.response.unwrap().error_code, Some("EINVAL"));
    assert!(manager.pending_frame.is_none());

    let static_chunk = format!(
        "a=t,f=32,s=1,v=1,i=10,m=1;{}",
        &STANDARD.encode([1, 2, 3, 4])[..4]
    );
    manager.handle_command(command(static_chunk.as_bytes()), false);
    let error = manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=9", &[4, 3, 2, 1]), false);
    assert_eq!(error.response.unwrap().error_code, Some("EINVAL"));
    assert!(manager.pending.is_none());
    assert_eq!(manager.animations[&ImageId(9)].frame_count(), 2);
}

#[test]
fn frame_composition_is_bounded_and_supports_replace_and_source_over() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(
        raw_command("a=t,f=32,s=2,v=1,i=11", &[0, 0, 255, 255, 0, 255, 0, 255]),
        false,
    );
    manager.handle_command(
        raw_command("a=f,f=32,s=2,v=1,i=11,X=1", &[255, 0, 0, 128, 0, 0, 0, 0]),
        false,
    );
    let missing = manager.handle_command(command(b"a=c,f=0,i=11,c=1"), false);
    assert_eq!(missing.response.unwrap().error_code, Some("ENOENT"));
    let composed = manager.handle_command(
        command(b"a=c,f=0,i=11,r=2,c=1,w=1,h=1,X=0,Y=0,x=0,y=0"),
        false,
    );
    assert!(composed.response.unwrap().ok);
    let root = manager.frame_pixels(ImageId(11), 0).unwrap();
    assert_eq!(root[3], 255);
    assert!(root[0] >= 127 && root[2] >= 127);

    manager.handle_command(
        command(b"a=c,f=0,i=11,r=2,c=1,w=1,h=1,X=1,Y=0,x=1,y=0,C=1"),
        false,
    );
    assert_eq!(
        &manager.frame_pixels(ImageId(11), 0).unwrap()[4..8],
        &[0, 0, 0, 0]
    );

    let overlap = manager.handle_command(
        command(b"a=c,f=0,i=11,r=1,c=1,w=1,h=1,X=0,Y=0,x=0,y=0"),
        false,
    );
    assert_eq!(overlap.response.unwrap().error_code, Some("EINVAL"));
    let overflow = manager.handle_command(
        command(b"a=c,f=0,i=11,r=2,c=1,w=2,h=1,X=1,Y=0,x=0,y=0"),
        false,
    );
    assert_eq!(overflow.response.unwrap().error_code, Some("EINVAL"));
}

#[test]
fn animation_control_uses_exact_deadlines_loading_and_finite_loops() {
    let mut manager = GraphicsManager::new();
    let started = Instant::now();
    manager.handle_command_at(
        raw_command("a=T,f=32,s=1,v=1,i=13,c=1,r=1", &[1, 2, 3, 255]),
        false,
        started,
    );
    manager.handle_command_at(
        raw_command("a=f,f=32,s=1,v=1,i=13,z=-1", &[4, 5, 6, 255]),
        false,
        started,
    );
    manager.handle_command_at(
        raw_command("a=f,f=32,s=1,v=1,i=13,z=20", &[7, 8, 9, 255]),
        false,
        started,
    );
    assert!(
        manager
            .handle_command_at(command(b"a=a,i=13,r=1,z=10,s=3,v=3"), false, started)
            .response
            .is_none()
    );
    assert_eq!(
        manager.next_animation_deadline(false),
        Some(started + Duration::from_millis(10))
    );
    assert_eq!(manager.next_animation_deadline(true), None);
    manager.handle_command_at(
        command(b"a=a,i=13,s=3"),
        false,
        started + Duration::from_millis(5),
    );
    assert_eq!(
        manager.next_animation_deadline(false),
        Some(started + Duration::from_millis(10))
    );
    assert!(!manager.advance_animations(false, started + Duration::from_millis(9)));
    assert!(manager.advance_animations(false, started + Duration::from_millis(10)));
    assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
    assert_eq!(
        manager.next_animation_deadline(false),
        Some(started + Duration::from_millis(30))
    );
    assert!(manager.advance_animations(false, started + Duration::from_millis(30)));
    assert_eq!(manager.animations[&ImageId(13)].current_frame, 0);
    assert!(manager.advance_animations(false, started + Duration::from_millis(40)));
    assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
    assert_eq!(manager.next_animation_deadline(false), None);

    manager.handle_command_at(command(b"a=a,i=13,c=1,s=2"), false, started);
    assert!(manager.advance_animations(false, started + Duration::from_millis(10)));
    assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
    assert_eq!(manager.next_animation_deadline(false), None);
}

#[test]
fn frame_deletion_promotes_root_and_uppercase_removes_last_frame_image() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(
        raw_command("a=T,f=32,s=1,v=1,i=15,c=1,r=1", &[1, 1, 1, 255]),
        false,
    );
    manager.handle_command(
        raw_command("a=f,f=32,s=1,v=1,i=15,z=88", &[2, 2, 2, 255]),
        false,
    );
    manager.handle_command(command(b"a=d,d=f,i=15,r=1"), false);
    assert_eq!(
        manager.frame_pixels(ImageId(15), 0).unwrap().as_ref(),
        &[2, 2, 2, 255]
    );
    assert_eq!(manager.animations[&ImageId(15)].root_gap_ms, 88);
    assert_eq!(manager.memory_usage(), 4);
    assert_eq!(manager.image_count(), 1);

    manager.handle_command(command(b"a=d,d=f,i=15"), false);
    assert_eq!(manager.image_count(), 1);
    manager.handle_command(command(b"a=d,d=F,i=15"), false);
    assert_eq!(manager.image_count(), 0);
    assert_eq!(manager.placement_count(), 0);
    assert_eq!(manager.memory_usage(), 0);
}

#[test]
fn animation_frame_count_is_bounded_without_losing_existing_frames() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=17", &[0, 0, 0, 255]), false);
    for value in 1..MAX_ANIMATION_FRAMES_PER_IMAGE {
        let pixel = [value as u8, 0, 0, 255];
        let result = manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=17", &pixel), false);
        assert!(result.response.unwrap().ok);
    }
    assert_eq!(
        manager.animations[&ImageId(17)].frame_count(),
        MAX_ANIMATION_FRAMES_PER_IMAGE
    );
    let rejected =
        manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=17", &[9, 9, 9, 255]), false);
    assert_eq!(rejected.response.unwrap().error_code, Some("ENOSPC"));
    assert_eq!(
        manager.animations[&ImageId(17)].frame_count(),
        MAX_ANIMATION_FRAMES_PER_IMAGE
    );
}

#[test]
fn replacing_an_image_id_releases_all_animation_storage() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=19", &[1, 2, 3, 4]), false);
    manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=19", &[5, 6, 7, 8]), false);
    assert_eq!(manager.memory_usage(), 8);
    assert!(manager.animations.contains_key(&ImageId(19)));

    manager.handle_command(
        raw_command("a=t,f=32,s=1,v=1,i=19", &[9, 10, 11, 12]),
        false,
    );
    assert_eq!(manager.memory_usage(), 4);
    assert!(!manager.animations.contains_key(&ImageId(19)));
}

#[test]
fn animation_frames_use_the_supported_temporary_file_medium() {
    use std::io::Write;

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=21", &[1, 2, 3, 4]), false);
    let mut file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-animation-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    file.write_all(&[5, 6, 7, 8]).unwrap();
    file.flush().unwrap();
    let path = file.path().to_path_buf();
    let payload = format!("a=f,t=t,f=32,s=1,v=1,i=21;{}", encoded_path(&path));
    let response = manager
        .handle_command(command(payload.as_bytes()), false)
        .response
        .unwrap();
    assert!(response.ok);
    assert_eq!(response.frame_number, Some(2));
    assert_eq!(
        manager.frame_pixels(ImageId(21), 1).unwrap().as_ref(),
        &[5, 6, 7, 8]
    );
    assert!(!path.exists());
}

fn encoded_path(path: &std::path::Path) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::os::unix::ffi::OsStrExt;

    STANDARD.encode(path.as_os_str().as_bytes())
}

struct TestSharedMemory {
    name: std::ffi::CString,
    file: std::fs::File,
}

impl TestSharedMemory {
    fn new(data: &[u8]) -> Self {
        use std::io::Write;
        use std::os::unix::io::FromRawFd;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_NAME: AtomicU64 = AtomicU64::new(1);
        loop {
            let sequence = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
            let name = std::ffi::CString::new(format!(
                "/termeric-kitty-test-{}-{sequence}",
                std::process::id()
            ))
            .unwrap();
            // SAFETY: `name` is a live C string, creation uses private
            // permissions, and a successful descriptor is transferred
            // exactly once into `File` ownership below.
            let descriptor = unsafe {
                libc::shm_open(
                    name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if descriptor < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                    continue;
                }
                panic!(
                    "could not create test shared memory: {}",
                    std::io::Error::last_os_error()
                );
            }
            // SAFETY: this is the sole owner transfer for the newly
            // returned descriptor.
            let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
            file.set_len(data.len() as u64).unwrap();
            let mut writer = &file;
            writer.write_all(data).unwrap();
            writer.flush().unwrap();
            return Self { name, file };
        }
    }

    fn encoded_name(&self) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        STANDARD.encode(self.name.as_bytes())
    }

    fn encoded_bare_name(&self) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        STANDARD.encode(&self.name.as_bytes()[1..])
    }

    fn is_linked(&self) -> bool {
        use std::os::unix::io::FromRawFd;

        // SAFETY: `name` is a live C string and any successful descriptor
        // is immediately transferred to `File` for closure.
        let descriptor =
            unsafe { libc::shm_open(self.name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) };
        if descriptor < 0 {
            false
        } else {
            // SAFETY: this is the sole owner transfer for the descriptor.
            drop(unsafe { std::fs::File::from_raw_fd(descriptor) });
            true
        }
    }
}

impl Drop for TestSharedMemory {
    fn drop(&mut self) {
        // SAFETY: `name` remains a live C string. Failure is expected when
        // the graphics handler already performed the required unlink.
        unsafe {
            libc::shm_unlink(self.name.as_ptr());
        }
    }
}

#[test]
fn temporary_file_range_is_loaded_deleted_and_queryable() {
    use std::io::Write;

    let mut image_file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    image_file
        .write_all(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8])
        .unwrap();
    image_file.flush().unwrap();
    let image_path = image_file.path().to_path_buf();
    let payload = format!(
        "a=T,t=t,f=32,s=2,v=1,S=8,O=4,i=41,c=2,r=1,C=1;{}",
        encoded_path(&image_path)
    );

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!image_path.exists());
    assert_eq!(
        manager.get_image(ImageId(41)).unwrap().data.as_ref(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );

    let mut query_file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    query_file.write_all(&[9, 8, 7, 6]).unwrap();
    query_file.flush().unwrap();
    let query_path = query_file.path().to_path_buf();
    let query = format!(
        "a=q,t=t,f=32,s=1,v=1,S=4,i=42;{}",
        encoded_path(&query_path)
    );
    let result = manager.handle_command(command(query.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!query_path.exists());
    assert!(manager.get_image(ImageId(42)).is_none());

    let mut unsupported_file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    unsupported_file.write_all(&[1, 2, 3, 4]).unwrap();
    unsupported_file.flush().unwrap();
    let unsupported_path = unsupported_file.path().to_path_buf();
    let unsupported = format!(
        "a=q,t=t,f=32,s=1,v=1,S=4,o=z,i=48;{}",
        encoded_path(&unsupported_path)
    );
    let result = manager.handle_command(command(unsupported.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(!unsupported_path.exists());
}

#[test]
fn opted_in_ordinary_file_range_is_loaded_without_deletion() {
    use std::io::Write;

    let mut image_file = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    image_file
        .write_all(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8])
        .unwrap();
    image_file.flush().unwrap();
    let image_path = image_file.path().to_path_buf();
    let payload = format!(
        "a=T,t=f,f=32,s=2,v=1,S=8,O=4,i=59,c=2,r=1,C=1;{}",
        encoded_path(&image_path)
    );

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(manager.file_transmission_allowed());
    assert!(image_path.exists());
    assert_eq!(
        manager.get_image(ImageId(59)).unwrap().data.as_ref(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );

    let query = format!(
        "a=q,t=f,f=32,s=1,v=1,S=4,O=4,i=60;{}",
        encoded_path(&image_path)
    );
    let result = manager.handle_command(command(query.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(image_path.exists());
    assert!(manager.get_image(ImageId(60)).is_none());
}

#[test]
fn opted_in_ordinary_files_follow_safe_symlinks_but_reject_unsafe_targets() {
    use std::io::Write;

    let mut target = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    target.write_all(&[1, 2, 3, 4]).unwrap();
    target.flush().unwrap();
    let directory = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let link = directory.path().join("image-link");
    std::os::unix::fs::symlink(target.path(), &link).unwrap();

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let payload = format!("a=q,t=f,f=32,s=1,v=1,S=4,i=61;{}", encoded_path(&link));
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(link.exists());
    assert!(target.path().exists());

    let relative = command(b"a=q,t=f,f=32,s=1,v=1,S=4,i=62;aW1hZ2U=");
    let result = manager.handle_command(relative, false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));

    let directory_payload = format!(
        "a=q,t=f,f=32,s=1,v=1,S=4,i=63;{}",
        encoded_path(directory.path())
    );
    let result = manager.handle_command(command(directory_payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));

    let other = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    let replacement =
        ensure_ordinary_file_unchanged(target.path(), &other.as_file().metadata().unwrap())
            .unwrap_err();
    assert_eq!(replacement.code, "EINVAL");

    if Path::new("/proc/self/status").exists() {
        let protected = format!(
            "a=q,t=f,f=32,s=1,v=1,S=4,i=64;{}",
            encoded_path(Path::new("/proc/self/status"))
        );
        let result = manager.handle_command(command(protected.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EPERM"));
    }
}

#[test]
fn oversized_ordinary_file_is_rejected_without_deletion() {
    let file = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    file.as_file()
        .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
        .unwrap();
    let path = file.path().to_path_buf();
    let payload = format!("a=q,t=f,f=32,s=1,v=1,i=65;{}", encoded_path(&path));

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
    assert!(path.exists());
    assert!(manager.storage.is_empty());
}

#[test]
fn local_media_policy_rejects_unsafe_paths_and_special_files() {
    let ordinary = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    let ordinary_path = ordinary.path().to_path_buf();
    let payload = format!("a=q,t=f,f=32,s=1,v=1,i=43;{}", encoded_path(&ordinary_path));
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EPERM"));
    assert!(ordinary_path.exists());
    manager.set_file_transmission_allowed(true);

    let unmarked = tempfile::Builder::new()
        .prefix("termeric-image-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    let unmarked_path = unmarked.path().to_path_buf();
    let payload = format!("a=q,t=t,f=32,s=1,v=1,i=44;{}", encoded_path(&unmarked_path));
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(unmarked_path.exists());

    let directory = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    let directory_path = directory.path().to_path_buf();
    let payload = format!(
        "a=q,t=t,f=32,s=1,v=1,i=45;{}",
        encoded_path(&directory_path)
    );
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(directory_path.exists());

    let target = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    let symlink_seed = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    let symlink_path = symlink_seed.path().to_path_buf();
    drop(symlink_seed);
    std::os::unix::fs::symlink(target.path(), &symlink_path).unwrap();
    let payload = format!("a=q,t=t,f=32,s=1,v=1,i=46;{}", encoded_path(&symlink_path));
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(symlink_path.exists());
    std::fs::remove_file(&symlink_path).unwrap();
}

#[test]
fn oversized_safe_temporary_file_is_rejected_and_removed() {
    let file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    file.as_file()
        .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
        .unwrap();
    let path = file.path().to_path_buf();
    let payload = format!("a=q,t=t,f=32,s=1,v=1,i=47;{}", encoded_path(&path));

    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
    assert!(!path.exists());
    assert!(manager.storage.is_empty());
}

#[test]
fn shared_memory_range_is_loaded_unlinked_and_queryable() {
    let image = TestSharedMemory::new(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8]);
    let payload = format!(
        "a=T,t=s,f=32,s=2,v=1,S=8,O=4,i=49,c=2,r=1,C=1;{}",
        image.encoded_name()
    );
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!image.is_linked());
    assert_eq!(
        manager.get_image(ImageId(49)).unwrap().data.as_ref(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );

    let query = TestSharedMemory::new(&[9, 8, 7, 6]);
    let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=50;{}", query.encoded_name());
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!query.is_linked());
    assert!(manager.get_image(ImageId(50)).is_none());

    let unsupported = TestSharedMemory::new(&[1, 2, 3, 4]);
    let payload = format!(
        "a=q,t=s,f=32,s=1,v=1,S=4,o=z,i=51;{}",
        unsupported.encoded_name()
    );
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(!unsupported.is_linked());
}

#[test]
fn current_kitty_bare_shared_memory_name_is_normalized_and_unlinked() {
    let image = TestSharedMemory::new(&[1, 2, 3, 4]);
    let payload = format!(
        "a=T,t=s,f=32,s=1,v=1,S=4,i=67,c=1,r=1,C=1;{}",
        image.encoded_bare_name()
    );

    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!image.is_linked());
    assert_eq!(
        manager.get_image(ImageId(67)).unwrap().data.as_ref(),
        [1, 2, 3, 4]
    );
}

#[test]
fn compressed_local_media_uses_format_specific_size_semantics() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    let raw_pixels = [1, 2, 3, 4, 5, 6, 7, 8];
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(&raw_pixels).unwrap();
    let compressed_raw = compressor.finish().unwrap();
    let mut raw_file = tempfile::Builder::new()
        .prefix("tty-graphics-protocol-")
        .tempfile_in(std::env::temp_dir())
        .unwrap();
    raw_file.write_all(&compressed_raw).unwrap();
    raw_file.flush().unwrap();
    let raw_path = raw_file.path().to_path_buf();
    let payload = format!(
        "a=t,t=t,f=32,s=2,v=1,S={},o=z,i=55;{}",
        compressed_raw.len(),
        encoded_path(&raw_path)
    );
    let mut manager = GraphicsManager::new();
    manager.set_file_transmission_allowed(true);
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!raw_path.exists());
    assert_eq!(
        manager.get_image(ImageId(55)).unwrap().data.as_ref(),
        raw_pixels
    );

    let mut ordinary_raw = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
    ordinary_raw.write_all(&compressed_raw).unwrap();
    ordinary_raw.flush().unwrap();
    let ordinary_path = ordinary_raw.path().to_path_buf();
    let payload = format!(
        "a=t,t=f,f=32,s=2,v=1,S={},o=z,i=66;{}",
        compressed_raw.len(),
        encoded_path(&ordinary_path)
    );
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(ordinary_path.exists());
    assert_eq!(
        manager.get_image(ImageId(66)).unwrap().data.as_ref(),
        raw_pixels
    );

    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[9, 8, 7, 6]).unwrap();
    }
    png_data.extend(std::iter::repeat_n(0, 4_096));
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::best());
    compressor.write_all(&png_data).unwrap();
    let compressed_png = compressor.finish().unwrap();
    assert!(compressed_png.len() < png_data.len());
    let png_object = TestSharedMemory::new(&compressed_png);
    let payload = format!(
        "a=t,t=s,f=100,S={},o=z,i=56;{}",
        png_data.len(),
        png_object.encoded_name()
    );
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
    assert!(!png_object.is_linked());
    assert_eq!(
        manager.get_image(ImageId(56)).unwrap().data.as_ref(),
        [9, 8, 7, 6]
    );

    let oversized = TestSharedMemory::new(&[0]);
    let payload = format!(
        "a=q,t=s,f=100,S={},o=z,i=57;{}",
        MAX_TRANSMISSION_BYTES as u64 + 1,
        oversized.encoded_name()
    );
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
    assert!(!oversized.is_linked());

    let direct_png = STANDARD.encode(compressed_png);
    let payload = format!("a=q,f=100,S={},o=z,i=58;{direct_png}", png_data.len());
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert!(result.response.unwrap().ok);
}

#[test]
fn shared_memory_policy_rejects_public_and_invalid_names() {
    use std::os::unix::fs::PermissionsExt;

    let public = TestSharedMemory::new(&[1, 2, 3, 4]);
    public
        .file
        .set_permissions(std::fs::Permissions::from_mode(0o644))
        .unwrap();
    let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=52;{}", public.encoded_name());
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    assert!(public.is_linked());

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let invalid = STANDARD.encode(b"/nested/name");
    let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=53;{invalid}");
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
}

#[test]
fn oversized_private_shared_memory_is_rejected_and_unlinked() {
    let object = TestSharedMemory::new(&[]);
    object
        .file
        .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
        .unwrap();
    let payload = format!("a=q,t=s,f=32,s=1,v=1,i=54;{}", object.encoded_name());

    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(payload.as_bytes()), false);
    assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
    assert!(!object.is_linked());
    assert!(manager.storage.is_empty());
}

#[test]
fn chunk_continuation_without_repeated_id_works() {
    let mut manager = GraphicsManager::new();
    assert!(
        manager
            .handle_command(command(b"a=t,f=32,s=2,v=1,i=3,m=1;AQID"), false)
            .response
            .is_none()
    );
    let result = manager.handle_command(command(b"m=0;BAUGBwg="), false);
    assert!(result.response.unwrap().ok);
    assert_eq!(manager.get_image(ImageId(3)).unwrap().data.len(), 8);
}

#[test]
fn quiet_policy_and_placement_replacement_are_enforced() {
    let mut manager = GraphicsManager::new();
    let result =
        manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=2,p=4,q=1,C=1;AQIDBA=="), false);
    assert!(result.response.is_none());
    manager.handle_command(command(b"a=p,i=2,p=4,C=1"), false);
    assert_eq!(manager.placements(false).count(), 1);
}

#[test]
fn png_and_zlib_are_decoded_within_bounds() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[1, 2, 3, 4]).unwrap();
    }
    let encoded = STANDARD.encode(&png_bytes);
    let mut manager = GraphicsManager::new();
    let payload = format!("a=t,f=100,i=8;{encoded}");
    assert!(
        manager
            .handle_command(command(payload.as_bytes()), false)
            .response
            .unwrap()
            .ok
    );
    assert_eq!(
        manager.get_image(ImageId(8)).unwrap().data.as_ref(),
        [1, 2, 3, 4]
    );

    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(&[9, 8, 7, 6]).unwrap();
    let encoded = STANDARD.encode(compressor.finish().unwrap());
    let payload = format!("a=q,f=32,s=1,v=1,o=z,i=9;{encoded}");
    assert!(
        manager
            .handle_command(command(payload.as_bytes()), false)
            .response
            .unwrap()
            .ok
    );
}

#[test]
fn checked_dimensions_reject_overflow_before_allocation() {
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(b"a=q,f=32,s=4294967295,v=4294967295,i=1;"), false);
    assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
}

#[test]
fn anonymous_images_coexist_and_retransmission_invalidates_content() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(b"a=T,f=32,s=1,v=1,C=1;AQIDBA=="), false);
    manager.handle_command(command(b"a=T,f=32,s=1,v=1,C=1;BQYHCA=="), false);
    assert_eq!(manager.placements(false).count(), 2);

    manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=4,C=1;AQIDBA=="), false);
    let first_revision = manager.get_image(ImageId(4)).unwrap().revision;
    manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=4,C=1;BQYHCA=="), false);
    assert!(manager.get_image(ImageId(4)).unwrap().revision > first_revision);
    assert_eq!(
        manager
            .placements(false)
            .filter(|placement| placement.image_id == ImageId(4))
            .count(),
        1
    );
}

#[test]
fn unicode_virtual_placements_are_invisible_bounded_prototypes() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;AQIDBA=="), false);
    let placed = manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=2,r=3"), false);
    assert_eq!(placed.cursor_movement, None);
    assert!(placed.response.unwrap().ok);
    assert_eq!(manager.placements(false).count(), 0);
    assert_eq!(manager.placement_count(), 1);

    // Coordinate and all-visible deletion selectors cannot affect an
    // invisible virtual prototype.
    manager.handle_command(command(b"a=d,d=a"), false);
    manager.handle_command(command(b"a=d,d=z,z=-1"), false);
    assert_eq!(manager.virtual_placements.len(), 1);

    // Reusing an explicit placement ID updates rather than duplicates the
    // virtual placement, while anonymous prototypes remain independently
    // bounded.
    manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=4,r=5"), false);
    assert_eq!(manager.virtual_placements.len(), 1);
    assert_eq!(manager.virtual_placements[0].columns, Some(4));
    manager.handle_command(command(b"a=d,d=i,i=42,p=7"), false);
    assert!(manager.virtual_placements.is_empty());
    assert!(manager.get_image(ImageId(42)).is_some());

    for placement_id in 1..=MAX_VIRTUAL_IMAGE_PLACEMENTS + 1 {
        let payload = format!("a=p,U=1,i=42,p={placement_id},c=1,r=1");
        manager.handle_command(command(payload.as_bytes()), false);
    }
    assert_eq!(
        manager.virtual_placements.len(),
        MAX_VIRTUAL_IMAGE_PLACEMENTS
    );
    assert_eq!(
        manager.virtual_placements.front().unwrap().placement_id,
        Some(2)
    );

    manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;BQYHCA=="), false);
    assert!(manager.virtual_placements.is_empty());
}

#[test]
fn unicode_virtual_placements_are_pruned_with_storage_eviction() {
    let mut manager = GraphicsManager::new();
    manager.storage = ImageStorage::new(1, 1024);
    manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;AQIDBA=="), false);
    manager.handle_command(command(b"a=p,U=1,i=42,c=1,r=1"), false);
    assert_eq!(manager.virtual_placements.len(), 1);
    manager.handle_command(command(b"a=t,i=43,f=32,s=1,v=1;BQYHCA=="), false);
    assert!(manager.get_image(ImageId(42)).is_none());
    assert!(manager.virtual_placements.is_empty());
}

#[test]
fn unicode_virtual_placements_follow_number_and_range_deletion() {
    let mut manager = GraphicsManager::new();
    let numbered = manager.handle_command(command(b"a=t,I=13,f=32,s=1,v=1;AQIDBA=="), false);
    let numbered_id = ImageId(numbered.response.unwrap().image_id);
    manager.handle_command(command(b"a=p,U=1,I=13,p=7,c=1,r=1"), false);
    manager.handle_command(command(b"a=d,d=n,I=13,p=7"), false);
    assert!(manager.virtual_placements.is_empty());
    assert!(manager.get_image(numbered_id).is_some());
    manager.handle_command(command(b"a=p,U=1,I=13,p=7,c=1,r=1"), false);
    manager.handle_command(command(b"a=d,d=N,I=13,p=7"), false);
    assert!(manager.get_image(numbered_id).is_none());

    for id in [40, 41] {
        let payload = format!("a=t,i={id},f=32,s=1,v=1;AQIDBA==");
        manager.handle_command(command(payload.as_bytes()), false);
        let payload = format!("a=p,U=1,i={id},c=1,r=1");
        manager.handle_command(command(payload.as_bytes()), false);
    }
    manager.handle_command(command(b"a=d,d=R,x=40,y=40"), false);
    assert!(manager.get_image(ImageId(40)).is_none());
    assert!(manager.get_image(ImageId(41)).is_some());
    assert_eq!(manager.virtual_placements.len(), 1);
    assert_eq!(manager.virtual_placements[0].image_id, ImageId(41));
}

#[test]
fn unicode_placeholder_presentations_fit_and_crop_each_visible_cell() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(
        command(b"a=t,i=42,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA=="),
        false,
    );
    manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=2,r=2"), false);
    let mut grid = Grid::new(3, 2);
    for row in 0..2 {
        for col in 0..2 {
            let cell = grid.cell_mut(row, col).unwrap();
            cell.set_grapheme(match (row, col) {
                (0, 0) => "\u{10eeee}\u{0305}\u{0305}",
                (0, 1) => "\u{10eeee}",
                (1, 0) => "\u{10eeee}\u{030d}",
                _ => "\u{10eeee}",
            });
            cell.fg_spec = crate::grid::ColorSpec::Indexed(42);
            cell.underline_color = Some(crate::grid::Color::rgb(1, 2, 3));
            cell.underline_color_spec = Some(crate::grid::ColorSpec::Indexed(7));
        }
    }

    let presentations = manager.presentation_images(&grid, false, 1, 1);
    assert_eq!(presentations.len(), 2);
    for (index, presentation) in presentations.iter().enumerate() {
        assert_eq!(presentation.placement.placement_id, Some(7));
        assert_eq!(presentation.placement.z_index, -1);
        assert_eq!(presentation.pixel_rect, Some([0.0, 0.0, 2.0, 1.0]));
        let row = index as f32;
        assert_eq!(
            presentation.source_uv,
            Some([0.0, row / 2.0, 1.0, (row + 1.0) / 2.0])
        );
    }

    manager.handle_command(command(b"a=p,U=1,i=42,p=8,c=103,r=52"), false);
    let mut current_client_grid = Grid::new(103, 52);
    for row in 0..52 {
        for col in 0..103 {
            let cell = current_client_grid.cell_mut(row, col).unwrap();
            cell.set_grapheme(if col == 0 {
                "\u{10eeee}\u{0305}\u{0305}"
            } else {
                "\u{10eeee}"
            });
            cell.fg_spec = crate::grid::ColorSpec::Indexed(42);
            cell.underline_color = Some(crate::grid::Color::rgb(1, 2, 3));
            cell.underline_color_spec = Some(crate::grid::ColorSpec::Indexed(8));
        }
    }
    let current_client = manager.presentation_images(&current_client_grid, false, 10, 20);
    assert_eq!(current_client.len(), 52);
    assert_eq!(current_client.first().unwrap().placement.row, 0);
    assert_eq!(current_client.last().unwrap().placement.row, 51);
    assert!(
        current_client
            .iter()
            .all(|image| image.placement.width == Some(103))
    );

    let prototype = VirtualImagePlacement {
        uid: PlacementUid(1),
        image_id: ImageId(42),
        placement_id: None,
        columns: Some(1),
        rows: Some(1),
        alternate_screen: false,
    };
    let letterboxed = placeholder_presentation(
        manager.get_image(ImageId(42)).unwrap(),
        &prototype,
        0,
        0,
        0,
        0,
        2,
        4,
    )
    .unwrap();
    assert_eq!(letterboxed.pixel_rect, Some([0.0, 1.0, 2.0, 2.0]));
    assert_eq!(letterboxed.source_uv, Some([0.0, 0.0, 1.0, 1.0]));
}

#[test]
fn relative_placements_follow_named_parents_and_delete_as_groups() {
    let mut manager = GraphicsManager::new();
    for image_id in 1..=3 {
        let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
        assert!(
            manager
                .handle_command(command(payload.as_bytes()), false)
                .response
                .unwrap()
                .ok
        );
    }
    let mut parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
    parent.placement.col = Some(2);
    parent.placement.row = Some(3);
    manager.handle_command(parent, false);

    let child = manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1,V=2,c=1,r=1"), false);
    assert_eq!(child.cursor_movement, None);
    assert!(child.response.unwrap().ok);
    assert!(
        manager
            .handle_command(command(b"a=p,i=3,p=3,P=2,Q=2,H=4,V=-1,c=1,r=1"), false,)
            .response
            .unwrap()
            .ok
    );

    let grid = Grid::new(12, 8);
    let positions = |manager: &GraphicsManager| {
        manager
            .presentation_images(&grid, false, 1, 1)
            .into_iter()
            .map(|image| {
                (
                    image.placement.image_id,
                    image.placement.col,
                    image.placement.row,
                )
            })
            .collect::<HashSet<_>>()
    };
    assert_eq!(
        positions(&manager),
        HashSet::from([(ImageId(1), 2, 3), (ImageId(2), 1, 5), (ImageId(3), 5, 4),])
    );

    let mut moved_parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
    moved_parent.placement.col = Some(6);
    moved_parent.placement.row = Some(1);
    manager.handle_command(moved_parent, false);
    assert_eq!(
        positions(&manager),
        HashSet::from([(ImageId(1), 6, 1), (ImageId(2), 5, 3), (ImageId(3), 9, 2),])
    );

    let cycle = manager.handle_command(command(b"a=p,i=1,p=1,P=3,Q=3,c=1,r=1,C=1"), false);
    assert_eq!(cycle.response.unwrap().error_code, Some("ECYCLE"));
    let missing = manager.handle_command(command(b"a=p,i=1,p=9,P=99,Q=1"), false);
    assert_eq!(missing.response.unwrap().error_code, Some("ENOPARENT"));
    assert!(positions(&manager).contains(&(ImageId(1), 6, 1)));

    manager.handle_command(command(b"a=d,d=i,i=2,p=2"), false);
    assert_eq!(positions(&manager), HashSet::from([(ImageId(1), 6, 1)]));
    assert!(manager.get_image(ImageId(2)).is_some());
    assert!(manager.get_image(ImageId(3)).is_none());
}

#[test]
fn relative_placements_clip_negative_columns_and_reject_virtual_children() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
    manager.handle_command(command(b"a=t,i=2,f=32,s=2,v=1;AQIDBAUGBwg="), false);
    manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
    manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1,c=2,r=1"), false);
    let grid = Grid::new(4, 2);
    let child = manager
        .presentation_images(&grid, false, 1, 1)
        .into_iter()
        .find(|image| image.placement.image_id == ImageId(2))
        .unwrap();
    assert_eq!((child.placement.col, child.placement.width), (0, Some(2)));
    assert_eq!(child.pixel_rect, Some([-1.0, 0.0, 2.0, 1.0]));
    assert_eq!(child.source_uv, Some([0.0, 0.0, 1.0, 1.0]));

    manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1"), false);
    let natural = manager
        .presentation_images(&grid, false, 1, 1)
        .into_iter()
        .find(|image| image.placement.image_id == ImageId(2))
        .unwrap();
    assert_eq!(natural.placement.width, Some(1));
    assert_eq!(natural.placement.pixel_width, Some(2));
    assert_eq!(natural.pixel_rect, Some([-1.0, 0.0, 2.0, 1.0]));

    let invalid = manager.handle_command(command(b"a=p,i=2,p=3,U=1,P=1,Q=1,c=1,r=1"), false);
    assert_eq!(invalid.response.unwrap().error_code, Some("EINVAL"));
}

#[test]
fn relative_placement_depth_is_bounded_above_the_required_minimum() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
    manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
    for placement_id in 2..=MAX_RELATIVE_PLACEMENT_DEPTH as u32 + 1 {
        let payload = format!(
            "a=p,i=1,p={placement_id},P=1,Q={},H=1,c=1,r=1",
            placement_id - 1
        );
        assert!(
            manager
                .handle_command(command(payload.as_bytes()), false)
                .response
                .unwrap()
                .ok
        );
    }
    let too_deep = format!(
        "a=p,i=1,p={},P=1,Q={},H=1,c=1,r=1",
        MAX_RELATIVE_PLACEMENT_DEPTH + 2,
        MAX_RELATIVE_PLACEMENT_DEPTH + 1
    );
    let response = manager
        .handle_command(command(too_deep.as_bytes()), false)
        .response
        .unwrap();
    assert_eq!(response.error_code, Some("ETOODEEP"));
}

#[test]
fn relative_placements_follow_scrolling_and_screen_ownership() {
    let mut manager = GraphicsManager::new();
    for image_id in 1..=2 {
        let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
        manager.handle_command(command(payload.as_bytes()), false);
    }
    let mut parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
    parent.placement.col = Some(2);
    parent.placement.row = Some(3);
    manager.handle_command(parent, false);
    manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=1,V=1,c=1,r=1"), false);
    let grid = Grid::new(8, 6);
    manager.scroll_up(false, 0..6, 0..8, 1, true, 8);
    let child = manager
        .presentation_images(&grid, false, 1, 1)
        .into_iter()
        .find(|image| image.placement.image_id == ImageId(2))
        .unwrap();
    assert_eq!((child.placement.col, child.placement.row), (3, 3));

    let cross_screen = manager.handle_command(command(b"a=p,i=2,p=9,P=1,Q=1,c=1,r=1"), true);
    assert_eq!(cross_screen.response.unwrap().error_code, Some("ENOPARENT"));

    manager.clear_screen(false);
    assert!(manager.presentation_images(&grid, false, 1, 1).is_empty());
}

#[test]
fn relative_descendants_are_pruned_when_the_parent_hits_the_placement_cap() {
    let mut manager = GraphicsManager::new();
    for image_id in 1..=2 {
        let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
        manager.handle_command(command(payload.as_bytes()), false);
    }
    manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
    manager.handle_command(command(b"a=p,i=2,p=1,P=1,Q=1,c=1,r=1,C=1"), false);
    for placement_id in 2..=MAX_IMAGE_PLACEMENTS as u32 {
        let payload = format!("a=p,i=1,p={placement_id},c=1,r=1,C=1");
        manager.handle_command(command(payload.as_bytes()), false);
    }
    assert!(manager.placement_count() <= MAX_IMAGE_PLACEMENTS);
    assert!(manager.get_image(ImageId(2)).is_none());
    assert!(
        manager
            .placements(false)
            .all(|placement| placement.image_id != ImageId(2))
    );
}

#[test]
fn image_numbers_allocate_ids_resolve_newest_and_fall_back_after_deletion() {
    let mut manager = GraphicsManager::new();
    let first = manager.handle_command(command(b"a=t,I=13,N=1,f=32,s=1,v=1;AQIDBA=="), false);
    let first = first.response.unwrap();
    assert!(first.ok);
    assert_eq!(first.image_id, 1);
    assert_eq!(first.image_number, Some(13));
    assert_eq!(first.to_escape_sequence(), "\x1b_Gi=1,I=13;OK\x1b\\");

    let second = manager.handle_command(command(b"a=t,I=13,f=32,s=1,v=1;BQYHCA=="), false);
    let second = second.response.unwrap();
    assert_eq!(second.image_id, 2);
    assert_eq!(manager.image_count(), 2);
    assert_eq!(manager.newest_image_with_number(13), Some(ImageId(2)));

    let displayed = manager.handle_command(command(b"a=p,I=13,p=7,c=1,r=1,C=1"), false);
    let displayed = displayed.response.unwrap();
    assert_eq!((displayed.image_id, displayed.image_number), (2, Some(13)));
    assert_eq!(
        manager.placements(false).next().unwrap().image_id,
        ImageId(2)
    );

    manager.handle_command(command(b"a=d,d=N,I=13,p=7"), false);
    assert!(manager.get_image(ImageId(2)).is_none());
    assert_eq!(manager.newest_image_with_number(13), Some(ImageId(1)));

    manager.handle_command(command(b"a=p,I=13,p=8,c=1,r=1,C=1"), false);
    assert_eq!(
        manager.placements(false).next().unwrap().image_id,
        ImageId(1)
    );

    // Reusing an explicit ID replaces the old image and removes its
    // previous number association.
    manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;CQoLDA=="), false);
    assert_eq!(manager.newest_image_with_number(13), None);
    let missing = manager.handle_command(command(b"a=p,I=13,c=1,r=1,C=1"), false);
    assert_eq!(missing.response.unwrap().error_code, Some("ENOENT"));
}

#[test]
fn numbered_chunking_preserves_allocated_response_identity() {
    let mut manager = GraphicsManager::new();
    let first = manager.handle_command(command(b"a=t,I=44,f=32,s=2,v=1,m=1;AQID"), false);
    assert!(first.response.is_none());
    let final_chunk = manager.handle_command(command(b"m=0;BAUGBwg="), false);
    let response = final_chunk.response.unwrap();
    assert_eq!((response.image_id, response.image_number), (1, Some(44)));
    assert_eq!(response.to_escape_sequence(), "\x1b_Gi=1,I=44;OK\x1b\\");
    assert_eq!(manager.newest_image_with_number(44), Some(ImageId(1)));
}

#[test]
fn image_number_indices_follow_id_allocation_and_storage_eviction() {
    let mut manager = GraphicsManager::new();
    manager.storage = ImageStorage::new(2, 1024);
    manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
    let numbered = manager.handle_command(command(b"a=t,I=20,f=32,s=1,v=1;BQYHCA=="), false);
    assert_eq!(numbered.response.unwrap().image_id, 2);
    manager.handle_command(command(b"a=t,I=30,f=32,s=1,v=1;CQoLDA=="), false);
    assert_eq!(manager.image_count(), 2);
    assert_eq!(manager.newest_image_with_number(20), Some(ImageId(2)));

    manager.handle_command(command(b"a=t,I=40,f=32,s=1,v=1;DQ4PEA=="), false);
    assert_eq!(manager.newest_image_with_number(20), None);
    assert!(!manager.image_number_by_id.contains_key(&ImageId(2)));
}

#[test]
fn queries_require_ids_and_reject_image_numbers_truthfully() {
    let mut manager = GraphicsManager::new();
    let result = manager.handle_command(command(b"a=q,I=9,f=32,s=1,v=1;AQIDBA=="), false);
    let response = result.response.unwrap();
    assert_eq!(response.image_number, Some(9));
    assert_eq!(response.error_code, Some("EINVAL"));
    assert!(
        response
            .to_escape_sequence()
            .starts_with("\x1b_GI=9;EINVAL:")
    );
    assert!(manager.storage.is_empty());
}

#[test]
fn supported_delete_selectors_remove_only_matching_placements() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(
        command(b"a=T,f=32,s=1,v=1,i=1,p=1,c=2,r=2,z=-1,C=1;AQIDBA=="),
        false,
    );
    manager.handle_command(
        command(b"a=T,f=32,s=1,v=1,i=2,p=2,c=1,r=1,z=2,C=1;AQIDBA=="),
        false,
    );
    manager.handle_command(command(b"a=d,d=z,z=-1"), false);
    assert_eq!(manager.placements(false).count(), 1);
    assert!(manager.get_image(ImageId(1)).is_some());
    manager.handle_command(command(b"a=d,d=I,i=2,p=2"), false);
    assert!(manager.get_image(ImageId(2)).is_none());
}

#[test]
fn referenced_images_evict_after_unplaced_cache_entries() {
    let mut manager = GraphicsManager::new();
    manager.storage = ImageStorage::new(2, 1024);

    manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=1,C=1;AQIDBA=="), false);
    manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=2;BQYHCA=="), false);
    manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=3;CQoLDA=="), false);

    assert!(manager.get_image(ImageId(1)).is_some());
    assert!(manager.get_image(ImageId(2)).is_none());
    assert!(manager.get_image(ImageId(3)).is_some());
    assert_eq!(manager.placements(false).count(), 1);

    // Once the placement disappears, image 1 becomes an ordinary cached
    // transmission and is again eligible for oldest-first eviction.
    manager.handle_command(command(b"a=d,d=a"), false);
    manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=4;DQ4PEA=="), false);
    assert!(manager.get_image(ImageId(1)).is_none());
    assert!(manager.get_image(ImageId(3)).is_some());
    assert!(manager.get_image(ImageId(4)).is_some());
}

#[test]
fn repeated_display_of_one_image_has_a_bounded_fifo_placement_window() {
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=1;AQIDBA=="), false);

    for placement_id in 1..=MAX_IMAGE_PLACEMENTS as u32 + 7 {
        let value = format!("a=p,i=1,p={placement_id},C=1");
        let result = manager.handle_command(command(value.as_bytes()), false);
        assert!(result.response.unwrap().ok);
    }

    let placements: Vec<_> = manager.placements(false).collect();
    assert_eq!(placements.len(), MAX_IMAGE_PLACEMENTS);
    assert_eq!(placements.first().unwrap().placement_id, Some(8));
    assert_eq!(
        placements.last().unwrap().placement_id,
        Some(MAX_IMAGE_PLACEMENTS as u32 + 7)
    );
    assert_eq!(manager.image_count(), 1);

    manager.handle_command(command(b"a=p,i=1,p=8,C=1"), false);
    assert_eq!(manager.placement_count(), MAX_IMAGE_PLACEMENTS);
    assert_eq!(
        manager.placements(false).last().unwrap().placement_id,
        Some(8)
    );
}

#[test]
fn natural_and_single_axis_placements_derive_exact_pixel_and_cell_geometry() {
    let mut manager = GraphicsManager::new();
    manager.set_cell_pixel_geometry(10, 20);
    let pixels = vec![0; 20 * 40 * 4];
    let result = manager.handle_command(raw_command("a=T,f=32,s=20,v=40,i=5", &pixels), false);
    assert!(result.response.unwrap().ok);
    assert_eq!(result.cursor_movement, Some((2, 2)));
    let natural = manager.placements(false).next().unwrap();
    assert_eq!((natural.width, natural.height), (Some(2), Some(2)));
    assert_eq!(
        (natural.pixel_width, natural.pixel_height),
        (Some(20), Some(40))
    );

    let result = manager.handle_command(command(b"a=p,i=5,p=2,c=4"), false);
    assert!(result.response.unwrap().ok);
    assert_eq!(result.cursor_movement, Some((4, 4)));
    let one_axis = manager.placements(false).last().unwrap();
    assert_eq!((one_axis.width, one_axis.height), (Some(4), Some(4)));
    assert_eq!(
        (one_axis.pixel_width, one_axis.pixel_height),
        (Some(40), Some(80))
    );
}

#[test]
fn partial_cell_pixel_images_scroll_at_pixel_boundaries() {
    for alternate in [false, true] {
        for down in [false, true] {
            let mut manager = GraphicsManager::new();
            manager.set_cell_pixel_geometry(4, 4);
            manager
                .handle_sixel(b"7q\"1;1;6;6!6~", 0, 0, (4, 4), alternate)
                .unwrap();
            let original = manager
                .current_image(manager.placements(alternate).next().unwrap().image_id)
                .unwrap();
            if down {
                manager.scroll_down(alternate, 0..2, 0..2, 1);
            } else {
                manager.scroll_up(alternate, 0..2, 0..2, 1, false, 0);
            }
            let p = manager.placements(alternate).next().unwrap();
            assert_eq!(
                (p.source_y, p.source_height, p.pixel_height),
                if down {
                    (0, Some(4), Some(4))
                } else {
                    (4, Some(2), Some(2))
                }
            );
            assert_eq!((p.row, p.height), (i64::from(down), Some(1)));
            assert!(std::sync::Arc::ptr_eq(
                &original.data,
                &manager.current_image(p.image_id).unwrap().data
            ));
            manager.clear_screen(alternate);
            assert_eq!(manager.memory_usage(), 0);
        }
    }
}

#[test]
fn partial_cell_pixel_images_copy_and_erase_exact_coverage() {
    let mut manager = GraphicsManager::new();
    manager.set_cell_pixel_geometry(4, 4);
    manager
        .handle_sixel(b"7q\"1;1;6;6!6~", 0, 0, (4, 4), false)
        .unwrap();
    manager.copy_rect(false, 1..2, 1..2, 4, 4);
    let copied = manager.placements(false).find(|p| p.row == 4).unwrap();
    assert_eq!((copied.source_x, copied.source_y), (4, 4));
    assert_eq!(
        (copied.source_width, copied.source_height),
        (Some(2), Some(2))
    );
    assert_eq!(
        (copied.pixel_width, copied.pixel_height),
        (Some(2), Some(2))
    );
    manager.erase_rect(false, 0..1, 0..1);
    let bottom = manager.placements(false).find(|p| p.row == 1).unwrap();
    assert_eq!(
        (bottom.source_y, bottom.source_height, bottom.pixel_height),
        (4, Some(2), Some(2))
    );
    let right = manager
        .placements(false)
        .find(|p| p.col == 1 && p.row == 0)
        .unwrap();
    assert_eq!(
        (right.source_x, right.source_width, right.pixel_width),
        (4, Some(2), Some(2))
    );
    assert_eq!(
        (right.source_height, right.pixel_height),
        (Some(4), Some(4))
    );
}

#[test]
fn pixel_protocol_placements_clip_reflow_and_release_unreachable_pixels() {
    let mut manager = GraphicsManager::new();
    manager.set_cell_pixel_geometry(2, 3);
    manager
        .handle_sixel(b"q\"1;1!4~", 0, 0, (2, 3), false)
        .unwrap();
    manager
        .handle_sixel(b"q\"1;1!4~", 0, 0, (2, 3), true)
        .unwrap();
    assert_eq!(manager.image_count(), 2);
    assert_eq!(manager.placement_count(), 2);
    assert_eq!(manager.memory_usage(), 2 * 4 * 6 * 4);

    manager.scroll_up(false, 0..2, 0..usize::MAX, 1, false, 0);
    let clipped = manager.placements(false).next().unwrap();
    assert_eq!(clipped.height, Some(1));
    assert_eq!(clipped.pixel_height, Some(3));
    assert_eq!(clipped.source_y, 3);
    assert_eq!(clipped.source_height, Some(3));

    manager.set_cell_pixel_geometry(4, 6);
    let primary = manager.placements(false).next().unwrap();
    assert_eq!((primary.width, primary.height), (Some(1), Some(1)));
    assert_eq!(
        (primary.pixel_width, primary.pixel_height),
        (Some(4), Some(3))
    );

    manager.clear_screen(false);
    assert_eq!(manager.image_count(), 1);
    assert_eq!(manager.placement_count(), 1);
    manager.clear_screen(true);
    assert_eq!(manager.image_count(), 0);
    assert_eq!(manager.memory_usage(), 0);
}

#[test]
fn sixel_aspect_changes_display_footprint_without_expanding_source_storage() {
    let mut manager = GraphicsManager::new();
    let movement = manager
        .handle_sixel(b"7q\"2;1;2;6!2~", 3, 4, (2, 3), false)
        .unwrap();
    assert_eq!(movement, (0, 4));
    assert_eq!(manager.memory_usage(), 2 * 6 * 4);
    let placement = manager.placements(false).next().unwrap();
    assert_eq!((placement.col, placement.row), (3, 4));
    assert_eq!((placement.width, placement.height), (Some(1), Some(4)));
    assert_eq!(
        (placement.pixel_width, placement.pixel_height),
        (Some(2), Some(12))
    );
    let image = manager.get_image(placement.image_id).unwrap();
    assert_eq!((image.width, image.height), (2, 6));
}

#[test]
fn static_pixel_protocols_share_scroll_resize_screen_and_cleanup_lifecycle() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
            .unwrap();
    }
    let encoded = STANDARD.encode(png_bytes);
    let data = format!("preserveAspectRatio=0:{encoded}");
    let params = [
        b"File=inline=1".as_slice(),
        b"width=4px".as_slice(),
        b"height=6px".as_slice(),
        data.as_bytes(),
    ];

    let mut manager = GraphicsManager::new();
    manager.set_cell_pixel_geometry(2, 3);
    for alternate_screen in [false, true] {
        manager
            .handle_sixel(b"7q\"1;1;4;6!4~", 0, 0, (2, 3), alternate_screen)
            .unwrap();
        manager
            .handle_iterm2(&params, (0, 0), (80, 24), (2, 3), alternate_screen)
            .unwrap();
    }
    assert_eq!(manager.image_count(), 4);
    assert_eq!(manager.placements(false).count(), 2);
    assert_eq!(manager.placements(true).count(), 2);

    manager.scroll_up(false, 0..2, 0..usize::MAX, 1, false, 0);
    let clipped: Vec<_> = manager.placements(false).collect();
    assert_eq!(clipped.len(), 2);
    assert!(clipped.iter().all(|placement| placement.height == Some(1)));
    assert_eq!(manager.placements(true).count(), 2);

    manager.set_cell_pixel_geometry(4, 6);
    assert!(
        manager
            .placements(false)
            .all(|placement| { placement.width == Some(1) && placement.height == Some(1) })
    );

    manager.clear_screen(false);
    assert_eq!(manager.image_count(), 2);
    assert_eq!(manager.placement_count(), 2);
    assert_eq!(manager.placements(false).count(), 0);
    assert_eq!(manager.placements(true).count(), 2);
    manager.clear_screen(true);
    assert_eq!(manager.image_count(), 0);
    assert_eq!(manager.placement_count(), 0);
    assert_eq!(manager.memory_usage(), 0);
}

#[test]
fn offset_pixel_images_keep_and_crop_their_last_covered_cell() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let mut manager = GraphicsManager::new();
    manager.set_cell_pixel_geometry(4, 4);
    let pixels = STANDARD.encode(vec![255; 6 * 6 * 4]);
    manager.handle_command(
        command(format!("a=T,i=1,f=32,s=6,v=6,X=3,Y=3,C=1;{pixels}").as_bytes()),
        false,
    );
    let p = manager.placements(false).next().unwrap();
    assert_eq!((p.width, p.height), (Some(3), Some(3)));
    manager.copy_rect(false, 2..3, 2..3, 4, 4);
    let p = manager.placements(false).find(|p| p.row == 4).unwrap();
    assert_eq!((p.source_x, p.source_y), (5, 5));
    assert_eq!((p.pixel_width, p.pixel_height), (Some(1), Some(1)));
    assert_eq!((p.x_offset, p.y_offset), (0, 0));
    manager.erase_rect(false, 0..1, 0..1);
    let p = manager.placements(false).find(|p| p.row == 1).unwrap();
    assert_eq!((p.source_y, p.pixel_height, p.y_offset), (1, Some(5), 0));
    assert_eq!((p.source_x, p.pixel_width, p.x_offset), (0, Some(6), 3));
    manager.set_cell_pixel_geometry(2, 2);
    let p = manager.placements(false).find(|p| p.row == 1).unwrap();
    assert_eq!((p.width, p.height), (Some(5), Some(3)));
}

#[test]
fn pixel_crop_axis_matches_independent_covered_pixel_oracle() {
    for cell in 1_u32..=7 {
        for offset in 0_u32..=7 {
            for extent in 1..=13 {
                let footprint = (offset + extent).div_ceil(cell);
                for start in 0..footprint {
                    for end in start + 1..=footprint {
                        let pixels: Vec<_> = (offset..offset + extent)
                            .filter(|pixel| (start * cell..end * cell).contains(pixel))
                            .collect();
                        let actual =
                            crop_axis(extent, Some(extent), offset, cell, start..end, footprint);
                        if pixels.is_empty() {
                            assert!(actual.is_none());
                        } else {
                            let actual = actual.unwrap();
                            assert_eq!(actual.source_start, pixels[0] - offset);
                            assert_eq!(actual.source_extent, pixels.len() as u32);
                            assert_eq!(actual.pixel_extent, Some(pixels.len() as u32));
                            assert_eq!(actual.offset, pixels[0] - start * cell);
                        }
                    }
                }
            }
        }
    }
    let scaled = crop_axis(6, Some(12), 0, 5, 1..3, 3).unwrap();
    assert_eq!(
        (
            scaled.source_start,
            scaled.source_extent,
            scaled.pixel_extent
        ),
        (2, 4, Some(7))
    );
}

#[test]
fn rectangular_text_operations_crop_erase_and_copy_image_coverage() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let pixels = STANDARD.encode(vec![255; 4 * 4 * 4]);
    let command_text = format!("a=T,f=32,s=4,v=4,i=1,c=4,r=4,C=1;{pixels}");
    let mut manager = GraphicsManager::new();
    manager.handle_command(command(command_text.as_bytes()), false);
    manager.erase_rect(false, 1..3, 1..3);

    let fragments: Vec<_> = manager.placements(false).collect();
    assert_eq!(fragments.len(), 4);
    assert_eq!(
        fragments
            .iter()
            .map(|placement| { placement.width.unwrap_or(1) * placement.height.unwrap_or(1) })
            .sum::<u32>(),
        12
    );
    for row in 1..3 {
        for col in 1..3 {
            assert!(
                fragments
                    .iter()
                    .all(|placement| !placement_intersects(placement, col, row))
            );
        }
    }
    assert!(manager.get_image(ImageId(1)).is_some());

    let mut copied = GraphicsManager::new();
    let command_text = format!("a=T,f=32,s=4,v=4,i=2,c=2,r=2,C=1;{pixels}");
    copied.handle_command(command(command_text.as_bytes()), false);
    copied.copy_rect(false, 0..2, 0..2, 1, 2);
    let placements: Vec<_> = copied.placements(false).collect();
    assert_eq!(placements.len(), 2);
    assert!(placements.iter().any(|placement| {
        placement.col == 0
            && placement.row == 0
            && placement.width == Some(2)
            && placement.height == Some(2)
    }));
    assert!(placements.iter().any(|placement| {
        placement.col == 2
            && placement.row == 1
            && placement.width == Some(2)
            && placement.height == Some(2)
    }));
    assert!(copied.get_image(ImageId(2)).is_some());
}

#[test]
fn all_protocols_share_one_bounded_cache_and_normalized_placement_model() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
            .unwrap();
    }
    let encoded = STANDARD.encode(png_bytes);
    let data = format!("height=10px:{encoded}");
    let params = [b"File=inline=1".as_slice(), data.as_bytes()];

    let mut manager = GraphicsManager::new();
    manager
        .handle_sixel(b"q\"1;1!4~", 1, 2, (10, 10), false)
        .unwrap();
    manager
        .handle_iterm2(&params, (3, 4), (80, 24), (10, 10), false)
        .unwrap();

    let placements: Vec<_> = manager.placements(false).collect();
    assert_eq!(placements.len(), 2);
    assert!(placements.iter().all(|placement| {
        placement.source_width.is_some()
            && placement.source_height.is_some()
            && placement.pixel_width.is_some()
            && placement.pixel_height.is_some()
    }));
    assert_eq!(manager.image_count(), 2);
    assert!(manager.memory_usage() <= MAX_IMAGE_MEMORY);
}
