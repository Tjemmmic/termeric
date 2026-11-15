//! Process details from libproc and `sysctl`.

use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// The kernel's description of `pid`, or `None` once it has ended.
fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_bsdinfo is plain old data, for which all zeroes is valid.
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: the buffer is writable for exactly `size` bytes during the call.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (written == size).then_some(info)
}

pub(super) fn working_directory(pid: u32) -> Option<PathBuf> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_vnodepathinfo is plain old data, for which all zeroes is
    // valid.
    let mut info = unsafe { std::mem::zeroed::<libc::proc_vnodepathinfo>() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
    // SAFETY: the buffer is writable for exactly `size` bytes during the call.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // The path is a NUL-terminated MAXPATHLEN array, which libc spells as
    // 32 rows of 32 characters.
    let path = info.pvi_cdir.vip_path.as_flattened();
    // SAFETY: c_char and u8 have the same size and alignment.
    let bytes = unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), path.len()) };
    let path = CStr::from_bytes_until_nul(bytes).ok()?;
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
    path.is_absolute().then_some(path)
}

pub(super) fn parent(pid: u32) -> Option<u32> {
    bsd_info(pid).map(|info| info.pbi_ppid)
}

pub(super) fn newest_child(pid: u32) -> Option<u32> {
    // More children than a shell ever has at once; a pathological process
    // with more answers from the ones listed.
    const MAX_CHILDREN: usize = 256;
    let parent = libc::pid_t::try_from(pid).ok()?;
    let mut children = [0 as libc::pid_t; MAX_CHILDREN];
    let size = libc::c_int::try_from(std::mem::size_of_val(&children)).ok()?;
    // SAFETY: the buffer is writable for exactly `size` bytes during the call.
    let result = unsafe { libc::proc_listchildpids(parent, children.as_mut_ptr().cast(), size) };
    if result < 0 {
        return None;
    }
    // The buffer starts zeroed and no child is process 0, so the children
    // are its nonzero entries, whichever unit the count is reported in.
    children
        .iter()
        .filter_map(|child| u32::try_from(*child).ok().filter(|child| *child > 0))
        .filter_map(|child| {
            let info = bsd_info(child)?;
            Some(((info.pbi_start_tvsec, info.pbi_start_tvusec), child))
        })
        .max()
        .map(|(_, child)| child)
}

pub(super) fn name(pid: u32) -> Option<String> {
    let info = bsd_info(pid)?;
    // `pbi_name` holds the full name when it is longer than `pbi_comm`'s
    // sixteen characters, and is empty otherwise.
    [&info.pbi_name[..], &info.pbi_comm[..]]
        .into_iter()
        .map(c_chars_to_string)
        .find(|name| !name.is_empty())
}

fn c_chars_to_string(characters: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = characters
        .iter()
        .take_while(|character| **character != 0)
        .map(|character| *character as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The arguments from `sysctl(KERN_PROCARGS2)`, which only answers for this
/// user's own processes.
pub(super) fn arguments(pid: u32) -> Option<Vec<String>> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut size = argument_space()?;
    let mut buffer = vec![0u8; size];
    let mut name = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `name` holds three integers, and the buffer is writable for
    // `size` bytes, which sysctl lowers to what it wrote.
    let result = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    buffer.truncate(size);
    parse_process_arguments(&buffer)
}

/// The most space a process's arguments and environment can take.
fn argument_space() -> Option<usize> {
    let mut space: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut name = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `name` holds two integers and `space` is writable for `size`.
    let result = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            2,
            (&mut space as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0).then(|| usize::try_from(space).ok()).flatten()
}

/// `KERN_PROCARGS2` gives the argument count as a native `int`, the path the
/// program was executed from, NUL padding, then that many NUL-terminated
/// arguments (and the environment after them, which is not read).
fn parse_process_arguments(buffer: &[u8]) -> Option<Vec<String>> {
    let count = buffer.get(..4)?;
    let count = usize::try_from(i32::from_ne_bytes(count.try_into().ok()?)).ok()?;
    let rest = &buffer[4..];
    let rest = &rest[rest.iter().position(|byte| *byte == 0)?..];
    let rest = &rest[rest.iter().position(|byte| *byte != 0)?..];
    let arguments: Vec<String> = rest
        .split(|byte| *byte == 0)
        .take(count)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    (arguments.len() == count).then_some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_arguments_skip_the_executable_path_and_its_padding() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/bin/sleep\0\0\0\0sleep\x0030\0HOME=/Users/x\0");
        assert_eq!(
            parse_process_arguments(&buffer),
            Some(vec!["sleep".to_owned(), "30".to_owned()])
        );
    }

    #[test]
    fn truncated_process_arguments_are_refused() {
        let mut buffer = 3i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/bin/sleep\0sleep\x0030");
        assert_eq!(parse_process_arguments(&buffer), None);
        assert_eq!(parse_process_arguments(&[1, 0]), None);
    }
}
