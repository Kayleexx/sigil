use crate::config::ContainerConfig;
use nix::errno::Errno;
use nix::unistd::getpid;
use std::thread;
use std::time::Duration;
use std::fs::{File, create_dir_all, read_to_string, remove_dir, write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const CGROUP_ATTACH_RETRY_LIMIT: usize = 10;
const CGROUP_ATTACH_RETRY_DELAY: Duration = Duration::from_millis(2);
static HOST_CGROUP_ROOT: Mutex<Option<File>> = Mutex::new(None);

pub struct CgroupHandle {
    parent_dir: PathBuf,
    container_dir: PathBuf,
}

pub fn capture_host_context() -> nix::Result<()> {
    let mut guard = HOST_CGROUP_ROOT.lock().map_err(|_| Errno::EIO)?;
    if guard.is_none() {
        *guard = Some(File::open(CGROUP_ROOT).map_err(io_err)?);
    }
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
    attach_pid_to_cgroup(&container_dir.join("cgroup.procs"))?;

    Ok(CgroupHandle {
        parent_dir: sigil_root,
        container_dir,
    })
}

impl CgroupHandle {
    pub fn cleanup(&self) -> nix::Result<()> {
        attach_pid_to_cgroup(&self.parent_dir.join("cgroup.procs"))?;
        match remove_dir(&self.container_dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(io_err(err)),
        }
    }
}

fn attach_pid_to_cgroup(cgroup_procs: &Path) -> nix::Result<()> {
    let pid = getpid().as_raw();
    eprintln!(
        "setup_cgroups: attaching namespace pid {} to {}",
        pid,
        cgroup_procs.display()
    );

    let pid = format!("{}\n", pid);
    for attempt in 0..CGROUP_ATTACH_RETRY_LIMIT {
        match write(cgroup_procs, &pid) {
            Ok(()) => return Ok(()),
            Err(err) if err.raw_os_error() == Some(nix::libc::ESRCH) && attempt + 1 < CGROUP_ATTACH_RETRY_LIMIT => {
                thread::sleep(CGROUP_ATTACH_RETRY_DELAY);
            }
            Err(err) => return Err(io_err(err)),
        }
    }

    Err(Errno::ESRCH)
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

fn io_err(err: std::io::Error) -> Errno {
    Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EIO))
}
