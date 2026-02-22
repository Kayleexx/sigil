use nix::errno::Errno;
use nix::unistd::{execvp, read};
use std::os::fd::OwnedFd;
use std::ffi::CString;

pub fn bootstrap(sync_fd: OwnedFd, args: &[String]) -> nix::Result<()> {
    let mut buf = [0u8; 1];
    let n = read(&sync_fd, &mut buf)?;
    if n == 0 {
        return Err(Errno::EPIPE);
    }
    drop(sync_fd);

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
