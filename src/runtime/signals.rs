use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub enum SignalEvent {
    Forward(Signal),
    Reap,
}

static FORWARDED_SIGNAL: AtomicI32 = AtomicI32::new(0);
static GOT_CHILD_EVENT: AtomicBool = AtomicBool::new(false);

extern "C" fn signal_handler(sig: i32) {
    if let Ok(signal) = Signal::try_from(sig) {
        match signal {
            Signal::SIGINT | Signal::SIGTERM | Signal::SIGQUIT | Signal::SIGHUP => {
                FORWARDED_SIGNAL.store(sig, Ordering::SeqCst);
            }
            Signal::SIGCHLD => {
                GOT_CHILD_EVENT.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

pub fn install() {
    let action = SigAction::new(
        SigHandler::Handler(signal_handler),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );

    let signals_to_forward = [
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGQUIT,
        Signal::SIGHUP,
    ];

    for sig in signals_to_forward {
        unsafe {
            signal::sigaction(sig, &action).expect("failed to set signal handler");
        }
    }
    let chld_action = SigAction::new(
        SigHandler::Handler(signal_handler),
        SaFlags::SA_RESTART | SaFlags::SA_NOCLDSTOP,
        SigSet::empty(),
    );

    unsafe {
        signal::sigaction(Signal::SIGCHLD, &chld_action).expect("failed to set SIGCHLD handler");
    }
}

pub fn check_signals() -> Option<SignalEvent> {
    let signal = FORWARDED_SIGNAL.swap(0, Ordering::SeqCst);
    if signal != 0 {
        return Signal::try_from(signal).ok().map(SignalEvent::Forward);
    }

    if GOT_CHILD_EVENT.swap(false, Ordering::SeqCst) {
        return Some(SignalEvent::Reap);
    }

    None
}
