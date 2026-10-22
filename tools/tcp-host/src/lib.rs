//! Host-side harness for the kernel's TCP engine: the same source files and
//! the same lossy-wire simulation the boot test runs, without booting.
#![allow(dead_code)]

#[path = "../../../kernel/src/ip.rs"]
pub mod ip;

#[path = "../../../kernel/src/sockopt.rs"]
pub mod sockopt;

#[path = "../../../kernel/src/tcp.rs"]
pub mod tcp;

pub mod sync {
    pub struct TicketLock<T>(std::sync::Mutex<T>);

    impl<T> TicketLock<T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }

        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_self_test() {
        let handle = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(super::tcp::self_test)
            .unwrap();
        let report = handle.join().unwrap();
        assert!(report.verified, "TCP engine self-test failed");
    }
}
