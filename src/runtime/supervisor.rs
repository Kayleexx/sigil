use crate::{
    isolation,
    runtime::{child, signals},
};
use nix::errno::Errno;
use nix::sched::clone;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{pipe, setpgid, write, Pid};

const CHILD_STACK_SIZE: usize = 1024 * 1024;

pub fn spawn(args: Vec<String>) -> nix::Result<i32> {
    signals::install();

    let (reader, writer) = pipe()?;
    let child_writer = writer
        .try_clone()
        .map_err(|err| Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EINVAL)))?;
    let mut child_stack = vec![0u8; CHILD_STACK_SIZE];
    let clone_flags = isolation::namespaces::clone_flags();
    let mut child_reader = Some(reader);
    let mut child_writer = Some(child_writer);
    let mut child_args = Some(args);

    let child_pid = unsafe {
        clone(
            Box::new(move || {
                drop(child_writer.take());
                let reader = child_reader
                    .take()
                    .expect("clone callback invoked without child reader");
                let args = child_args
                    .take()
                    .expect("clone callback invoked without child args");

                if let Err(err) = child::bootstrap(reader, &args) {
                    eprintln!("child bootstrap failed: {}", err);
                }

                1
            }),
            &mut child_stack,
            clone_flags,
            Some(Signal::SIGCHLD as i32),
        )?
    };

    // Parent assigns child to its own process group before releasing it.
    setpgid(child_pid, child_pid)?;

    write(&writer, &[0u8; 1])?;

    drop(writer);

    supervisor_loop(child_pid)
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
