use std::fmt;

#[derive(Debug)]
#[allow(dead_code)]
enum SyscallError {
    PermissionDenied,
    InvalidArgument,
    ResourceUnavailable,
    UnknownError(i32), // For unknown or OS-specific error codes
}

impl fmt::Display for SyscallError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SyscallError::PermissionDenied => write!(f, "permission denied"),
            SyscallError::InvalidArgument => write!(f, "Invalid argument"),
            SyscallError::ResourceUnavailable => write!(f, "Resource unavailable"),
            SyscallError::UnknownError(code) => write!(f, "Unknown syscall error: {}", code),
        }
    }
}
