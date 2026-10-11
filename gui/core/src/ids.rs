//! Stable identities. IDs are process-unique and never reused, so a late
//! reply for a closed connection or a superseded request can always be
//! recognised and dropped.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            /// Allocate a fresh, never-reused id.
            pub fn next() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                Self(NEXT.fetch_add(1, Ordering::Relaxed))
            }

            pub fn as_u64(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

define_id!(
    /// A live connection (one worker thread, one `Database`).
    ConnectionId,
    "conn-"
);
define_id!(
    /// One operation sent to a connection worker (query, explain, …).
    RequestId,
    "req-"
);
define_id!(
    /// A workspace tab (query editor, table inspector, …).
    TabId,
    "tab-"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_display_with_prefix() {
        let a = ConnectionId::next();
        let b = ConnectionId::next();
        assert_ne!(a, b);
        assert!(a.to_string().starts_with("conn-"));
        assert!(RequestId::next().to_string().starts_with("req-"));
    }
}
