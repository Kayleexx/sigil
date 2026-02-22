use crate::runtime::{child, signals};
use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, pipe, setpgid, write, ForkResult, Pid};
use std::process::exit;

pub fn spawn(args: Vec<String>) -> nix::Result<i32> {
    signals::install();

    let (reader, writer) = pipe()?;

    match unsafe { fork()? } {
        ForkResult::Parent { child: child_pid } => {
            drop(reader);

            // Parent assigns child to its own process group before releasing it.
            setpgid(child_pid, child_pid)?;

            write(&writer, &[0u8; 1])?;

            drop(writer);

            supervisor_loop(child_pid)
        }
        ForkResult::Child => {
            drop(writer);

            if let Err(err) = child::bootstrap(reader, &args) {
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
