//! CAEN FELib wrapper module
//!
//! Safe Rust bindings for CAEN digitizer access via FELib.

pub mod error;
pub mod ffi;
pub mod handle;

// Re-exports for convenience
pub use error::CaenError;
pub use handle::{CaenHandle, DeviceInfo, EndpointHandle, ParamInfo, RawData};
