//! Safe wrapper for CAEN device handle
//!
//! Provides RAII-based handle management (like C++ std::unique_ptr with custom deleter).

use super::error::CaenError;
use super::ffi;
use std::ffi::CString;

// C wrapper for variadic CAEN_FELib_ReadData function
// Rust cannot directly call C variadic functions on all platforms (especially macOS ARM64).
// We use a C wrapper function compiled via cc crate.
extern "C" {
    /// C wrapper for CAEN_FELib_ReadData with RAW format
    /// Defined in wrapper.c, compiled by build.rs
    fn caen_read_data_raw(
        handle: u64,
        timeout: std::os::raw::c_int,
        data: *mut u8,
        size: *mut usize,
        n_events: *mut u32,
    ) -> std::os::raw::c_int;
}

/// Safe wrapper for CAEN device handle
///
/// Automatically closes the device when dropped (RAII pattern).
/// Equivalent to C++ unique_ptr<void, CaenDeleter>.
pub struct CaenHandle {
    handle: u64,
}

/// Handle for data endpoint (for ReadData operations)
///
/// This is a sub-handle obtained from the main device handle.
/// It does NOT implement Drop - it's just a reference to an internal resource.
pub struct EndpointHandle {
    handle: u64,
}

/// Raw data read result
#[derive(Debug)]
pub struct RawData {
    /// Raw binary data from digitizer
    pub data: Vec<u8>,
    /// Actual size of valid data in bytes
    pub size: usize,
    /// Number of events in this data block
    pub n_events: u32,
}

impl CaenHandle {
    /// Open a connection to a CAEN device
    ///
    /// # Arguments
    /// * `url` - Device URL (e.g., "dig2://172.18.4.56")
    ///
    /// # Example
    /// ```no_run
    /// use delila_rs::reader::caen::CaenHandle;
    /// let handle = CaenHandle::open("dig2://172.18.4.56").unwrap();
    /// ```
    pub fn open(url: &str) -> Result<Self, CaenError> {
        let c_url = CString::new(url).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "URL contains null byte".to_string(),
        })?;

        let mut handle: u64 = 0;
        let ret = unsafe { ffi::CAEN_FELib_Open(c_url.as_ptr(), &mut handle) };

        CaenError::check(ret)?;
        Ok(Self { handle })
    }

    /// Get the raw handle value (for advanced use)
    pub fn raw(&self) -> u64 {
        self.handle
    }

    /// Get device tree as JSON string
    pub fn get_device_tree(&self) -> Result<String, CaenError> {
        // First call to get required buffer size
        let size = unsafe { ffi::CAEN_FELib_GetDeviceTree(self.handle, std::ptr::null_mut(), 0) };

        if size <= 0 {
            return Err(CaenError {
                code: size,
                name: "GetDeviceTreeError".to_string(),
                description: "Failed to get device tree size".to_string(),
            });
        }

        // Allocate buffer and get the actual data
        let mut buffer = vec![0i8; (size + 1) as usize];
        let ret = unsafe {
            ffi::CAEN_FELib_GetDeviceTree(self.handle, buffer.as_mut_ptr(), buffer.len())
        };

        if ret < 0 {
            return Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Failed to get device tree".to_string(),
            }));
        }

        // Convert to Rust string
        let c_str = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        Ok(c_str.to_string_lossy().into_owned())
    }

    /// Get a parameter value
    ///
    /// # Arguments
    /// * `path` - Parameter path (e.g., "/par/ModelName")
    pub fn get_value(&self, path: &str) -> Result<String, CaenError> {
        let c_path = CString::new(path).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Path contains null byte".to_string(),
        })?;

        let mut buffer = [0i8; 256];
        let ret = unsafe { ffi::CAEN_FELib_GetValue(self.handle, c_path.as_ptr(), buffer.as_mut_ptr()) };

        CaenError::check(ret)?;

        let c_str = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        Ok(c_str.to_string_lossy().into_owned())
    }

    /// Set a parameter value
    ///
    /// # Arguments
    /// * `path` - Parameter path (e.g., "/ch/0/par/ChEnable")
    /// * `value` - Value to set (e.g., "True")
    pub fn set_value(&self, path: &str, value: &str) -> Result<(), CaenError> {
        let c_path = CString::new(path).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Path contains null byte".to_string(),
        })?;

        let c_value = CString::new(value).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Value contains null byte".to_string(),
        })?;

        let ret = unsafe { ffi::CAEN_FELib_SetValue(self.handle, c_path.as_ptr(), c_value.as_ptr()) };

        CaenError::check(ret)
    }

    /// Send a command to the device
    ///
    /// # Arguments
    /// * `path` - Command path (e.g., "/cmd/Reset")
    pub fn send_command(&self, path: &str) -> Result<(), CaenError> {
        let c_path = CString::new(path).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Path contains null byte".to_string(),
        })?;

        let ret = unsafe { ffi::CAEN_FELib_SendCommand(self.handle, c_path.as_ptr()) };

        CaenError::check(ret)
    }

    /// Get a sub-handle for a given path
    ///
    /// # Arguments
    /// * `path` - Path to the resource (e.g., "/endpoint/RAW")
    pub fn get_handle(&self, path: &str) -> Result<u64, CaenError> {
        let c_path = CString::new(path).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Path contains null byte".to_string(),
        })?;

        let mut sub_handle: u64 = 0;
        let ret = unsafe { ffi::CAEN_FELib_GetHandle(self.handle, c_path.as_ptr(), &mut sub_handle) };

        CaenError::check(ret)?;
        Ok(sub_handle)
    }

    /// Get parent handle of a given handle
    ///
    /// # Arguments
    /// * `handle` - The handle to get parent of
    pub fn get_parent_handle(&self, handle: u64) -> Result<u64, CaenError> {
        let mut parent_handle: u64 = 0;
        let ret = unsafe {
            ffi::CAEN_FELib_GetParentHandle(handle, std::ptr::null(), &mut parent_handle)
        };

        CaenError::check(ret)?;
        Ok(parent_handle)
    }

    /// Set value using a sub-handle
    ///
    /// # Arguments
    /// * `handle` - Sub-handle to use
    /// * `path` - Parameter path
    /// * `value` - Value to set
    pub fn set_value_with_handle(&self, handle: u64, path: &str, value: &str) -> Result<(), CaenError> {
        let c_path = CString::new(path).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Path contains null byte".to_string(),
        })?;

        let c_value = CString::new(value).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Value contains null byte".to_string(),
        })?;

        let ret = unsafe { ffi::CAEN_FELib_SetValue(handle, c_path.as_ptr(), c_value.as_ptr()) };

        CaenError::check(ret)
    }

    /// Configure endpoint for RAW data reading
    ///
    /// This sets up the RAW endpoint and returns an EndpointHandle for data reading.
    /// Follows the C++ pattern from Digitizer2::EndpointConfigure()
    pub fn configure_endpoint(&self) -> Result<EndpointHandle, CaenError> {
        // Get endpoint handle
        let ep_handle = self.get_handle("/endpoint/RAW")?;

        // Get parent (endpoint folder) handle
        let ep_folder_handle = self.get_parent_handle(ep_handle)?;

        // Set active endpoint to RAW
        self.set_value_with_handle(ep_folder_handle, "/par/activeendpoint", "RAW")?;

        // Get fresh handle for read operations
        let read_data_handle = self.get_handle("/endpoint/RAW")?;

        // Set data format (RAW format with DATA, SIZE, N_EVENTS)
        let format_json = r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0},
            {"name": "N_EVENTS", "type": "U32", "dim": 0}
        ]"#;

        let c_format = CString::new(format_json).map_err(|_| CaenError {
            code: -2,
            name: "InvalidParam".to_string(),
            description: "Format JSON contains null byte".to_string(),
        })?;

        let ret = unsafe { ffi::CAEN_FELib_SetReadDataFormat(read_data_handle, c_format.as_ptr()) };
        CaenError::check(ret)?;

        Ok(EndpointHandle { handle: read_data_handle })
    }
}

impl EndpointHandle {
    /// Get the raw handle value
    pub fn raw(&self) -> u64 {
        self.handle
    }

    /// Check if data is available
    ///
    /// # Arguments
    /// * `timeout_ms` - Timeout in milliseconds
    ///
    /// # Returns
    /// * `Ok(true)` - Data is available
    /// * `Ok(false)` - Timeout (no data available)
    /// * `Err(...)` - Error occurred
    pub fn has_data(&self, timeout_ms: i32) -> Result<bool, CaenError> {
        let ret = unsafe { ffi::CAEN_FELib_HasData(self.handle, timeout_ms) };

        if ret == 0 {
            // CAEN_FELib_Success
            Ok(true)
        } else if ret == -11 {
            // CAEN_FELib_Timeout
            Ok(false)
        } else {
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Unknown error in HasData".to_string(),
            }))
        }
    }

    /// Read raw data from the endpoint
    ///
    /// # Arguments
    /// * `timeout_ms` - Timeout in milliseconds (-1 for infinite)
    /// * `buffer_size` - Maximum buffer size for raw data
    ///
    /// # Returns
    /// * `Ok(Some(RawData))` - Data was read successfully
    /// * `Ok(None)` - Timeout (no data available)
    /// * `Err(...)` - Error occurred
    ///
    /// # Safety
    /// This function uses variadic C FFI internally. The format must match
    /// what was configured in `configure_endpoint()`.
    pub fn read_data(&self, timeout_ms: i32, buffer_size: usize) -> Result<Option<RawData>, CaenError> {
        // Allocate buffer - must be pre-allocated like C++ version
        let mut data = vec![0u8; buffer_size];
        let mut size: usize = 0;
        let mut n_events: u32 = 0;

        // Call ReadData via C wrapper (handles variadic calling convention)
        let ret = unsafe {
            caen_read_data_raw(
                self.handle,
                timeout_ms,
                data.as_mut_ptr(),
                &mut size,
                &mut n_events,
            )
        };

        if ret == 0 {
            // Success - truncate data to actual size
            data.truncate(size);
            Ok(Some(RawData { data, size, n_events }))
        } else if ret == -11 {
            // Timeout
            Ok(None)
        } else if ret == -12 {
            // Stop signal (end of run)
            Ok(None)
        } else {
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Unknown error in ReadData".to_string(),
            }))
        }
    }
}

/// RAII: Automatically close the device when the handle is dropped
impl Drop for CaenHandle {
    fn drop(&mut self) {
        unsafe {
            // Ignore errors on close - we're in a destructor
            let _ = ffi::CAEN_FELib_Close(self.handle);
        }
    }
}

// CaenHandle is NOT Send/Sync because CAEN_FELib_Open/Close are not thread-safe
// according to the documentation. If thread safety is needed, wrap in Arc<Mutex<>>.
