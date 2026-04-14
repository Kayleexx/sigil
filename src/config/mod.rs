use std::path::PathBuf;

pub struct ContainerConfig {
    pub rootfs: PathBuf,
    pub command: Vec<String>,
    pub cgroup_name: String,
    pub memory_max: String,
}

pub fn parse_args() -> Result<ContainerConfig, String> {
    let mut args = std::env::args().skip(1);
    let mut rootfs = PathBuf::from("./rootfs");
    let mut memory_max = String::from("100M");
    let mut cgroup_name = format!("container-{}", std::process::id());
    let mut command = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rootfs" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("missing value for --rootfs"))?;
                rootfs = PathBuf::from(value);
            }
            "--memory" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("missing value for --memory"))?;
                memory_max = value;
            }
            "--container-id" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("missing value for --container-id"))?;
                cgroup_name = value;
            }
            "--" => {
                command.extend(args);
                break;
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option: {}", arg));
            }
            _ => {
                command.push(arg);
                command.extend(args);
                break;
            }
        }
    }

    if command.is_empty() {
        return Err(String::from(
            "usage: sigil [--rootfs PATH] [--memory LIMIT] [--container-id ID] -- <command> [args...]",
        ));
    }

    if rootfs.is_relative() {
        rootfs = std::env::current_dir()
            .map_err(|err| format!("failed to resolve current directory: {}", err))?
            .join(rootfs);
    }

    Ok(ContainerConfig {
        rootfs,
        command,
        cgroup_name,
        memory_max,
    })
}
