# Sigil

Sigil is a low-level container runtime written in Rust. It implements process supervision, Linux namespaces, filesystem isolation, and cgroups from scratch to demonstrate how containers actually work under the hood.

The goal of this project is to understand and build the core primitives behind tools like Docker and runc rather than treating them as black boxes.

<img width="965" height="708" alt="image" src="https://github.com/user-attachments/assets/6521dfa2-cf7e-48c1-a0a9-5f1d35238086" />


---

## Features

* Process supervision with proper signal forwarding
* PID namespace with init process (PID 1) handling
* UTS namespace for hostname isolation
* Mount namespace with private propagation
* Root filesystem isolation using pivot_root
* Proc filesystem mounted inside container
* Basic cgroups v2 support for memory limits
* Clean separation between runtime and isolation layers

---

## Architecture

Sigil is structured into clearly separated components:

```
src/
  runtime/
    supervisor.rs   # process lifecycle, signals, waitpid
    child.rs        # child bootstrap and init (PID 1)
    signals.rs      # signal handling
  isolation/
    namespaces.rs   # namespace setup
    mounts.rs       # mount propagation
    fs.rs           # rootfs and pivot_root
    cgroups.rs      # cgroup v2 management
  config/           # CLI and runtime configuration
```

Design principles:

* Supervisor owns lifecycle and is the only place that calls waitpid for the container process
* Child becomes PID 1 inside the namespace and is responsible for reaping orphaned processes
* Isolation logic is strictly separated from runtime logic
* All namespace and filesystem setup happens before exec

---

## How It Works

1. Sigil starts as the supervisor process
2. It creates a child process using clone with namespace flags
3. The child blocks until the parent completes setup
4. The child configures isolation:

   * mount namespace and private propagation
   * pivot_root into container filesystem
   * mount /proc
   * attach to cgroup
5. The child becomes PID 1 and forks the target process
6. PID 1 reaps orphaned processes and exits with the child status
7. The supervisor forwards signals and waits for container exit

---

## Requirements

* Linux kernel with namespace and cgroup v2 support
* Root privileges or appropriate capabilities
* Rust toolchain

---

## Usage

Prepare a minimal root filesystem:

```
mkdir -p rootfs/bin
cp /bin/sh rootfs/bin/
```

Run a container:

```
sudo cargo run -- --rootfs ./rootfs -- /bin/sh
```

Inside the container:

```
hostname
ps
```

The process list and hostname should be isolated from the host.

---

## Limitations

* Networking namespace not implemented
* No seccomp filtering
* No capability dropping
* Minimal cgroup support
* No OCI compatibility layer

---

## References

* man 2 clone
* man 7 namespaces
* man 2 pivot_root
* man 7 cgroups
* runc source code
