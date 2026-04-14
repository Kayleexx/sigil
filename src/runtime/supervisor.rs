use crate::{
    config::ContainerConfig,
    isolation,
    runtime::{child, signals},
};
use nix::fcntl::OFlag;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::pty::{OpenptyResult, Winsize, openpty};
use nix::sched::clone;
use nix::sys::signal::Signal;
use nix::sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{isatty, pipe2, read, setpgid, write, Pid};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::thread;
use std::time::Duration;

const CHILD_STACK_SIZE: usize = 1024 * 1024;
const SETPGID_RETRY_LIMIT: usize = 10;
const SETPGID_RETRY_DELAY: Duration = Duration::from_millis(1);

pub fn spawn(config: ContainerConfig) -> nix::Result<i32> {
    signals::install();

    let (reader, writer) = make_sync_pipe()?;
    let pty = maybe_create_pty()?;
    let child_writer = writer
        .try_clone()
        .map_err(|err| Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EINVAL)))?;
    let mut child_stack = vec![0u8; CHILD_STACK_SIZE];
    let clone_flags = isolation::namespaces::clone_flags();
    let mut child_reader = Some(reader);
    let mut child_writer = Some(child_writer);
    let mut child_config = Some(config);
    let mut child_tty = pty
        .as_ref()
        .map(ChildTerminalSetup::from_pty)
        .transpose()?;
    let supervisor_tty = pty.map(SupervisorPty::new).transpose()?;

    let child_pid = unsafe {
        clone(
            Box::new(move || {
                drop(child_writer.take());
                if let Some(tty) = child_tty.as_mut() {
                    drop(tty.master_to_close.take());
                }
                let reader = child_reader
                    .take()
                    .expect("clone callback invoked without child reader");
                let config = child_config
                    .take()
                    .expect("clone callback invoked without child config");
                let tty_slave = child_tty.as_mut().and_then(|tty| tty.slave.take());

                if let Err(err) = child::bootstrap(reader, tty_slave, &config) {
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
    setpgid_when_ready(child_pid)?;

    write(&writer, &[0u8; 1])?;

    drop(writer);

    supervisor_loop(child_pid, supervisor_tty)
}

fn make_sync_pipe() -> nix::Result<(OwnedFd, OwnedFd)> {
    let (reader, writer) = pipe2(OFlag::O_CLOEXEC)?;
    Ok((ensure_high_fd(reader)?, ensure_high_fd(writer)?))
}

fn ensure_high_fd(fd: OwnedFd) -> nix::Result<OwnedFd> {
    if fd.as_raw_fd() > 2 {
        return Ok(fd);
    }

    let duplicated = fd
        .try_clone()
        .map_err(|err| Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EINVAL)))?;
    drop(fd);
    Ok(duplicated)
}

fn setpgid_when_ready(child_pid: Pid) -> nix::Result<()> {
    for attempt in 0..SETPGID_RETRY_LIMIT {
        match setpgid(child_pid, child_pid) {
            Ok(()) => return Ok(()),
            Err(Errno::ESRCH) if attempt + 1 < SETPGID_RETRY_LIMIT => {
                thread::sleep(SETPGID_RETRY_DELAY);
            }
            Err(err) => return Err(err),
        }
    }

    Err(Errno::ESRCH)
}

fn supervisor_loop(child_pid: Pid, mut pty: Option<SupervisorPty>) -> nix::Result<i32> {
    loop {
        let mut saw_signal = false;

        if let Some(pty) = pty.as_mut() {
            saw_signal = true;
            pump_pty_io(pty)?;
        }

        if let Some(event) = signals::check_signals() {
            saw_signal = true;
            if let signals::SignalEvent::Forward(signal) = event {
                let _ = nix::sys::signal::kill(
                    Pid::from_raw(-child_pid.as_raw()),
                    signal,
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

struct PtyPair {
    master: OwnedFd,
    slave: OwnedFd,
    stdin_termios: Termios,
}

struct ChildTerminalSetup {
    master_to_close: Option<OwnedFd>,
    slave: Option<OwnedFd>,
}

struct SupervisorPty {
    master: OwnedFd,
    stdin_open: bool,
    original_termios: Termios,
}

impl ChildTerminalSetup {
    fn from_pty(pty: &PtyPair) -> nix::Result<Self> {
        let master_to_close = pty
            .master
            .try_clone()
            .map_err(|err| Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EINVAL)))?;
        let slave = pty
            .slave
            .try_clone()
            .map_err(|err| Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EINVAL)))?;
        Ok(Self {
            master_to_close: Some(master_to_close),
            slave: Some(slave),
        })
    }
}

impl SupervisorPty {
    fn new(pty: PtyPair) -> nix::Result<Self> {
        let stdin = unsafe { BorrowedFd::borrow_raw(0) };
        let mut raw = pty.stdin_termios.clone();
        cfmakeraw(&mut raw);
        tcsetattr(stdin, SetArg::TCSANOW, &raw)?;
        drop(pty.slave);

        Ok(Self {
            master: pty.master,
            stdin_open: true,
            original_termios: pty.stdin_termios,
        })
    }
}

impl Drop for SupervisorPty {
    fn drop(&mut self) {
        let stdin = unsafe { BorrowedFd::borrow_raw(0) };
        let _ = tcsetattr(stdin, SetArg::TCSANOW, &self.original_termios);
    }
}

fn maybe_create_pty() -> nix::Result<Option<PtyPair>> {
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    if !isatty(stdin)? {
        return Ok(None);
    }

    let stdin_termios = tcgetattr(stdin)?;
    let winsize = get_winsize(stdin)?;
    let OpenptyResult { master, slave } = openpty(Some(&winsize), Some(&stdin_termios))?;

    Ok(Some(PtyPair {
        master: ensure_high_fd(master)?,
        slave: ensure_high_fd(slave)?,
        stdin_termios,
    }))
}

fn get_winsize(fd: BorrowedFd<'_>) -> nix::Result<Winsize> {
    let mut winsize = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let res = unsafe { nix::libc::ioctl(fd.as_raw_fd(), nix::libc::TIOCGWINSZ, &mut winsize) };
    Errno::result(res)?;
    Ok(winsize)
}

fn pump_pty_io(pty: &mut SupervisorPty) -> nix::Result<()> {
    let master_fd = pty.master.as_fd();
    let mut pollfds = if pty.stdin_open {
        let stdin = unsafe { BorrowedFd::borrow_raw(0) };
        vec![
            PollFd::new(stdin, PollFlags::POLLIN),
            PollFd::new(master_fd, PollFlags::POLLIN),
        ]
    } else {
        vec![PollFd::new(master_fd, PollFlags::POLLIN)]
    };

    match poll(&mut pollfds, PollTimeout::from(10u16)) {
        Ok(_) => {}
        Err(Errno::EINTR) => return Ok(()),
        Err(err) => return Err(err),
    }

    let mut idx = 0;
    if pty.stdin_open {
        if let Some(revents) = pollfds[idx].revents() {
            if revents.contains(PollFlags::POLLIN) {
                copy_stdin_to_pty(&pty.master)?;
            }
            if revents.intersects(PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL)
            {
                pty.stdin_open = false;
            }
        }
        idx += 1;
    }

    if let Some(revents) = pollfds[idx].revents() {
        if revents.contains(PollFlags::POLLIN) {
            copy_pty_to_stdout(&pty.master)?;
        }
    }

    Ok(())
}

fn copy_stdin_to_pty(master: &OwnedFd) -> nix::Result<()> {
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    let mut buf = [0u8; 4096];
    let n = read(stdin, &mut buf)?;
    if n == 0 {
        return Ok(());
    }
    write_all(master, &buf[..n])
}

fn copy_pty_to_stdout(master: &OwnedFd) -> nix::Result<()> {
    let stdout = unsafe { BorrowedFd::borrow_raw(1) };
    let mut buf = [0u8; 4096];
    match read(master, &mut buf) {
        Ok(0) => Ok(()),
        Ok(n) => write_all(stdout, &buf[..n]),
        Err(Errno::EIO) => Ok(()),
        Err(err) => Err(err),
    }
}

fn write_all<Fd: std::os::fd::AsFd>(fd: Fd, mut buf: &[u8]) -> nix::Result<()> {
    while !buf.is_empty() {
        let written = write(&fd, buf)?;
        buf = &buf[written..];
    }
    Ok(())
}
