use crate::runtime::{child, signals};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, close, fork, pipe, write};
use std::os::unix::io::{AsRawFd, BorrowedFd};
use std::process::exit;

pub fn spawn(args: Vec<String>) {
    let (reader, writer) = pipe().expect("failed to create a pipe");

    let r_fd = reader.as_raw_fd();
    let w_fd = writer.as_raw_fd();

    match unsafe { fork().expect("failed to fork") } {
        ForkResult::Parent { child: child_pid } => {
            let _ = close(r_fd);
            let borrowed_writer = unsafe { BorrowedFd::borrow_raw(w_fd) };
            let _ = write(borrowed_writer, &[0u8; 1]);

            let _ = close(w_fd);

            supervisor_loop(child_pid);
        }
        ForkResult::Child => {
            let _ = close(w_fd);

            child::bootstrap(r_fd, &args);

            exit(1);
        }
    }
}

fn supervisor_loop(child_pid: Pid) {
    loop {
        if let Some(_) = signals::check_signals() {
            let _ = nix::sys::signal::kill(
                Pid::from_raw(-child_pid.as_raw()),
                nix::sys::signal::Signal::SIGTERM,
            );
        }

        match waitpid(child_pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(_, status)) => {
                println!("Child exited with status: {}", status);
                break;
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                println!("Child killed by signal: {:?}", sig);
                break;
            }
            Ok(WaitStatus::StillAlive) => {}
            Err(_) => break,
            _ => {}
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

