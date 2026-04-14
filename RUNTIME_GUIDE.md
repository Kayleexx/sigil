# Sigil Runtime Guide (Phase 1.5)

This document explains the runtime from start to finish in simple, practical terms.
It is focused on the code in:

- `src/main.rs`
- `src/runtime/supervisor.rs`
- `src/runtime/child.rs`
- `src/runtime/signals.rs`

The goal is to make the control flow and kernel behavior easy to reason about.

## 1) What the runtime is responsible for

Runtime scope in Sigil:

- Spawn one child command.
- Supervise child lifecycle.
- Forward termination signals to child process group.
- Reap child using `waitpid`.
- Exit with child's exit status.

Runtime does **not** yet do:

- Namespaces/mount/pivot root (`isolation/` later).
- Cgroup resource controls (`resources/` later).

## 2) High-level lifecycle

When you run:

```bash
cargo run -- /bin/sleep 10
```

the runtime flow is:

1. `main` parses CLI args.
2. `main` calls `runtime::supervisor::spawn(args)`.
3. `spawn` installs signal handlers.
4. `spawn` creates a pipe for parent-child synchronization.
5. `spawn` calls `fork()`.
6. Parent branch:
   - drops read end of pipe.
   - moves child into its own process group (`setpgid(child, child)`).
   - writes one byte to release child.
   - enters supervisor loop.
7. Child branch:
   - drops write end of pipe.
   - blocks on read until parent releases it.
   - `execvp(...)` target command.
8. Supervisor keeps checking:
   - pending signal events from atomics.
   - child state via `waitpid(WNOHANG)`.
9. On child exit/termination, supervisor returns exit code.
10. `main` exits with same code.

## 3) Why signal installation must happen before fork

`signals::install()` is called before `fork()` in `spawn`.

Reason:

- Signal handlers are process state.
- `fork()` clones process state to child.
- If handlers are installed late, a signal can arrive in a race window and use default behavior.

What can break if installed after fork:

- Parent may die from default SIGINT/SIGTERM before supervising child.
- SIGCHLD wake signal can be missed before handler exists.
- Behavior becomes timing-dependent and flaky.

## 4) Pipe sync: what it solves

The pipe creates an explicit "release gate" for child bootstrap.

Parent sequence:

- `setpgid(child, child)` first
- then write one byte to pipe

Child sequence:

- block in `read`
- continue only after parent writes

This prevents race conditions where parent tries to signal process group before it exists.

## 5) File descriptor ownership model

Sigil now uses `OwnedFd` semantics from `nix::unistd::pipe()`.

Rule:

- If you own an `OwnedFd`, dropping it closes it.
- Do not convert to raw fd + manually close unless you intentionally transfer ownership.

Current pattern:

- Parent: `drop(reader)`, `write(&writer, ...)`, `drop(writer)`
- Child: `drop(writer)`, `read(&reader, ...)`, `drop(reader)` in bootstrap

Why this is safer:

- No double-close risk.
- No borrowed-vs-owned confusion.
- Lifetime/ownership is visible in function signatures.

## 6) Process groups: why they matter

The child is placed in its own process group:

```text
setpgid(child_pid, child_pid)
```

Then supervisor can signal entire group:

```text
kill(-child_pid, SIGTERM)
```

Negative PID in `kill` means "send to process group id".

Why you need this:

- Child command may spawn grandchildren.
- Killing only direct child can leave descendants running.
- Group signal gives consistent full shutdown behavior.

## 7) Signal handling model in Sigil

`signals.rs` uses async-signal-safe handlers that only set atomic flags.

Flags:

- `GOT_TERMINATION` for `SIGINT`, `SIGTERM`, `SIGQUIT`, `SIGHUP`
- `GOT_CHILD_EVENT` for `SIGCHLD`

`check_signals()` converts flags into events:

- `SignalEvent::Terminate`
- `SignalEvent::Reap`

Important behavior:

- `Terminate` => supervisor forwards `SIGTERM` to child process group.
- `Reap` => supervisor wakes and checks `waitpid`; it does **not** forward a signal.

Why forwarding SIGCHLD is wrong:

- SIGCHLD means "state changed", not "please terminate child".
- Forwarding it would create accidental kills.

## 8) Supervisor loop: kernel truth source

Main loop in `supervisor_loop(child_pid)`:

1. Poll signal atomics.
2. If terminate requested, send SIGTERM to child process group.
3. Call `waitpid(child_pid, WNOHANG)`.
4. Interpret result.

`waitpid` outcomes:

- `Exited(_, status)` => child exited normally.
- `Signaled(_, sig, _)` => child died from signal.
- `StillAlive` => no state change yet.
- `EINTR` => interrupted syscall, retry.
- `ECHILD` => no child to wait for.

Why `waitpid` is the source of truth:

- Signals are notifications and can be coalesced.
- Only `waitpid` provides actual exit state and reaps zombies.

## 9) Exit status propagation

Supervisor returns integer status:

- Normal exit code as-is (for example `7`).
- Signaled exit mapped to `128 + signal_number` (shell convention).

`main` exits using that same code.

Why this matters:

- Shell scripting (`&&`, `||`, CI jobs) depends on correct exit status.
- Runtime behaves predictably like standard process runners.

## 10) Error handling strategy used

Critical syscalls return `nix::Result` and are not ignored:

- `pipe`, `fork`, `setpgid`, `read`, `write`, `execvp`, `waitpid`

Current strategy:

- Parent path propagates errors up (`?`).
- Child path logs bootstrap failure and exits non-zero.

This is intentionally simple and sufficient for Phase 1.5.

## 11) End-to-end example timeline

Example command:

```bash
cargo run -- /bin/sh -c "sleep 100"
```

Timeline:

1. Parent installs handlers.
2. Parent forks child.
3. Parent sets child process group.
4. Parent releases child via pipe write.
5. Child reads byte, then `execvp("/bin/sh", ...)`.
6. Supervisor loop runs with `waitpid(WNOHANG)`.
7. User presses `Ctrl+C`.
8. Parent receives SIGINT -> sets termination flag.
9. Supervisor sees `Terminate` -> sends `SIGTERM` to `-child_pid`.
10. Shell and sleep in child group terminate.
11. Kernel reports child exit via `waitpid`.
12. Supervisor returns status.
13. Main exits with same status.

## 12) Common failure modes to remember

- Installing handlers after fork introduces race windows.
- Letting child own `setpgid` can race group-targeted `kill`.
- Treating SIGCHLD as terminate signal causes wrong shutdowns.
- Ignoring syscall errors can deadlock bootstrap (pipe write/read mismatch).
- Not calling `waitpid` reliably can leak zombies.

## 13) Quick testing commands

Exit propagation:

```bash
cargo run -- /bin/sh -c "exit 7"; echo $?
cargo run -- /bin/true; echo $?
cargo run -- /bin/false; echo $?
```

Signal forwarding:

```bash
cargo run -- /bin/sleep 30
# press Ctrl+C
```

You should see runtime terminate child and exit cleanly.

## 14) Current architecture boundary (important invariant)

Sigil invariant:

> Exactly one component (Supervisor) observes and controls child lifecycle.

Practically this means:

- Only supervisor decides shutdown policy.
- Only supervisor owns wait/reap decisions.
- Child bootstrap remains minimal and policy-free.

Keeping this boundary clean now will make namespaces/cgroups integration much easier in later phases.
