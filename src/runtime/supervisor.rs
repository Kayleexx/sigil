use crate::runtime::{child, signals};
use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, fork, pipe, setpgid, write, ForkResult, Pid};
use std::os::unix::io::{AsRawFd, BorrowedFd};
use std::process::exit;

pub fn spawn(args: Vec<String>) -> nix::Result<i32> {
    signals::install();

    let (reader, writer) = pipe()?;

    let r_fd = reader.as_raw_fd();
    let w_fd = writer.as_raw_fd();

    match unsafe { fork()? } {
        ForkResult::Parent { child: child_pid } => {
            close(r_fd)?;

            // Parent assigns child to its own process group before releasing it.
            setpgid(child_pid, child_pid)?;

            let borrowed_writer = unsafe { BorrowedFd::borrow_raw(w_fd) };
            write(borrowed_writer, &[0u8; 1])?;

            close(w_fd)?;

            supervisor_loop(child_pid)
        }
        ForkResult::Child => {
            if let Err(err) = close(w_fd) {
                eprintln!("child close(w_fd) failed: {}", err);
                exit(1);
            }

            if let Err(err) = child::bootstrap(r_fd, &args) {
                eprintln!("child bootstrap failed: {}", err);
            }

            exit(1);
        }
    }
}

fn supervisor_loop(child_pid: Pid) -> nix::Result<i32> {
    loop {
        let mut saw_signal = false;

        if let Some(event) = signals::check_signals() {
            saw_signal = true;
            if matches!(event, signals::SignalEvent::Terminate) {
                let _ = nix::sys::signal::kill(
                    Pid::from_raw(-child_pid.as_raw()),
                    Signal::SIGTERM,
                );
            }
        }

        match waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, status)) => {
                println!("Child exited with status: {}", status);
                return Ok(status);
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                println!("Child killed by signal: {:?}", sig);
                return Ok(128 + sig as i32);
            }
            Ok(WaitStatus::StillAlive) => {}
            Err(Errno::EINTR) => continue,
            Err(Errno::ECHILD) => return Ok(0),
            Err(err) => return Err(err),
            _ => {}
        }

        if !saw_signal {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
