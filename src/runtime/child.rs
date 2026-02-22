use nix::errno::Errno;
use nix::unistd::{close, execvp, read};
use std::ffi::CString;
use std::os::unix::io::BorrowedFd;

pub fn bootstrap(sync_fd: i32, args: &[String]) -> nix::Result<()> {
    let fd = unsafe { BorrowedFd::borrow_raw(sync_fd) };

    let mut buf = [0u8; 1];
    let n = read(fd, &mut buf)?;
    if n == 0 {
        return Err(Errno::EPIPE);
    }
    close(sync_fd)?;

    let c_args: Vec<std::ffi::CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).expect("failed to convert to CString"))
        .collect();

    if let Some(cmd) = c_args.first() {
        execvp(cmd, &c_args)?;
    }

    let _ = &c_args;
    Ok(())
}
