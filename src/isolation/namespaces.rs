use nix::sched::CloneFlags;
use nix::unistd::sethostname;

pub fn clone_flags() -> CloneFlags {
    CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS | CloneFlags::CLONE_NEWNS
}

pub fn setup_namespaces() -> nix::Result<()> {
    sethostname("sigil-container")?;
    Ok(())
}
