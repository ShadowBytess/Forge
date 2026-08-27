//! Cooperative cancellation token shared between the UI, the CLI and the
//! transaction executor.

use tokio::sync::watch;

/// Cloneable handle signalling "please stop" to a running operation.
#[derive(Clone, Debug)]
pub struct CancelToken {
    tx: watch::Sender<bool>,
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self { tx }
    }

    /// Request cancellation of every task holding a clone of this token.
    ///
    /// Uses `send_replace` so it also works when no receiver currently
    /// exists (late subscribers still observe the flag).
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    /// Non-blocking check.
    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    /// Resolves once [`CancelToken::cancel`] has been called.
    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Runs `fut`, returning `Err(ForgeError::Cancelled)` if the token fires
    /// before `fut` completes.
    pub async fn guard<T>(
        &self,
        fut: impl std::future::Future<Output = crate::error::Result<T>>,
    ) -> crate::error::Result<T> {
        tokio::select! {
            biased;
            _ = self.cancelled() => Err(crate::error::ForgeError::Cancelled),
            res = fut => res,
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signals_cancellation() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        let t2 = token.clone();
        token.cancel();
        assert!(t2.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_millis(50), t2.cancelled())
            .await
            .expect("cancelled() should resolve after cancel()");
    }

    #[tokio::test]
    async fn guard_returns_cancelled_error() {
        use crate::error::ForgeError;
        let token = CancelToken::new();
        token.cancel();
        let res: Result<(), ForgeError> = token
            .guard(async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok(())
            })
            .await;
        assert!(matches!(res, Err(ForgeError::Cancelled)));
    }
}
