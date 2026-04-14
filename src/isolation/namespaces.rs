use nix::sched::CloneFlags;
use nix::unistd::sethostname;

pub fn clone_flags() -> CloneFlags {
    CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS
}

pub fn setup_uts_namespace() -> nix::Result<()> {
    sethostname("sigil-container")?;
    Ok(())
}
