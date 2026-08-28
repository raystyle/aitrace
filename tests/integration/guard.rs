//! Kill spawned aitrace children when a test panics, so they cannot leak
//! `target\debug\aitrace.exe` (Windows then refuses to relink).

use std::process::Child;

pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
