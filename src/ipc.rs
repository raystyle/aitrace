//! Local stream socket used by the daemon.
//!
//! Unix: `std::os::unix::net`. Windows: `uds_windows` (Win10 1809+ AF_UNIX).

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};

use std::io;
use std::path::Path;

/// Bind a listener, removing any leftover socket file first.
///
/// Windows AF_UNIX leaves an NTFS reparse point; `bind` fails unless that
/// file is deleted. `exists()` is unreliable on that reparse point.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path)
}

/// Connect to a bound daemon socket.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(path)
}
