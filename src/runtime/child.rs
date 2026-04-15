use crate::{config::ContainerConfig, isolation};
use nix::fcntl::{fcntl, FcntlArg};
use nix::errno::Errno;
use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{
    dup2_stderr, dup2_stdin, dup2_stdout, execvp, fork, getpid, read, setsid,
    ForkResult, Pid,
};
use std::convert::TryFrom;
use std::ffi::CString;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::process::exit;
use std::sync::atomic::{AtomicI32, Ordering};

static FORWARDED_SIGNAL: AtomicI32 = AtomicI32::new(0);

pub fn bootstrap(
    sync_fd: OwnedFd,
    tty_slave: Option<OwnedFd>,
    config: &ContainerConfig,
) -> Result<(), String> {
    wait_for_parent(&sync_fd).map_err(|err| format!("wait_for_parent failed: {}", err))?;
    drop(sync_fd);

    run_stage("setup_namespaces", || isolation::namespaces::setup_namespaces())?;
    run_stage("setup_mounts", || isolation::mounts::setup_mounts())?;
    run_stage("setup_rootfs", || isolation::fs::setup_rootfs(&config.rootfs))?;
    run_stage("setup_proc", || isolation::fs::setup_proc())?;
    let cgroup = run_stage("setup_cgroups", || isolation::cgroups::setup_cgroups(config))?;

    if getpid().as_raw() == 1 {
        let exit_code = run_as_init(&config.command, tty_slave)
            .map_err(|err| format!("run_as_init failed: {}", err))?;
        if let Err(err) = cgroup.cleanup() {
            eprintln!("cgroup cleanup failed: {}", err);
        }
        exit(exit_code);
    } else {
        exec_with_terminal(tty_slave, &config.command)
            .map_err(|err| format!("exec_target failed: {}", err))?;
    }

    Ok(())
}

fn wait_for_parent(sync_fd: &OwnedFd) -> nix::Result<()> {
    let mut buf = [0u8; 1];
    let n = read(sync_fd, &mut buf)?;
    if n == 0 {
        return Err(Errno::EPIPE);
    }
    Ok(())
}

fn run_stage<T, F>(stage: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> nix::Result<T>,
{
    f().map_err(|err| format!("{} failed: {}", stage, err))
}

fn run_as_init(args: &[String], tty_slave: Option<OwnedFd>) -> nix::Result<i32> {
    install_init_signal_handlers()?;
    let mut tty_slave = tty_slave;

    match unsafe { fork()? } {
        ForkResult::Parent { child } => {
            drop(tty_slave.take());
            return init_loop(child);
        }
        ForkResult::Child => {
            let tty_slave = tty_slave.take();
            exec_with_terminal(tty_slave, args)?;
        }
    }

    Ok(0)
}

fn exec_target(args: &[String]) -> nix::Result<()> {
    verify_stdio()?;

    let c_args: Vec<CString> = args
        .iter()
        .map(|s| CString::new(s.as_str()).expect("failed to convert to CString"))
        .collect();
    let cmd = c_args.first().ok_or(Errno::EINVAL)?;
    execvp(cmd, &c_args)?;
    Ok(())
}

fn exec_with_terminal(tty_slave: Option<OwnedFd>, args: &[String]) -> nix::Result<()> {
    if let Some(slave) = tty_slave {
        setup_controlling_terminal(slave)?;
    }

    exec_target(args)
}

fn verify_stdio() -> nix::Result<()> {
    for fd in [0, 1, 2] {
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        fcntl(borrowed, FcntlArg::F_GETFD)?;
    }
    Ok(())
}

fn setup_controlling_terminal(slave: OwnedFd) -> nix::Result<()> {
    setsid()?;

    let fd = slave.as_raw_fd();
    let res = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCSCTTY, 0) };
    Errno::result(res)?;

    dup2_stdin(&slave)?;
    dup2_stdout(&slave)?;
    dup2_stderr(&slave)?;

    if fd > 2 {
        drop(slave);
    }

    Ok(())
}

fn init_loop(main_child: Pid) -> nix::Result<i32> {
    loop {
        if let Some(signal) = take_pending_signal()? {
            terminate_namespace()?;
            reap_all_children()?;
            return Ok(128 + signal as i32);
        }

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

fn take_pending_signal() -> nix::Result<Option<Signal>> {
    let pending = FORWARDED_SIGNAL.swap(0, Ordering::SeqCst);
    if pending == 0 {
        return Ok(None);
    }

    let signal = Signal::try_from(pending).map_err(|_| Errno::EINVAL)?;
    Ok(Some(signal))
}

fn terminate_namespace() -> nix::Result<()> {
    match signal::kill(Pid::from_raw(-1), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(err),
    }
}

fn reap_all_children() -> nix::Result<()> {
    loop {
        match waitpid(Pid::from_raw(-1), None) {
            Ok(_) => continue,
            Err(Errno::EINTR) => continue,
            Err(Errno::ECHILD) => return Ok(()),
            Err(err) => return Err(err),
        }
    }
}

fn forwarded_signals() -> [Signal; 4] {
    [Signal::SIGINT, Signal::SIGTERM, Signal::SIGQUIT, Signal::SIGHUP]
}

extern "C" fn init_signal_handler(sig: i32) {
    FORWARDED_SIGNAL.store(sig, Ordering::SeqCst);
}
