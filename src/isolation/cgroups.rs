use crate::config::ContainerConfig;
use nix::errno::Errno;
use std::fs::{File, create_dir_all, read_to_string, remove_dir, write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
static HOST_CGROUP_ROOT: Mutex<Option<File>> = Mutex::new(None);
static HOST_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub struct CgroupHandle {
    parent_dir: PathBuf,
    container_dir: PathBuf,
}

pub fn capture_host_context() -> nix::Result<()> {
    let mut guard = HOST_CGROUP_ROOT.lock().map_err(|_| Errno::EIO)?;
    if guard.is_none() {
        *guard = Some(File::open(CGROUP_ROOT).map_err(io_err)?);
    }
    drop(guard);

    HOST_PID.store(read_host_pid()?, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

pub fn setup_cgroups(config: &ContainerConfig) -> nix::Result<CgroupHandle> {
    let cgroup_root = host_cgroup_root()?;
    let sigil_root = cgroup_root.join("sigil");
    create_dir_all(&sigil_root).map_err(io_err)?;

    enable_memory_controller(&cgroup_root)?;
    enable_memory_controller(&sigil_root)?;

    let container_dir = sigil_root.join(&config.cgroup_name);
    create_dir_all(&container_dir).map_err(io_err)?;
    write(container_dir.join("memory.max"), &config.memory_max).map_err(io_err)?;
    write(container_dir.join("cgroup.procs"), format!("{}\n", host_pid()?)).map_err(io_err)?;

    Ok(CgroupHandle {
        parent_dir: sigil_root,
        container_dir,
    })
}

impl CgroupHandle {
    pub fn cleanup(&self) -> nix::Result<()> {
        let pid = format!("{}\n", host_pid()?);
        write(self.parent_dir.join("cgroup.procs"), pid).map_err(io_err)?;
        match remove_dir(&self.container_dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(io_err(err)),
        }
    }
}

fn enable_memory_controller(path: &Path) -> nix::Result<()> {
    let controllers = read_to_string(path.join("cgroup.controllers")).map_err(io_err)?;
    if !controllers.split_whitespace().any(|controller| controller == "memory") {
        return Err(Errno::ENODEV);
    }

    let subtree_control = path.join("cgroup.subtree_control");
    let enabled = read_to_string(&subtree_control).map_err(io_err)?;
    if enabled.split_whitespace().any(|controller| controller == "memory") {
        return Ok(());
    }

    write(&subtree_control, "+memory").map_err(io_err)?;
    Ok(())
}

fn host_cgroup_root() -> nix::Result<PathBuf> {
    let guard = HOST_CGROUP_ROOT.lock().map_err(|_| Errno::EIO)?;
    let root = guard.as_ref().ok_or(Errno::EINVAL)?;
    Ok(PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd())))
}

fn read_host_pid() -> nix::Result<i32> {
    let status = read_to_string("/proc/self/status").map_err(io_err)?;
    let line = status
        .lines()
        .find(|line| line.starts_with("NSpid:"))
        .ok_or(Errno::EINVAL)?;
    let pid = line
        .split_whitespace()
        .nth(1)
        .ok_or(Errno::EINVAL)?
        .parse::<i32>()
        .map_err(|_| Errno::EINVAL)?;
    Ok(pid)
}

fn host_pid() -> nix::Result<i32> {
    let pid = HOST_PID.load(std::sync::atomic::Ordering::SeqCst);
    if pid <= 0 {
        return Err(Errno::EINVAL);
    }
    Ok(pid)
}

fn io_err(err: std::io::Error) -> Errno {
    Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EIO))
}
