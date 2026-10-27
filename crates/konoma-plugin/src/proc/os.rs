//! Operating-system helpers of the child supervision (the same ones the SVG drawing child uses).

/// Resident set size of `pid` in bytes. `None` where it cannot be read (the memory watch is then
/// off; the deadlines still apply).
#[cfg(target_os = "linux")]
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf with a valid constant has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(pages * if page > 0 { page as u64 } else { 4096 })
}

#[cfg(target_os = "macos")]
pub fn rss_bytes(pid: u32) -> Option<u64> {
    // SAFETY: an all-zero `proc_taskinfo` is a valid value of a plain C struct.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `info` is a valid writable buffer of exactly `size` bytes.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (got == size).then_some(info.pti_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn rss_bytes(_pid: u32) -> Option<u64> {
    None
}
