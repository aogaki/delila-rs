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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_returns_none() {
        assert!(CaenError::from_code(codes::SUCCESS).is_none());
    }

    #[test]
    fn test_error_codes_return_some() {
        // Test various error codes return Some
        let error = CaenError::from_code(codes::GENERIC_ERROR);
        assert!(error.is_some());
        let err = error.unwrap();
        assert_eq!(err.code, codes::GENERIC_ERROR);

        let error = CaenError::from_code(codes::TIMEOUT);
        assert!(error.is_some());
        assert_eq!(error.unwrap().code, codes::TIMEOUT);
    }

    #[test]
    fn test_check_success() {
        assert!(CaenError::check(codes::SUCCESS).is_ok());
    }

    #[test]
    fn test_check_error() {
        let result = CaenError::check(codes::INVALID_PARAM);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAM);
    }

    #[test]
    fn test_error_display() {
        let error = CaenError {
            code: -1,
            name: "GenericError".to_string(),
            description: "A generic error occurred".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("CAEN error -1"));
        assert!(display.contains("GenericError"));
        assert!(display.contains("A generic error occurred"));
    }

    #[test]
    fn test_error_debug() {
        let error = CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Invalid parameter".to_string(),
        };
        let debug = format!("{:?}", error);
        assert!(debug.contains("CaenError"));
        assert!(debug.contains("-2"));
    }

    #[test]
    fn test_all_error_codes() {
        // Verify all defined error codes
        let codes_to_test = [
            (codes::GENERIC_ERROR, -1),
            (codes::INVALID_PARAM, -2),
            (codes::DEVICE_ALREADY_OPEN, -3),
            (codes::DEVICE_NOT_FOUND, -4),
            (codes::MAX_DEVICES_ERROR, -5),
            (codes::COMMAND_ERROR, -6),
            (codes::INTERNAL_ERROR, -7),
            (codes::NOT_IMPLEMENTED, -8),
            (codes::INVALID_HANDLE, -9),
            (codes::DEVICE_LIBRARY_NOT_AVAILABLE, -10),
            (codes::TIMEOUT, -11),
            (codes::STOP, -12),
            (codes::DISABLED, -13),
            (codes::BAD_LIBRARY_VERSION, -14),
            (codes::COMMUNICATION_ERROR, -15),
        ];

        for (code, expected) in codes_to_test {
            assert_eq!(code, expected, "Error code constant mismatch");
        }
    }

    #[test]
    fn test_error_clone() {
        let error = CaenError {
            code: -5,
            name: "TestError".to_string(),
            description: "Test description".to_string(),
        };
        let cloned = error.clone();
        assert_eq!(error.code, cloned.code);
        assert_eq!(error.name, cloned.name);
        assert_eq!(error.description, cloned.description);
    }
}
