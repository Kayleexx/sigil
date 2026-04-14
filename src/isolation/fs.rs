use nix::errno::Errno;
use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::unistd::chdir;
use std::ffi::CString;
use std::fs::{create_dir_all, remove_dir};
use std::path::Path;

pub fn setup_rootfs(rootfs: &Path) -> nix::Result<()> {
    crate::isolation::cgroups::capture_host_context()?;
    create_dir_all(rootfs).map_err(io_err)?;
    mount(
        Some(rootfs),
        rootfs,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )?;

    let old_root = rootfs.join(".old_root");
    create_dir_all(&old_root).map_err(io_err)?;

    pivot_root(rootfs, &old_root)?;
    chdir("/")?;
    umount2("/.old_root", MntFlags::MNT_DETACH)?;
    remove_dir("/.old_root").map_err(io_err)?;
    Ok(())
}

pub fn setup_proc() -> nix::Result<()> {
    create_dir_all("/proc").map_err(io_err)?;
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )?;
    Ok(())
}

fn pivot_root(new_root: &Path, put_old: &Path) -> nix::Result<()> {
    let new_root = CString::new(new_root.as_os_str().as_encoded_bytes()).map_err(|_| Errno::EINVAL)?;
    let put_old = CString::new(put_old.as_os_str().as_encoded_bytes()).map_err(|_| Errno::EINVAL)?;
    let res = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_pivot_root,
            new_root.as_ptr(),
            put_old.as_ptr(),
        )
    };
    Errno::result(res as i32).map(drop)
}

fn io_err(err: std::io::Error) -> Errno {
    Errno::from_raw(err.raw_os_error().unwrap_or(nix::libc::EIO))
}
