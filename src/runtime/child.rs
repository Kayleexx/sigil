use crate::{config::ContainerConfig, isolation};
use nix::errno::Errno;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{execvp, fork, getpid, read, setpgid, ForkResult, Pid};
use std::convert::TryFrom;
use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::process::exit;
use std::sync::atomic::{AtomicI32, Ordering};

static FORWARDED_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub fn bootstrap(sync_fd: OwnedFd, config: &ContainerConfig) -> nix::Result<()> {
    let mut buf = [0u8; 1];
    let n = read(&sync_fd, &mut buf)?;
    if n == 0 {
        return Err(Errno::EPIPE);
    }
    drop(sync_fd);

    isolation::namespaces::setup_namespaces()?;
    isolation::mounts::setup_mounts()?;
    isolation::fs::setup_rootfs(&config.rootfs)?;
    isolation::fs::setup_proc()?;
    let cgroup = isolation::cgroups::setup_cgroups(config)?;

    if getpid().as_raw() == 1 {
        let exit_code = run_as_init(&config.command)?;
        if let Err(err) = cgroup.cleanup() {
            eprintln!("cgroup cleanup failed: {}", err);
        }
        exit(exit_code);
    } else {
        exec_target(&config.command)?;
    }

    Ok(())
}

fn run_as_init(args: &[String]) -> nix::Result<i32> {
    install_init_signal_handlers()?;

    match unsafe { fork()? } {
        ForkResult::Parent { child } => {
            return init_loop(child);
        }
        ForkResult::Child => {
            setpgid(Pid::from_raw(0), Pid::from_raw(0))?;
            exec_target(args)?;
        }
    }

    Ok(0)
}

fn exec_target(args: &[String]) -> nix::Result<()> {
    let c_args: Vec<CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).expect("failed to convert to CString"))
        .collect();
    let cmd = c_args.first().ok_or(Errno::EINVAL)?;
    execvp(cmd, &c_args)?;
    Ok(())
}

fn init_loop(main_child: Pid) -> nix::Result<i32> {
    loop {
        forward_pending_signal(main_child)?;

        match waitpid(Pid::from_raw(-1), None) {
            Ok(WaitStatus::Exited(pid, status)) => {
                if pid == main_child {
                    return Ok(status);
                }
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                if pid == main_child {
                    return Ok(128 + sig as i32);
                }
            }
            Ok(WaitStatus::StillAlive) => {}
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(Errno::ECHILD) => return Ok(0),
            Err(err) => return Err(err),
        }
    }
}

fn install_init_signal_handlers() -> nix::Result<()> {
    let action = SigAction::new(
        SigHandler::Handler(init_signal_handler),
        SaFlags::empty(),
        SigSet::empty(),
    );

    for sig in forwarded_signals() {
        unsafe {
            signal::sigaction(sig, &action)?;
        }
    }

    Ok(())
}

fn forward_pending_signal(main_child: Pid) -> nix::Result<()> {
    let pending = FORWARDED_SIGNAL.swap(0, Ordering::SeqCst);
    if pending == 0 {
        return Ok(());
    }

    let signal = Signal::try_from(pending).map_err(|_| Errno::EINVAL)?;
    match signal::kill(Pid::from_raw(-main_child.as_raw()), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(err),
    }
}

fn forwarded_signals() -> [Signal; 4] {
    [Signal::SIGINT, Signal::SIGTERM, Signal::SIGQUIT, Signal::SIGHUP]
}

extern "C" fn init_signal_handler(sig: i32) {
    FORWARDED_SIGNAL.store(sig, Ordering::SeqCst);
}
