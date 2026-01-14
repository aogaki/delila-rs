//! CAEN FELib error handling
//!
//! Provides safe Rust error types wrapping CAEN_FELib_ErrorCode.

use super::ffi;
use std::ffi::CStr;
use std::fmt;
use thiserror::Error;

/// CAEN FELib error type
#[derive(Debug, Clone, Error)]
pub struct CaenError {
    pub code: i32,
    pub name: String,
    pub description: String,
}

impl fmt::Display for CaenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CAEN error {}: {} - {}",
            self.code, self.name, self.description
        )
    }
}

impl CaenError {
    /// Create a CaenError from an error code
    pub fn from_code(code: i32) -> Option<Self> {
        if code == 0 {
            return None; // Success
        }

        let mut name_buf = [0i8; 32];
        let mut desc_buf = [0i8; 256];

        // Convert i32 to CAEN_FELib_ErrorCode enum
        // Safety: The enum values match the i32 error codes from the C API
        let error_code: ffi::CAEN_FELib_ErrorCode = unsafe { std::mem::transmute(code) };

        unsafe {
            ffi::CAEN_FELib_GetErrorName(error_code, name_buf.as_mut_ptr());
            ffi::CAEN_FELib_GetErrorDescription(error_code, desc_buf.as_mut_ptr());
        }

        let name = unsafe {
            CStr::from_ptr(name_buf.as_ptr())
                .to_string_lossy()
                .into_owned()
        };

        let description = unsafe {
            CStr::from_ptr(desc_buf.as_ptr())
                .to_string_lossy()
                .into_owned()
        };

        Some(Self {
            code,
            name,
            description,
        })
    }

    /// Check result and convert to Result
    pub fn check(code: i32) -> Result<(), Self> {
        match Self::from_code(code) {
            None => Ok(()),
            Some(err) => Err(err),
        }
    }
}

/// Common CAEN error codes (for pattern matching)
pub mod codes {
    pub const SUCCESS: i32 = 0;
    pub const GENERIC_ERROR: i32 = -1;
    pub const INVALID_PARAM: i32 = -2;
    pub const DEVICE_ALREADY_OPEN: i32 = -3;
    pub const DEVICE_NOT_FOUND: i32 = -4;
    pub const MAX_DEVICES_ERROR: i32 = -5;
    pub const COMMAND_ERROR: i32 = -6;
    pub const INTERNAL_ERROR: i32 = -7;
    pub const NOT_IMPLEMENTED: i32 = -8;
    pub const INVALID_HANDLE: i32 = -9;
    pub const DEVICE_LIBRARY_NOT_AVAILABLE: i32 = -10;
    pub const TIMEOUT: i32 = -11;
    pub const STOP: i32 = -12;
    pub const DISABLED: i32 = -13;
    pub const BAD_LIBRARY_VERSION: i32 = -14;
    pub const COMMUNICATION_ERROR: i32 = -15;
}
