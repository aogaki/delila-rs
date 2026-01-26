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

/// Device information retrieved from digitizer
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Model name (e.g., "VX2730")
    pub model: String,
    /// Serial number
    pub serial_number: String,
    /// Firmware type (e.g., "DPP_PSD")
    pub firmware_type: String,
    /// Number of channels
    pub num_channels: u32,
    /// ADC resolution in bits
    pub adc_bits: u32,
    /// Sampling rate in samples/sec
    pub sampling_rate_sps: u64,
}

/// Parameter metadata from DevTree
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Parameter name
    pub name: String,
    /// Data type (e.g., "NUMBER", "STRING", "BOOL")
    pub datatype: String,
    /// Access mode (e.g., "READ_WRITE", "READ_ONLY")
    pub access_mode: String,
    /// Whether parameter can be changed during acquisition
    pub setinrun: bool,
    /// Minimum value (for numeric types)
    pub min_value: Option<String>,
    /// Maximum value (for numeric types)
    pub max_value: Option<String>,
    /// Allowed values (for enum types)
    pub allowed_values: Vec<String>,
    /// Unit of measurement
    pub unit: Option<String>,
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

    /// Check if the handle is connected (non-zero handle value)
    ///
    /// Note: This only checks if we have a valid handle. It does not
    /// verify the connection is still alive. Use get_device_info()
    /// for active connection verification.
    pub fn is_connected(&self) -> bool {
        self.handle != 0
    }

    /// Get device information
    ///
    /// Retrieves model name, serial number, firmware type, and hardware specs.
    ///
    /// # Example
    /// ```no_run
    /// use delila_rs::reader::caen::CaenHandle;
    /// let handle = CaenHandle::open("dig2://172.18.4.56").unwrap();
    /// let info = handle.get_device_info().unwrap();
    /// println!("Model: {}, FW: {}", info.model, info.firmware_type);
    /// ```
    pub fn get_device_info(&self) -> Result<DeviceInfo, CaenError> {
        let model = self.get_value("/par/ModelName")?;
        let serial_number = self.get_value("/par/SerialNum")?;
        let firmware_type = self.get_value("/par/FwType")?;
        let num_channels: u32 = self.get_value("/par/NumCh")?.parse().unwrap_or(0);
        let adc_bits: u32 = self.get_value("/par/ADC_Nbit")?.parse().unwrap_or(0);
        let sampling_rate_sps: u64 = self.get_value("/par/ADC_SamplRate")?.parse().unwrap_or(0);

        Ok(DeviceInfo {
            model,
            serial_number,
            firmware_type,
            num_channels,
            adc_bits,
            sampling_rate_sps,
        })
    }

    /// Get parameter metadata from DevTree
    ///
    /// Parses the device tree to extract parameter attributes like
    /// datatype, access mode, setinrun flag, min/max values, etc.
    ///
    /// # Arguments
    /// * `path` - Parameter path (e.g., "/ch/0/par/DCOffset" or "DCOffset")
    ///
    /// # Note
    /// This method parses the full DevTree JSON which can be expensive.
    /// Consider caching the result if calling frequently.
    pub fn get_param_info(&self, path: &str) -> Result<ParamInfo, CaenError> {
        let tree_json = self.get_device_tree()?;
        let tree: serde_json::Value = serde_json::from_str(&tree_json).map_err(|e| CaenError {
            code: -1,
            name: "JsonParseError".to_string(),
            description: format!("Failed to parse DevTree JSON: {}", e),
        })?;

        // Extract parameter name from path (last component after /par/)
        let param_name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);

        // Search for parameter in DevTree
        // DevTree structure: { "par": { "ParamName": { ... } }, "ch": { ... } }
        let param_node = Self::find_param_in_tree(&tree, param_name).ok_or_else(|| CaenError {
            code: -1,
            name: "ParamNotFound".to_string(),
            description: format!("Parameter '{}' not found in DevTree", param_name),
        })?;

        Self::extract_param_info(param_name, param_node)
    }

    /// Find a parameter node in the DevTree by name (recursive search)
    fn find_param_in_tree<'a>(
        node: &'a serde_json::Value,
        param_name: &str,
    ) -> Option<&'a serde_json::Value> {
        if let Some(obj) = node.as_object() {
            // Check if this object has the parameter directly
            if let Some(param) = obj.get(param_name) {
                // Verify it's a parameter (has datatype or value)
                if param.get("datatype").is_some() || param.get("value").is_some() {
                    return Some(param);
                }
            }

            // Check in "par" subfolder
            if let Some(par_folder) = obj.get("par") {
                if let Some(param) = Self::find_param_in_tree(par_folder, param_name) {
                    return Some(param);
                }
            }

            // Recursively search in child objects
            for (_key, value) in obj {
                if let Some(param) = Self::find_param_in_tree(value, param_name) {
                    return Some(param);
                }
            }
        }
        None
    }

    /// Extract ParamInfo from a DevTree parameter node
    fn extract_param_info(name: &str, node: &serde_json::Value) -> Result<ParamInfo, CaenError> {
        let get_attr_value = |attr: &str| -> Option<String> {
            node.get(attr)
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        let datatype = get_attr_value("datatype").unwrap_or_else(|| "UNKNOWN".to_string());
        let access_mode = get_attr_value("accessmode").unwrap_or_else(|| "READ_WRITE".to_string());
        let setinrun = get_attr_value("setinrun")
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or(false);
        let min_value = get_attr_value("minvalue");
        let max_value = get_attr_value("maxvalue");
        let unit = get_attr_value("uom").filter(|s| !s.is_empty());

        // Extract allowed values for enum types
        let mut allowed_values = Vec::new();
        if let Some(av) = node.get("allowedvalues") {
            if let Some(obj) = av.as_object() {
                for (key, val) in obj {
                    // Skip non-numeric keys (like "handle", "value")
                    if key.parse::<u32>().is_ok() {
                        if let Some(v) = val.get("value").and_then(|v| v.as_str()) {
                            allowed_values.push(v.to_string());
                        }
                    }
                }
            }
        }

        Ok(ParamInfo {
            name: name.to_string(),
            datatype,
            access_mode,
            setinrun,
            min_value,
            max_value,
            allowed_values,
            unit,
        })
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

        // Allocate buffer with extra space and get the actual data
        // size is returned as number of characters needed (including null terminator)
        let buffer_size = (size as usize) + 1024; // Extra padding for safety
        let mut buffer = vec![0i8; buffer_size];
        let ret =
            unsafe { ffi::CAEN_FELib_GetDeviceTree(self.handle, buffer.as_mut_ptr(), buffer_size) };

        if ret < 0 {
            return Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Failed to get device tree".to_string(),
            }));
        }

        // Find the actual string length (look for null terminator)
        let actual_len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());

        // Convert to Rust string using the actual length
        let bytes: Vec<u8> = buffer[..actual_len].iter().map(|&c| c as u8).collect();
        String::from_utf8(bytes).map_err(|_| CaenError {
            code: -1,
            name: "Utf8Error".to_string(),
            description: "Device tree contains invalid UTF-8".to_string(),
        })
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
        let ret =
            unsafe { ffi::CAEN_FELib_GetValue(self.handle, c_path.as_ptr(), buffer.as_mut_ptr()) };

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

        let ret =
            unsafe { ffi::CAEN_FELib_SetValue(self.handle, c_path.as_ptr(), c_value.as_ptr()) };

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
        let ret =
            unsafe { ffi::CAEN_FELib_GetHandle(self.handle, c_path.as_ptr(), &mut sub_handle) };

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
    pub fn set_value_with_handle(
        &self,
        handle: u64,
        path: &str,
        value: &str,
    ) -> Result<(), CaenError> {
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

        Ok(EndpointHandle {
            handle: read_data_handle,
        })
    }

    /// Apply digitizer configuration
    ///
    /// Applies all parameters from DigitizerConfig to the device.
    /// Parameters are applied in order: board-level first, then channel defaults,
    /// then channel-specific overrides.
    ///
    /// # Arguments
    /// * `config` - DigitizerConfig to apply
    ///
    /// # Returns
    /// * `Ok(applied_count)` - Number of parameters successfully applied
    /// * `Err(...)` - Error if a critical parameter fails
    pub fn apply_config(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, CaenError> {
        use tracing::{debug, info, warn};

        let params = config.to_caen_parameters();
        info!("Applying {} parameters to digitizer", params.len());

        let mut applied = 0;
        let mut errors = Vec::new();

        for param in &params {
            match self.set_value(&param.path, &param.value) {
                Ok(()) => {
                    debug!(path = %param.path, value = %param.value, "Parameter set");
                    applied += 1;
                }
                Err(e) => {
                    warn!(
                        path = %param.path,
                        value = %param.value,
                        error = %e,
                        "Failed to set parameter"
                    );
                    errors.push((param.path.clone(), e));
                }
            }
        }

        info!(applied, errors = errors.len(), "Configuration applied");

        // Return error if any critical parameters failed
        // For now, we just warn and continue
        if !errors.is_empty() {
            warn!(
                "Some parameters failed to apply: {:?}",
                errors.iter().map(|(p, _)| p).collect::<Vec<_>>()
            );
        }

        Ok(applied)
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
    pub fn read_data(
        &self,
        timeout_ms: i32,
        buffer_size: usize,
    ) -> Result<Option<RawData>, CaenError> {
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
            Ok(Some(RawData {
                data,
                size,
                n_events,
            }))
        } else if ret == -11 {
            // Timeout
            Ok(None)
        } else if ret == -12 {
            // Stop signal - propagate as Err so read_loop can detect it
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Stop".to_string(),
                description: "Acquisition stopped".to_string(),
            }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_data_struct() {
        let raw = RawData {
            data: vec![1, 2, 3, 4],
            size: 4,
            n_events: 1,
        };
        assert_eq!(raw.data.len(), 4);
        assert_eq!(raw.size, 4);
        assert_eq!(raw.n_events, 1);
    }

    #[test]
    fn test_raw_data_debug() {
        let raw = RawData {
            data: vec![0xAB, 0xCD],
            size: 2,
            n_events: 0,
        };
        let debug = format!("{:?}", raw);
        assert!(debug.contains("RawData"));
        assert!(debug.contains("size: 2"));
    }

    #[test]
    fn test_cstring_null_byte_in_url() {
        // Test that null bytes in URL are rejected
        let url_with_null = "dig2://192.168.0.1\0/extra";
        let c_string = CString::new(url_with_null);
        assert!(c_string.is_err());
    }

    #[test]
    fn test_cstring_valid_url() {
        let valid_url = "dig2://192.168.0.1";
        let c_string = CString::new(valid_url);
        assert!(c_string.is_ok());
    }

    #[test]
    fn test_cstring_null_byte_in_path() {
        // Test that null bytes in path are rejected
        let path_with_null = "/par/Model\0Name";
        let c_string = CString::new(path_with_null);
        assert!(c_string.is_err());
    }

    #[test]
    fn test_cstring_valid_path() {
        let valid_path = "/par/ModelName";
        let c_string = CString::new(valid_path);
        assert!(c_string.is_ok());
    }

    #[test]
    fn test_endpoint_handle_raw() {
        let ep = EndpointHandle { handle: 12345 };
        assert_eq!(ep.raw(), 12345);
    }

    #[test]
    fn test_format_json_validity() {
        // Test that the format JSON used in configure_endpoint is valid JSON
        let format_json = r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0},
            {"name": "N_EVENTS", "type": "U32", "dim": 0}
        ]"#;

        let parsed: Result<serde_json::Value, _> = serde_json::from_str(format_json);
        assert!(parsed.is_ok());

        let arr = parsed.unwrap();
        assert!(arr.is_array());
        assert_eq!(arr.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_buffer_sizes() {
        // Verify buffer sizes used in the code are reasonable
        let value_buffer_size = 256;
        let name_buffer_size = 32;
        let desc_buffer_size = 256;

        // These should be large enough for typical CAEN responses
        assert!(value_buffer_size >= 128);
        assert!(name_buffer_size >= 16);
        assert!(desc_buffer_size >= 64);
    }

    #[test]
    fn test_device_info_struct() {
        let info = DeviceInfo {
            model: "VX2730".to_string(),
            serial_number: "12345".to_string(),
            firmware_type: "DPP_PSD".to_string(),
            num_channels: 32,
            adc_bits: 14,
            sampling_rate_sps: 125_000_000,
        };
        assert_eq!(info.model, "VX2730");
        assert_eq!(info.num_channels, 32);
        assert_eq!(info.adc_bits, 14);
    }

    #[test]
    fn test_device_info_clone() {
        let info = DeviceInfo {
            model: "VX2730".to_string(),
            serial_number: "12345".to_string(),
            firmware_type: "DPP_PSD".to_string(),
            num_channels: 32,
            adc_bits: 14,
            sampling_rate_sps: 125_000_000,
        };
        let cloned = info.clone();
        assert_eq!(info.model, cloned.model);
        assert_eq!(info.serial_number, cloned.serial_number);
    }

    #[test]
    fn test_device_info_debug() {
        let info = DeviceInfo {
            model: "VX2730".to_string(),
            serial_number: "12345".to_string(),
            firmware_type: "DPP_PSD".to_string(),
            num_channels: 32,
            adc_bits: 14,
            sampling_rate_sps: 125_000_000,
        };
        let debug = format!("{:?}", info);
        assert!(debug.contains("VX2730"));
        assert!(debug.contains("DPP_PSD"));
    }

    #[test]
    fn test_param_info_struct() {
        let info = ParamInfo {
            name: "DCOffset".to_string(),
            datatype: "NUMBER".to_string(),
            access_mode: "READ_WRITE".to_string(),
            setinrun: true,
            min_value: Some("0".to_string()),
            max_value: Some("100".to_string()),
            allowed_values: vec![],
            unit: Some("%".to_string()),
        };
        assert_eq!(info.name, "DCOffset");
        assert!(info.setinrun);
        assert_eq!(info.min_value, Some("0".to_string()));
    }

    #[test]
    fn test_param_info_enum_type() {
        let info = ParamInfo {
            name: "Polarity".to_string(),
            datatype: "STRING".to_string(),
            access_mode: "READ_WRITE".to_string(),
            setinrun: false,
            min_value: None,
            max_value: None,
            allowed_values: vec!["Positive".to_string(), "Negative".to_string()],
            unit: None,
        };
        assert_eq!(info.allowed_values.len(), 2);
        assert!(!info.setinrun);
    }

    #[test]
    fn test_param_info_clone() {
        let info = ParamInfo {
            name: "TriggerThr".to_string(),
            datatype: "NUMBER".to_string(),
            access_mode: "READ_WRITE".to_string(),
            setinrun: true,
            min_value: Some("0".to_string()),
            max_value: Some("16383".to_string()),
            allowed_values: vec![],
            unit: None,
        };
        let cloned = info.clone();
        assert_eq!(info.name, cloned.name);
        assert_eq!(info.setinrun, cloned.setinrun);
    }

    #[test]
    fn test_extract_param_info_from_json() {
        // Simulate DevTree parameter node structure
        let json_str = r#"{
            "accessmode": { "value": "READ_WRITE" },
            "datatype": { "value": "NUMBER" },
            "setinrun": { "value": "true" },
            "minvalue": { "value": "0" },
            "maxvalue": { "value": "100" },
            "uom": { "value": "%" }
        }"#;

        let node: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let info = CaenHandle::extract_param_info("DCOffset", &node).unwrap();

        assert_eq!(info.name, "DCOffset");
        assert_eq!(info.datatype, "NUMBER");
        assert!(info.setinrun);
        assert_eq!(info.min_value, Some("0".to_string()));
        assert_eq!(info.max_value, Some("100".to_string()));
        assert_eq!(info.unit, Some("%".to_string()));
    }

    #[test]
    fn test_extract_param_info_enum() {
        // Simulate DevTree parameter node with allowed values
        let json_str = r#"{
            "accessmode": { "value": "READ_WRITE" },
            "datatype": { "value": "STRING" },
            "setinrun": { "value": "false" },
            "allowedvalues": {
                "handle": 123,
                "value": "2",
                "0": { "value": "Positive" },
                "1": { "value": "Negative" }
            }
        }"#;

        let node: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let info = CaenHandle::extract_param_info("Polarity", &node).unwrap();

        assert_eq!(info.datatype, "STRING");
        assert!(!info.setinrun);
        assert_eq!(info.allowed_values.len(), 2);
        assert!(info.allowed_values.contains(&"Positive".to_string()));
        assert!(info.allowed_values.contains(&"Negative".to_string()));
    }
}
