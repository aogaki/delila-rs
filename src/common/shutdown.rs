//! Unified shutdown handling for DELILA components
//!
//! # Design Principles (KISS)
//! - Single function to setup Ctrl+C handler with broadcast channel
//! - Returns (sender, receiver) for component use
//! - Components call run(shutdown_rx) as before

use tokio::signal;
use tokio::sync::broadcast;
use tracing::info;

/// Shutdown signal type (unit type, just signals "shutdown now")
pub type ShutdownSignal = ();

/// Shutdown channel sender
pub type ShutdownSender = broadcast::Sender<ShutdownSignal>;

/// Shutdown channel receiver
pub type ShutdownReceiver = broadcast::Receiver<ShutdownSignal>;

/// Setup shutdown handling with Ctrl+C signal
///
/// Creates a broadcast channel and spawns a task that sends on Ctrl+C.
/// Returns (sender, receiver) - caller uses receiver for their component,
/// and can clone sender if needed for additional shutdown triggers.
///
/// # Example
/// ```ignore
/// let (_shutdown_tx, shutdown_rx) = setup_shutdown();
/// component.run(shutdown_rx).await?;
/// ```
pub fn setup_shutdown() -> (ShutdownSender, ShutdownReceiver) {
    let (tx, rx) = broadcast::channel::<ShutdownSignal>(1);

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Ctrl+C received, initiating shutdown");
        let _ = tx_clone.send(());
    });

    (tx, rx)
}

/// Setup shutdown with custom message
///
/// Same as `setup_shutdown` but allows custom log message.
pub fn setup_shutdown_with_message(message: &'static str) -> (ShutdownSender, ShutdownReceiver) {
    let (tx, rx) = broadcast::channel::<ShutdownSignal>(1);

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        println!("\n{}", message);
        let _ = tx_clone.send(());
    });

    (tx, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_channel_creation() {
        let (tx, mut rx) = broadcast::channel::<ShutdownSignal>(1);

        // Sending should work
        tx.send(()).unwrap();

        // Receiving should work
        let result = rx.recv().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_type_aliases() {
        // Just verify the type aliases compile correctly
        fn _takes_sender(_: ShutdownSender) {}
        fn _takes_receiver(_: ShutdownReceiver) {}
    }
}
