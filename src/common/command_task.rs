//! Generic command task for ZMQ REP socket handling
//!
//! This module provides a reusable command handler task that:
//! - Binds a ZMQ REP socket to listen for commands
//! - Deserializes incoming JSON commands
//! - Calls a handler function to process commands
//! - Serializes and sends responses
//! - Handles graceful shutdown

use super::command::{Command, CommandResponse, ComponentState};
use std::sync::Arc;
use tmq::{request_reply, Context};
use tokio::sync::{broadcast, watch, Mutex};
use tracing::{info, warn};

/// Run a command handler task with a custom handler function
///
/// This function encapsulates the common pattern of:
/// 1. Binding a ZMQ REP socket
/// 2. Receiving JSON commands in a loop
/// 3. Calling a handler to process commands
/// 4. Sending JSON responses
/// 5. Handling shutdown signals
///
/// # Type Parameters
/// * `S` - State type (must be Send + 'static)
/// * `F` - Handler function type
///
/// # Arguments
/// * `command_address` - ZMQ address to bind (e.g., "tcp://*:5560")
/// * `shared_state` - Arc<Mutex<S>> containing the component's state
/// * `state_tx` - Watch channel sender for broadcasting state changes
/// * `shutdown` - Broadcast receiver for shutdown signals
/// * `handler` - Function that processes commands and returns responses
/// * `component_name` - Name for logging
///
/// # Handler Function
/// The handler function takes:
/// - `&mut S`: Mutable reference to the state
/// - `&watch::Sender<ComponentState>`: For broadcasting state changes
/// - `Command`: The command to process
///
/// And returns a `CommandResponse`.
pub async fn run_command_task<S, F>(
    command_address: String,
    shared_state: Arc<Mutex<S>>,
    state_tx: watch::Sender<ComponentState>,
    mut shutdown: broadcast::Receiver<()>,
    handler: F,
    component_name: &'static str,
) where
    S: Send + 'static,
    F: Fn(&mut S, &watch::Sender<ComponentState>, Command) -> CommandResponse + Send + 'static,
{
    let context = Context::new();

    let receiver = match request_reply::reply(&context).bind(&command_address) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                component = component_name,
                error = %e,
                address = %command_address,
                "Failed to bind command socket"
            );
            return;
        }
    };

    info!(
        component = component_name,
        address = %command_address,
        "Command task started"
    );

    let mut current_receiver = receiver;

    loop {
        tokio::select! {
            biased;

            _ = shutdown.recv() => {
                info!(component = component_name, "Command task received shutdown signal");
                break;
            }

            recv_result = current_receiver.recv() => {
                match recv_result {
                    Ok((mut multipart, sender)) => {
                        let response = if let Some(frame) = multipart.pop_front() {
                            match Command::from_json(&frame) {
                                Ok(cmd) => {
                                    info!(
                                        component = component_name,
                                        command = %cmd,
                                        "Received command"
                                    );
                                    let mut state = shared_state.lock().await;
                                    handler(&mut state, &state_tx, cmd)
                                }
                                Err(e) => {
                                    warn!(
                                        component = component_name,
                                        error = %e,
                                        "Invalid command"
                                    );
                                    // Need to get current state for error response
                                    // We don't have access to it here without locking
                                    CommandResponse::error(
                                        ComponentState::Idle, // Fallback
                                        format!("Invalid command: {}", e),
                                    )
                                }
                            }
                        } else {
                            CommandResponse::error(ComponentState::Idle, "Empty message")
                        };

                        let resp_bytes = match response.to_json() {
                            Ok(b) => b,
                            Err(e) => {
                                warn!(
                                    component = component_name,
                                    error = %e,
                                    "Failed to serialize response"
                                );
                                break;
                            }
                        };

                        let resp_msg: tmq::Multipart =
                            vec![tmq::Message::from(resp_bytes.as_slice())].into();

                        match sender.send(resp_msg).await {
                            Ok(next_receiver) => {
                                current_receiver = next_receiver;
                            }
                            Err(e) => {
                                warn!(
                                    component = component_name,
                                    error = %e,
                                    "Failed to send response"
                                );
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            component = component_name,
                            error = %e,
                            "Command receive error"
                        );
                        break;
                    }
                }
            }
        }
    }

    info!(component = component_name, "Command task stopped");
}

/// Simplified version that works with ComponentSharedState
///
/// This version is for components that use the standard ComponentSharedState
/// and CommandHandlerExt trait.
pub async fn run_command_task_with_state(
    command_address: String,
    shared_state: Arc<Mutex<super::state::ComponentSharedState>>,
    state_tx: watch::Sender<ComponentState>,
    shutdown: broadcast::Receiver<()>,
    component_name: &'static str,
) {
    run_command_task(
        command_address,
        shared_state,
        state_tx,
        shutdown,
        move |state, tx, cmd| super::state::handle_command_simple(state, tx, cmd, component_name),
        component_name,
    )
    .await
}

#[cfg(test)]
mod tests {
    // Integration tests would require ZMQ socket setup
    // Unit tests for the handler logic are in state.rs
}
