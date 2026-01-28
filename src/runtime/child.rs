use nix::unistd::{Pid, close, execvp, read, setpgid};
use std::ffi::CString;
use std::os::unix::io::BorrowedFd;

use std::process::exit;

pub fn bootstrap(sync_fd: i32, args: &[String]) {
    let fd = unsafe { BorrowedFd::borrow_raw(sync_fd) };

    let mut buf = [0u8; 1];
    let _ = read(fd, &mut buf);
    let _ = close(sync_fd);

    let _ = setpgid(Pid::from_raw(0), Pid::from_raw(0));

    let c_args: Vec<std::ffi::CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).expect("failed to convert to CString"))
        .collect();

    if !c_args.is_empty() {
        let _ = execvp(&c_args[0], &c_args);
    }

    let _ = (&c_args[0], &c_args);
    exit(1)
}
