//! Running DBCrust's async (tokio, often `!Send`) code from the UI.
//!
//! GPUI's executors are not tokio runtimes. Discovery work that never
//! touches a connection (Docker, Vault) runs on a short-lived thread with
//! its own current-thread runtime; the UI awaits a oneshot receiver, which
//! works on any executor.

use crate::error::{GuiError, GuiResult};
use std::future::Future;

/// Prefix of every thread this crate spawns. Under `cargo test` it contains
/// "test", which DBCrust core uses to isolate config files in a temp dir.
pub(crate) const THREAD_PREFIX: &str = if cfg!(test) {
    "dbcrust-test"
} else {
    "dbcrust"
};

/// Run `make()` to completion on a fresh thread with its own tokio runtime
/// and return a `Send` future resolving to its output. The produced future
/// may be `!Send`; only its output crosses threads.
pub fn run_detached<T, F, Fut>(
    name: &str,
    make: F,
) -> impl Future<Output = GuiResult<T>> + Send + 'static
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = GuiResult<T>> + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("{THREAD_PREFIX}-{name}"))
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = tx.send(Err(GuiError::Unsupported(format!(
                        "Could not start a background runtime: {e}"
                    ))));
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            let output = local.block_on(&runtime, make());
            let _ = tx.send(output);
        });
    async move {
        if let Err(e) = spawned {
            return Err(GuiError::Unsupported(format!(
                "Could not start a background thread: {e}"
            )));
        }
        rx.await
            .unwrap_or_else(|_| Err(GuiError::Unsupported("Background task stopped".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_non_send_futures_and_returns_output() {
        let future = run_detached("test", || async {
            let not_send = std::rc::Rc::new(41);
            tokio::task::yield_now().await;
            Ok(*not_send + 1)
        });
        let output = futures_util::FutureExt::now_or_never(async {
            // Poll on a plain thread: the oneshot needs no runtime
            let mut future = Box::pin(future);
            loop {
                if let Some(value) = futures_util::FutureExt::now_or_never(&mut future) {
                    break value;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        assert_eq!(output, Some(Ok(42)));
    }
}
