//! Safe wrapper for CAEN device handle
//!
//! Provides RAII-based handle management (like C++ std::unique_ptr with custom deleter).

use super::error::CaenError;
use super::ffi;
use super::validation::{self, ApplyConfigResult, ParamApplyResult, ParamApplyStatus};
use crate::config::devtree_paths as devtree;
use std::collections::HashMap;
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

    /// C wrapper for CAEN_FELib_ReadData with OpenDPP format (no waveform)
    /// Defined in wrapper.c, compiled by build.rs
    fn caen_read_data_opendpp(
        handle: u64,
        timeout: std::os::raw::c_int,
        channel: *mut u8,
        timestamp: *mut u64,
        fine_timestamp: *mut u16,
        energy: *mut u16,
        flags_b: *mut u16,
        flags_a: *mut u16,
        psd: *mut u16,
        user_info: *mut u64,
        user_info_size: *mut usize,
        event_size: *mut usize,
    ) -> std::os::raw::c_int;

    /// C wrapper for CAEN_FELib_ReadData with OpenDPP format (with waveform)
    /// Defined in wrapper.c, compiled by build.rs
    fn caen_read_data_opendpp_waveform(
        handle: u64,
        timeout: std::os::raw::c_int,
        channel: *mut u8,
        timestamp: *mut u64,
        fine_timestamp: *mut u16,
        energy: *mut u16,
        flags_b: *mut u16,
        flags_a: *mut u16,
        psd: *mut u16,
        user_info: *mut u64,
        user_info_size: *mut usize,
        waveform: *mut u16,
        waveform_size: *mut usize,
        event_size: *mut usize,
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

/// OpenDPP decoded event data (single event)
#[derive(Debug, Clone)]
pub struct OpenDppEvent {
    /// Channel number
    pub channel: u8,
    /// Timestamp (in clock ticks, 1 LSB = 8 ns for VX2730)
    pub timestamp: u64,
    /// Fine timestamp (sub-clock resolution)
    pub fine_timestamp: u16,
    /// Energy value
    pub energy: u16,
    /// Flags B (12 bits)
    pub flags_b: u16,
    /// Flags A (8 bits)
    pub flags_a: u16,
    /// PSD value
    pub psd: u16,
    /// User info words
    pub user_info: Vec<u64>,
    /// Waveform samples (optional, only if configured with include_waveform=true)
    pub waveform: Option<Vec<u16>>,
    /// Total event size in bytes
    pub event_size: usize,
}

/// Device information retrieved from digitizer
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Step increment (e.g., "8", "2", "0.1")
    pub increment: Option<String>,
    /// Default value from DevTree
    pub default_value: Option<String>,
    /// Unit exponent (e.g., -9 for nanoseconds, 0 for base unit)
    pub expuom: Option<i32>,
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
        let increment = get_attr_value("increment");
        let default_value = get_attr_value("defaultvalue");
        let expuom = get_attr_value("expuom").and_then(|s| s.parse::<i32>().ok());

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
            increment,
            default_value,
            expuom,
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

    /// Read a user register value
    ///
    /// # Arguments
    /// * `address` - Register address (e.g., 0xEF24)
    ///
    /// # Note
    /// This is a low-level operation. Use with caution.
    pub fn get_user_register(&self, address: u32) -> Result<u32, CaenError> {
        let mut value: u32 = 0;
        let ret = unsafe { ffi::CAEN_FELib_GetUserRegister(self.handle, address, &mut value) };
        CaenError::check(ret)?;
        Ok(value)
    }

    /// Write a user register value
    ///
    /// # Arguments
    /// * `address` - Register address (e.g., 0xEF24 for software reset)
    /// * `value` - Value to write
    ///
    /// # Note
    /// This is a low-level operation. Use with caution.
    /// Desktop digitizers only: writing any value to 0xEF24 triggers a software reset.
    pub fn set_user_register(&self, address: u32, value: u32) -> Result<(), CaenError> {
        let ret = unsafe { ffi::CAEN_FELib_SetUserRegister(self.handle, address, value) };
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
    /// Follows the C++ pattern from Digitizer1/Digitizer2::EndpointConfigure()
    ///
    /// # Arguments
    /// * `include_n_events` - If true, include N_EVENTS in the read format (DIG2).
    ///   If false, use DATA + SIZE only (DIG1).
    pub fn configure_endpoint(&self, include_n_events: bool) -> Result<EndpointHandle, CaenError> {
        // Get endpoint handle
        let ep_handle = self.get_handle("/endpoint/RAW")?;

        // Get parent (endpoint folder) handle
        let ep_folder_handle = self.get_parent_handle(ep_handle)?;

        // Set active endpoint to RAW
        self.set_value_with_handle(ep_folder_handle, devtree::par::ACTIVE_ENDPOINT, "RAW")?;

        // Get fresh handle for read operations
        let read_data_handle = self.get_handle("/endpoint/RAW")?;

        // Set data format based on digitizer generation
        let format_json = if include_n_events {
            // DIG2 (VX2730 etc.): DATA, SIZE, N_EVENTS
            r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0},
            {"name": "N_EVENTS", "type": "U32", "dim": 0}
        ]"#
        } else {
            // DIG1 (DT5730 etc.): DATA, SIZE only
            r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0}
        ]"#
        };

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

    /// Configure endpoint for OpenDPP decoded data reading
    ///
    /// This sets up the OpenDPP endpoint and returns an EndpointHandle for data reading.
    /// The OpenDPP endpoint provides decoded event data (channel, timestamp, energy, etc.)
    /// instead of raw binary data.
    ///
    /// # Arguments
    /// * `include_waveform` - If true, include waveform data in the output
    pub fn configure_opendpp_endpoint(
        &self,
        include_waveform: bool,
    ) -> Result<EndpointHandle, CaenError> {
        // Get endpoint handle
        let ep_handle = self.get_handle("/endpoint/opendpp")?;

        // Get parent (endpoint folder) handle
        let ep_folder_handle = self.get_parent_handle(ep_handle)?;

        // Set active endpoint to OpenDPP
        self.set_value_with_handle(ep_folder_handle, devtree::par::ACTIVE_ENDPOINT, "OpenDPP")?;

        // Get fresh handle for read operations
        let read_data_handle = self.get_handle("/endpoint/opendpp")?;

        // Set data format for OpenDPP
        let format_json = if include_waveform {
            r#"[
            {"name": "CHANNEL", "type": "U8"},
            {"name": "TIMESTAMP", "type": "U64"},
            {"name": "FINE_TIMESTAMP", "type": "U16"},
            {"name": "ENERGY", "type": "U16"},
            {"name": "FLAGS_B", "type": "U16"},
            {"name": "FLAGS_A", "type": "U16"},
            {"name": "PSD", "type": "U16"},
            {"name": "USER_INFO", "type": "U64", "dim": 1},
            {"name": "USER_INFO_SIZE", "type": "SIZE_T"},
            {"name": "WAVEFORM", "type": "U16", "dim": 1},
            {"name": "WAVEFORM_SIZE", "type": "SIZE_T"},
            {"name": "EVENT_SIZE", "type": "SIZE_T"}
        ]"#
        } else {
            r#"[
            {"name": "CHANNEL", "type": "U8"},
            {"name": "TIMESTAMP", "type": "U64"},
            {"name": "FINE_TIMESTAMP", "type": "U16"},
            {"name": "ENERGY", "type": "U16"},
            {"name": "FLAGS_B", "type": "U16"},
            {"name": "FLAGS_A", "type": "U16"},
            {"name": "PSD", "type": "U16"},
            {"name": "USER_INFO", "type": "U64", "dim": 1},
            {"name": "USER_INFO_SIZE", "type": "SIZE_T"},
            {"name": "EVENT_SIZE", "type": "SIZE_T"}
        ]"#
        };

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

    /// Configure the **RawUDP** endpoint for bulk raw readout (AMax / DPP_OPEN
    /// 10G firmware).
    ///
    /// Unlike the generic `/endpoint/RAW` used by the scope firmwares
    /// (PSD2/PHA2), the open-DPP 10G firmware exposes its raw event-aggregate
    /// stream under `/endpoint/rawudp` with active-endpoint value `"RawUDP"`
    /// (the only allowed values are `RawUDP` and `OpenDPP`; there is no `RAW`).
    /// Verified from the live DevTree on 192.168.14.10 (2026-06-11) — using
    /// the generic RAW path returns CAEN error -6 (COMMAND ERROR).
    ///
    /// The returned bytes are the same FW wire words the OpenDPP endpoint
    /// decodes per-event; here we read whole buffers and let `AMaxDecoder`
    /// parse them on the decode workers (big-endian word0/word1, user words
    /// and waveform). The FW always frames waveforms on the wire, so there is
    /// no waveform toggle at this layer.
    pub fn configure_rawudp_endpoint(&self) -> Result<EndpointHandle, CaenError> {
        // Get endpoint handle
        let ep_handle = self.get_handle("/endpoint/rawudp").map_err(|e| {
            eprintln!("[rawudp] step1 get_handle(/endpoint/rawudp) failed: {}", e);
            e
        })?;

        // Get parent (endpoint folder) handle
        let ep_folder_handle = self.get_parent_handle(ep_handle).map_err(|e| {
            eprintln!("[rawudp] step2 get_parent_handle failed: {}", e);
            e
        })?;

        // Set active endpoint to RawUDP
        self.set_value_with_handle(ep_folder_handle, devtree::par::ACTIVE_ENDPOINT, "RawUDP")
            .map_err(|e| {
                eprintln!("[rawudp] step3 set activeendpoint=RawUDP failed: {}", e);
                e
            })?;

        // Get fresh handle for read operations
        let read_data_handle = self.get_handle("/endpoint/rawudp").map_err(|e| {
            eprintln!("[rawudp] step4 get_handle(read) failed: {}", e);
            e
        })?;

        // Raw event-aggregate format. The generic FELib doc lists DATA/SIZE/
        // N_EVENTS for the Raw endpoint, but the deployed 10G FW's `rawudp`
        // rejects N_EVENTS with -2 (INVALID PARAMETER). DATA+SIZE alone is
        // sufficient: `caen_read_data_raw` passes a (now unused) n_events
        // pointer that CAEN_FELib_ReadData simply doesn't fill when the format
        // omits it (same pattern as the DIG1 `include_n_events=false` path),
        // and the AMax decoder walks the buffer word-by-word so it doesn't
        // need the event count.
        let candidates: [&str; 2] = [
            r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0},
            {"name": "N_EVENTS", "type": "U32", "dim": 0}
        ]"#,
            r#"[
            {"name": "DATA", "type": "U8", "dim": 1},
            {"name": "SIZE", "type": "SIZE_T", "dim": 0}
        ]"#,
        ];

        let mut last_ret = -2;
        for (i, format_json) in candidates.iter().enumerate() {
            let c_format = CString::new(*format_json).map_err(|_| CaenError {
                code: -2,
                name: "InvalidParam".to_string(),
                description: "Format JSON contains null byte".to_string(),
            })?;
            let ret =
                unsafe { ffi::CAEN_FELib_SetReadDataFormat(read_data_handle, c_format.as_ptr()) };
            if ret == 0 {
                tracing::info!(variant = i, "RawUDP read data format set");
                return Ok(EndpointHandle {
                    handle: read_data_handle,
                });
            }
            eprintln!(
                "[rawudp] SetReadDataFormat variant {} failed: ret={}",
                i, ret
            );
            last_ret = ret;
        }
        CaenError::check(last_ret)?;

        Ok(EndpointHandle {
            handle: read_data_handle,
        })
    }

    /// Validate that config num_channels does not exceed hardware channel count.
    fn validate_num_channels(&self, config_num_ch: u8) -> Result<(), CaenError> {
        let hw_num_ch: u32 = self
            .get_value("/par/NumCh")
            .unwrap_or_default()
            .parse()
            .unwrap_or(0);
        let config_num_ch = config_num_ch as u32;

        if hw_num_ch > 0 && config_num_ch > hw_num_ch {
            return Err(CaenError {
                code: -1,
                name: "NumChannelsMismatch".to_string(),
                description: format!(
                    "Config num_channels={} exceeds hardware NumCh={}. \
                     Fix the JSON config or run Detect to auto-correct.",
                    config_num_ch, hw_num_ch
                ),
            });
        }
        Ok(())
    }

    /// Apply digitizer configuration.
    ///
    /// Applies all parameters from DigitizerConfig to the device.
    /// Parameters are applied in order: board-level first, then channel defaults,
    /// then channel-specific overrides.
    pub fn apply_config(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, CaenError> {
        use tracing::{debug, info, warn};

        // Validate num_channels against hardware
        self.validate_num_channels(config.num_channels)?;

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
        if !errors.is_empty() {
            warn!(
                "Some parameters failed to apply: {:?}",
                errors.iter().map(|(p, _)| p).collect::<Vec<_>>()
            );
        }

        // Defense in depth: detect if ALL channel parameters failed
        let ch_params: Vec<_> = params.iter().filter(|p| p.path.contains("/ch/")).collect();
        let ch_errors = errors.iter().filter(|(p, _)| p.contains("/ch/")).count();
        if !ch_params.is_empty() && ch_errors == ch_params.len() {
            return Err(CaenError {
                code: -1,
                name: "AllChannelParamsFailed".to_string(),
                description: format!(
                    "All {} channel parameters failed. \
                     Likely num_channels mismatch. Run Detect to update.",
                    ch_errors
                ),
            });
        }

        // Force ch_extras_opt for DIG1 firmware (PSD1/PHA1).
        // The decoder depends on the specific EXTRAS word bit layout.
        // In SW Fine TS mode, use SAZC/SBZC option (0b101) instead of HW Fine TS (0b010).
        if config.firmware.is_dig1() {
            let fine_ts_mode = config
                .board
                .fine_ts_mode
                .unwrap_or(crate::config::digitizer::FineTsMode::Hardware);

            let extras_value = match (config.firmware, fine_ts_mode) {
                (
                    crate::config::digitizer::FirmwareType::PSD1,
                    crate::config::digitizer::FineTsMode::Hardware,
                ) => "EXTRAS_OPT_TT48_FLAGS_FINETT",
                (
                    crate::config::digitizer::FirmwareType::PSD1,
                    crate::config::digitizer::FineTsMode::Software,
                ) => "EXTRAS_OPT_SBZC_SAZC",
                (
                    crate::config::digitizer::FirmwareType::PHA1,
                    crate::config::digitizer::FineTsMode::Hardware,
                ) => "EXTRAS_OPT_TT48_FINETT",
                (
                    crate::config::digitizer::FirmwareType::PHA1,
                    crate::config::digitizer::FineTsMode::Software,
                ) => "EXTRAS_OPT_EBZC_EAZC",
                _ => unreachable!(),
            };
            for ch in 0..config.num_channels {
                let path = format!("/ch/{}/par/ch_extras_opt", ch);
                match self.set_value(&path, extras_value) {
                    Ok(()) => {
                        debug!(path = %path, value = extras_value, "Forced ch_extras_opt");
                    }
                    Err(e) => {
                        warn!(path = %path, error = %e, "Failed to force ch_extras_opt");
                    }
                }
            }
            info!(
                firmware = ?config.firmware,
                fine_ts_mode = ?fine_ts_mode,
                value = extras_value,
                channels = config.num_channels,
                "Forced ch_extras_opt"
            );

            // Apply CFD interpolation point via direct register write (no FELib param exists)
            if let Some(interp_pt) = config.channel_defaults.cfd_interpolation_point {
                let interp_pt = interp_pt.min(3) as u32; // clamp to 0-3
                for ch in 0..config.num_channels {
                    let addr = 0x103C + (ch as u32) * 0x0100;
                    match self.get_user_register(addr) {
                        Ok(current) => {
                            let new_val = (current & !0x0C00) | (interp_pt << 10);
                            if let Err(e) = self.set_user_register(addr, new_val) {
                                warn!(ch = ch, error = %e, "Failed to set CFD interpolation point");
                            } else {
                                debug!(
                                    ch = ch,
                                    interp_pt = interp_pt,
                                    "Set CFD interpolation point"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(ch = ch, error = %e, "Failed to read CFD settings register");
                        }
                    }
                }
            }
        }

        Ok(applied)
    }

    /// Apply AMax-firmware per-channel registers via direct user-register writes.
    ///
    /// Walks `config.channel_overrides` (falling back to `channel_defaults`) and
    /// writes each `AMaxChannelConfig` field that is `Some(_)` to the FELib via
    /// `set_user_register`. Channel addresses are computed by
    /// [`amax_registers::channel_register_byte_addr`].
    ///
    /// Returns the count of successful register writes (the operator surfaces
    /// this as "Applied N parameters to hardware").
    ///
    /// Errors: on the first failed write the function bails out; partial state
    /// stays on the hardware (callers should treat AMax apply failures as a
    /// hard error and investigate).
    pub fn apply_amax_channel_config(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, CaenError> {
        use super::amax_registers as r;
        use crate::config::digitizer::AMaxChannelConfig;
        use tracing::{debug, info, warn};

        let mut count: usize = 0;
        let defaults_amax: Option<&AMaxChannelConfig> = config.channel_defaults.amax.as_ref();

        // Clamp the sweep to the channel pages the FW actually implements.
        // Every other part of this address map (PAGE_BASE, PAGE_STRIDE,
        // BROADCAST_BASE, the offsets) is auto-derived by `amax_codegen` and
        // therefore tracks each FW rebuild; `num_channels` is hand-written in
        // the digitizer JSON and does not. A 32-channel config left in place
        // against the 2-channel 12august FW would send 30 channels' worth of
        // writes to addresses that FW never defines, and writing outside the
        // FW's own map is how AMax firmware gets corrupted.
        let n_ch = r::amax_channel_span(config.num_channels, r::CHANNEL_PAGES);
        if n_ch < config.num_channels {
            warn!(
                config_num_channels = config.num_channels,
                fw_channel_pages = r::CHANNEL_PAGES,
                first_skipped = n_ch,
                "[AMax] config num_channels exceeds the channel pages this firmware \
                 defines - clamping the register sweep; the remaining channels are \
                 left unconfigured. Fix num_channels in the digitizer JSON."
            );
        }

        // Resolve override-or-default for every Optional field. Codegen
        // (`channel_writes`) drives the actual write list, so adding a new
        // FW register is one `cargo run --bin amax_codegen` away.
        for ch in 0..n_ch {
            let override_amax = config
                .channel_overrides
                .get(&ch)
                .and_then(|c| c.amax.as_ref());
            // Override-over-default merge is codegen-generated (see
            // `merge_amax_channel_config` in amax_registers_generated.rs) so the
            // field set lives in ONE place. Adding/removing an AMax register is
            // a `cargo run --bin amax_codegen` away — no edit here.
            let merged = r::merge_amax_channel_config(override_amax, defaults_amax);

            for (offset, value, name) in r::channel_writes(&merged) {
                let byte_addr = r::channel_register_byte_addr(ch, offset);
                debug!(
                    channel = ch,
                    register = name,
                    addr = format_args!("0x{:08X}", byte_addr),
                    value,
                    "[AMax] WriteUserRegister"
                );
                self.set_user_register(byte_addr, value)?;
                count += 1;
            }

            // 13may FW broadcast-page mirror write: ch0 of the per-channel
            // `_4_<N>` pages observed not to drive a visible physical
            // channel on the VX2730 caenlist firmware (31/32 channels fire,
            // ch0 stays silent). The same defaults also live on the
            // `page_amax_energy/<NAME>` broadcast page; writing them here
            // ensures the missing channel picks up the same
            // threshold/offset/polarity as the rest. Cheap (24 extra writes
            // per Configure, run once outside the channel loop).
            //
            // The broadcast-page base shifts on every FW rebuild (16June 0x8200
            // → 17June 0x200, etc.), so it is no longer hardcoded here: it is
            // auto-derived by `amax_codegen` from RegisterFile.json and emitted
            // as `amax_registers::BROADCAST_BASE`. `broadcast_register_byte_addr`
            // applies the same in-page offsets used for the per-channel writes
            // (the two pages share an identical layout — codegen asserts this).
            if ch == 0 {
                for (offset, value, name) in r::channel_writes(&merged) {
                    let byte_addr = r::broadcast_register_byte_addr(offset);
                    debug!(
                        register = name,
                        addr = format_args!("0x{:08X}", byte_addr),
                        value,
                        "[AMax] WriteUserRegister (broadcast page)"
                    );
                    self.set_user_register(byte_addr, value)?;
                    count += 1;
                }
            }
        }

        // Board-level (global) writes — applied once after the per-channel
        // sweep. ENABLE_ACQ toggles the debug FW's MUX selector + SE flag,
        // so it must be set after channel pages are stable so the first
        // SE event the digitizer emits has consistent shaping params.
        if let Some(board_cfg) = config.amax_board.as_ref() {
            for (byte_addr, value, name) in r::board_writes(board_cfg) {
                debug!(
                    register = name,
                    addr = format_args!("0x{:08X}", byte_addr),
                    value,
                    "[AMax] WriteUserRegister (board)"
                );
                self.set_user_register(byte_addr, value)?;
                count += 1;
            }
        }

        info!(applied = count, "AMax channel config applied");
        Ok(count)
    }

    /// Apply only SetInRun parameters (safe to call while Running)
    ///
    /// Filters parameters to only those the hardware supports changing
    /// during data acquisition. Non-SetInRun parameters are silently skipped.
    pub fn apply_config_running(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, CaenError> {
        use tracing::{debug, info, warn};

        let params = config.to_caen_parameters_set_in_run();
        info!(
            "Applying {} SetInRun parameters to digitizer (running)",
            params.len()
        );

        let mut applied = 0;
        let mut errors = Vec::new();

        for param in &params {
            match self.set_value(&param.path, &param.value) {
                Ok(()) => {
                    debug!(path = %param.path, value = %param.value, "SetInRun parameter set");
                    applied += 1;
                }
                Err(e) => {
                    warn!(
                        path = %param.path,
                        value = %param.value,
                        error = %e,
                        "Failed to set SetInRun parameter"
                    );
                    errors.push((param.path.clone(), e));
                }
            }
        }

        info!(
            applied,
            errors = errors.len(),
            "SetInRun configuration applied"
        );

        if !errors.is_empty() {
            warn!(
                "Some SetInRun parameters failed: {:?}",
                errors.iter().map(|(p, _)| p).collect::<Vec<_>>()
            );
        }

        Ok(applied)
    }

    /// Build a parameter cache from the DevTree.
    ///
    /// Fetches the device tree once and parses all parameter metadata
    /// (min, max, increment, allowed_values, etc.) into a HashMap keyed by
    /// parameter name. Both board-level (`/par/`) and channel-level
    /// (`/ch/0/par/`) parameters are collected.
    ///
    /// # Returns
    /// * `Ok(cache)` - HashMap mapping parameter name → ParamInfo
    /// * `Err(...)` - If DevTree fetch or JSON parse fails
    pub fn build_param_cache(&self) -> Result<HashMap<String, ParamInfo>, CaenError> {
        use tracing::{debug, info, warn};

        let tree_json = self.get_device_tree()?;
        let tree: serde_json::Value = serde_json::from_str(&tree_json).map_err(|e| CaenError {
            code: -1,
            name: "JsonParseError".to_string(),
            description: format!("Failed to parse DevTree JSON: {}", e),
        })?;

        let mut cache = HashMap::new();

        // Collect board-level parameters from /par/
        if let Some(par) = tree.get("par").and_then(|v| v.as_object()) {
            for (name, node) in par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    // Skip non-parameter nodes (those without datatype)
                    if obj.contains_key("datatype") {
                        match Self::extract_param_info(name, node) {
                            Ok(info) => {
                                debug!(param = %name, "Cached board param");
                                // Insert under the lowercased name (DevTree
                                // already uses lowercase; this just makes the
                                // contract explicit so a future DevTree dump
                                // with mixed case still maps cleanly).
                                cache.insert(name.to_lowercase(), info);
                            }
                            Err(e) => {
                                warn!(param = %name, error = %e, "Failed to parse board param");
                            }
                        }
                    }
                }
            }
        }

        // Collect channel-level parameters from /ch/0/par/
        // (metadata is identical across channels — just sample ch0)
        if let Some(ch0_par) = tree
            .get("ch")
            .and_then(|ch| ch.get("0"))
            .and_then(|ch0| ch0.get("par"))
            .and_then(|v| v.as_object())
        {
            for (name, node) in ch0_par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    let key_lower = name.to_lowercase();
                    if obj.contains_key("datatype") && !cache.contains_key(&key_lower) {
                        match Self::extract_param_info(name, node) {
                            Ok(info) => {
                                debug!(param = %name, "Cached channel param");
                                cache.insert(key_lower, info);
                            }
                            Err(e) => {
                                warn!(param = %name, error = %e, "Failed to parse channel param");
                            }
                        }
                    }
                }
            }
        }

        info!(total = cache.len(), "Parameter cache built from DevTree");
        Ok(cache)
    }

    /// Apply digitizer configuration with validation.
    ///
    /// Each parameter is validated against DevTree metadata before applying:
    /// - Numeric values are snapped to the nearest valid step
    /// - Values are clamped to [min, max]
    /// - Unknown parameters (not in DevTree) are applied without validation
    ///
    /// This replaces `apply_config()` when a param cache is available.
    pub fn apply_config_validated(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
        param_cache: &HashMap<String, ParamInfo>,
    ) -> Result<ApplyConfigResult, CaenError> {
        // Validate num_channels against hardware
        self.validate_num_channels(config.num_channels)?;

        let params = config.to_caen_parameters();
        self.apply_params_validated(&params, param_cache)
    }

    /// Apply only SetInRun parameters with validation (safe while Running).
    pub fn apply_config_running_validated(
        &self,
        config: &crate::config::digitizer::DigitizerConfig,
        param_cache: &HashMap<String, ParamInfo>,
    ) -> Result<ApplyConfigResult, CaenError> {
        let params = config.to_caen_parameters_set_in_run();
        self.apply_params_validated(&params, param_cache)
    }

    /// Internal: validate and apply a list of parameters.
    ///
    /// After every successful `set_value`, this also reads the value back
    /// via `get_value` ("loopback verification") and logs at INFO level if
    /// the firmware rounded or otherwise modified what we sent — typical
    /// for ns-denominated trapezoid params that the FW snaps to the
    /// nearest sample. Loopback is skipped for range-broadcast paths
    /// (`/ch/0..N/par/foo`) since they aren't readable in that form.
    fn apply_params_validated(
        &self,
        params: &[crate::config::digitizer::CaenParameter],
        param_cache: &HashMap<String, ParamInfo>,
    ) -> Result<ApplyConfigResult, CaenError> {
        use tracing::{debug, info, warn};

        info!("Applying {} parameters with validation", params.len());

        let mut result = ApplyConfigResult {
            total: params.len(),
            ..Default::default()
        };
        let mut loopback_mismatches: Vec<String> = Vec::new();

        for param in params {
            // Extract parameter name from path (last segment after '/').
            //
            // FELib accepts paths case-insensitively, so `to_caen_parameters`
            // emits CamelCase (`ChRecordLengthT`) while the DevTree — and
            // therefore `param_cache` — uses lowercase (`chrecordlengtht`).
            // Look the cache up case-insensitively so out-of-range values
            // get clamped by validation instead of silently rejected by the
            // firmware. (Pre-2026-05-04, every CamelCase write bypassed
            // validation; e.g. record_length_ns=17000 → CAEN error -6 on
            // /ch/0..31/par/ChRecordLengthT, with the channel left at the
            // previous value and no UI feedback.)
            let param_name_raw = param.path.rsplit('/').next().unwrap_or("");
            let param_name_lower = param_name_raw.to_lowercase();
            let cache_hit = param_cache
                .get(param_name_raw)
                .or_else(|| param_cache.get(param_name_lower.as_str()));

            match cache_hit {
                Some(info) => {
                    // Validate against DevTree metadata
                    let validated = validation::validate_param(&param.value, info);

                    if validated.adjusted {
                        info!(
                            path = %param.path,
                            original = %param.value,
                            adjusted = %validated.value,
                            message = ?validated.message,
                            "Parameter value adjusted"
                        );
                    }

                    match self.set_value(&param.path, &validated.value) {
                        Ok(()) => {
                            let status = if validated.adjusted {
                                result.adjusted += 1;
                                ParamApplyStatus::Adjusted
                            } else {
                                result.ok += 1;
                                ParamApplyStatus::Ok
                            };
                            debug!(path = %param.path, value = %validated.value, "Parameter set");

                            // Loopback verification (Gemini #5): read back to
                            // capture FW rounding so unexpected FWHM regressions
                            // can be traced to specific param adjustments.
                            let applied_value = self
                                .verify_loopback(&param.path, &validated.value)
                                .unwrap_or_else(|| validated.value.clone());
                            if applied_value != validated.value {
                                info!(
                                    path = %param.path,
                                    sent = %validated.value,
                                    applied = %applied_value,
                                    "Parameter FW-rounded (loopback mismatch)",
                                );
                                loopback_mismatches.push(format!(
                                    "{}: {} → {}",
                                    param.path, validated.value, applied_value
                                ));
                            }

                            result.details.push(ParamApplyResult {
                                path: param.path.clone(),
                                original_value: param.value.clone(),
                                applied_value,
                                status,
                                message: validated.message,
                            });
                        }
                        Err(e) => {
                            warn!(
                                path = %param.path,
                                value = %validated.value,
                                error = %e,
                                "Failed to set parameter"
                            );
                            result.failed += 1;
                            result.details.push(ParamApplyResult {
                                path: param.path.clone(),
                                original_value: param.value.clone(),
                                applied_value: validated.value,
                                status: ParamApplyStatus::Failed,
                                message: Some(format!("{}", e)),
                            });
                        }
                    }
                }
                None => {
                    // Parameter not in cache — apply without DevTree
                    // validation. Legitimate cases: range-expanded paths
                    // like `/ch/0..31/par/foo` (cache holds the broadcast
                    // base, not the expansion), or hand-crafted paths
                    // like `dt_ext_clock` for VME side-channels.
                    //
                    // 2026-05-04 post-mortem: this branch was a `debug!`
                    // log + `result.ok += 1`, so a CamelCase / lowercase
                    // cache-key bug routed every channel-broadcast write
                    // here for months — silent clamp bypass. We now log
                    // at `info!` level + count separately as `no_cache`
                    // so a regression is visible in the apply summary.
                    match self.set_value(&param.path, &param.value) {
                        Ok(()) => {
                            info!(
                                path = %param.path,
                                value = %param.value,
                                "Parameter applied without DevTree validation (no cache hit)"
                            );
                            result.no_cache += 1;
                            result.details.push(ParamApplyResult {
                                path: param.path.clone(),
                                original_value: param.value.clone(),
                                applied_value: param.value.clone(),
                                status: ParamApplyStatus::NoCache,
                                message: Some("no cache entry — passed unvalidated to FW".into()),
                            });
                        }
                        Err(e) => {
                            warn!(
                                path = %param.path,
                                value = %param.value,
                                error = %e,
                                "Parameter not in DevTree and set_value failed"
                            );
                            result.skipped += 1;
                            result.details.push(ParamApplyResult {
                                path: param.path.clone(),
                                original_value: param.value.clone(),
                                applied_value: param.value.clone(),
                                status: ParamApplyStatus::Skipped,
                                message: Some(format!("Not in DevTree, set_value failed: {}", e)),
                            });
                        }
                    }
                }
            }
        }

        info!(
            total = result.total,
            ok = result.ok,
            adjusted = result.adjusted,
            failed = result.failed,
            skipped = result.skipped,
            no_cache = result.no_cache,
            loopback_mismatches = loopback_mismatches.len(),
            "Validated configuration applied"
        );

        if result.adjusted > 0 {
            let adjusted_params: Vec<_> = result
                .details
                .iter()
                .filter(|d| d.status == ParamApplyStatus::Adjusted)
                .map(|d| format!("{}: {} → {}", d.path, d.original_value, d.applied_value))
                .collect();
            info!("Adjusted parameters: {:?}", adjusted_params);
        }

        if result.no_cache > 0 {
            let no_cache_params: Vec<_> = result
                .details
                .iter()
                .filter(|d| d.status == ParamApplyStatus::NoCache)
                .map(|d| format!("{}={}", d.path, d.applied_value))
                .collect();
            info!(
                count = result.no_cache,
                "Parameters applied without DevTree validation (no cache hit): {:?}",
                no_cache_params
            );
        }

        if !loopback_mismatches.is_empty() {
            info!(
                "FW-rounded parameters (loopback mismatch): {:?}",
                loopback_mismatches
            );
        }

        // Defense in depth: detect if ALL channel parameters failed
        let ch_total = result
            .details
            .iter()
            .filter(|d| d.path.contains("/ch/"))
            .count();
        let ch_failed = result
            .details
            .iter()
            .filter(|d| {
                d.path.contains("/ch/")
                    && matches!(
                        d.status,
                        ParamApplyStatus::Failed | ParamApplyStatus::Skipped
                    )
            })
            .count();
        if ch_total > 0 && ch_failed == ch_total {
            return Err(CaenError {
                code: -1,
                name: "AllChannelParamsFailed".to_string(),
                description: format!(
                    "All {} channel parameters failed. \
                     Likely num_channels mismatch. Run Detect to update.",
                    ch_failed
                ),
            });
        }

        Ok(result)
    }

    /// Verify a `set_value` succeeded by reading the value back. Returns
    /// `Some(actual)` when the FW reports a different value than `expected`
    /// (typical for ns→sample rounding on trapezoid params), `None` when
    /// the values are equivalent or readback isn't possible (range paths,
    /// write-only params).
    fn verify_loopback(&self, path: &str, expected: &str) -> Option<String> {
        // Range-broadcast paths like /ch/0..7/par/foo aren't readable.
        if path.contains("..") {
            return None;
        }
        match self.get_value(path) {
            Ok(actual) => {
                if values_equivalent(expected, &actual) {
                    None
                } else {
                    Some(actual)
                }
            }
            // get_value can fail for write-only params or transient errors.
            // Either way, we can't verify — skip silently.
            Err(_) => None,
        }
    }
}

/// Compare two parameter strings as the firmware sees them. Numeric strings
/// are compared as `f64` with a 1 ppm tolerance to absorb harmless float
/// roundtrip jitter (e.g. `"100"` vs `"100.0"` vs `"1e2"`); everything else
/// is case-insensitive equal (`"True"` vs `"true"`).
fn values_equivalent(a: &str, b: &str) -> bool {
    let at = a.trim();
    let bt = b.trim();
    if let (Ok(af), Ok(bf)) = (at.parse::<f64>(), bt.parse::<f64>()) {
        return (af - bf).abs() <= 1e-6 * af.abs().max(bf.abs()).max(1.0);
    }
    at.eq_ignore_ascii_case(bt)
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

    /// Read raw data from the endpoint using a pre-allocated reusable buffer.
    ///
    /// The buffer must be large enough for the maximum expected data.
    /// CAEN FELib does NOT check buffer bounds — undersized buffers cause SIGBUS.
    ///
    /// # Arguments
    /// * `timeout_ms` - Timeout in milliseconds (-1 for infinite)
    /// * `buffer` - Pre-allocated reusable buffer (capacity = max data size)
    ///
    /// # Returns
    /// * `Ok(Some(RawData))` - Data was read successfully (owns a copy of actual data)
    /// * `Ok(None)` - Timeout (no data available)
    /// * `Err(...)` - Error occurred
    pub fn read_data(
        &self,
        timeout_ms: i32,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<RawData>, CaenError> {
        let buffer_capacity = buffer.capacity();
        // Ensure buffer is usable at full capacity
        buffer.resize(buffer_capacity, 0);

        let mut size: usize = 0;
        let mut n_events: u32 = 0;

        // Call ReadData via C wrapper (handles variadic calling convention)
        let ret = unsafe {
            caen_read_data_raw(
                self.handle,
                timeout_ms,
                buffer.as_mut_ptr(),
                &mut size,
                &mut n_events,
            )
        };

        if ret == 0 {
            // Success - copy actual data to a right-sized Vec
            let data = buffer[..size].to_vec();
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

    /// Read a single decoded event from the OpenDPP endpoint.
    ///
    /// This reads one event at a time with decoded fields.
    /// Use with configure_opendpp_endpoint().
    ///
    /// # Arguments
    /// * `timeout_ms` - Timeout in milliseconds (-1 for infinite)
    /// * `user_info_buffer` - Pre-allocated buffer for user info words
    ///
    /// # Returns
    /// * `Ok(Some(OpenDppEvent))` - Event was read successfully
    /// * `Ok(None)` - Timeout (no data available)
    /// * `Err(...)` - Error occurred
    pub fn read_opendpp_event(
        &self,
        timeout_ms: i32,
        user_info_buffer: &mut [u64],
    ) -> Result<Option<OpenDppEvent>, CaenError> {
        let mut channel: u8 = 0;
        let mut timestamp: u64 = 0;
        let mut fine_timestamp: u16 = 0;
        let mut energy: u16 = 0;
        let mut flags_b: u16 = 0;
        let mut flags_a: u16 = 0;
        let mut psd: u16 = 0;
        let mut user_info_size: usize = 0;
        let mut event_size: usize = 0;

        let ret = unsafe {
            caen_read_data_opendpp(
                self.handle,
                timeout_ms,
                &mut channel,
                &mut timestamp,
                &mut fine_timestamp,
                &mut energy,
                &mut flags_b,
                &mut flags_a,
                &mut psd,
                user_info_buffer.as_mut_ptr(),
                &mut user_info_size,
                &mut event_size,
            )
        };

        if ret == 0 {
            // Success — clamp size to buffer capacity and warn on overflow
            if user_info_size > user_info_buffer.len() {
                eprintln!(
                    "[CAEN] WARNING: user_info_size {} exceeds buffer {} — data truncated!",
                    user_info_size,
                    user_info_buffer.len()
                );
                user_info_size = user_info_buffer.len();
            }
            let user_info = user_info_buffer[..user_info_size].to_vec();
            Ok(Some(OpenDppEvent {
                channel,
                timestamp,
                fine_timestamp,
                energy,
                flags_b,
                flags_a,
                psd,
                user_info,
                waveform: None,
                event_size,
            }))
        } else if ret == -11 {
            // Timeout
            Ok(None)
        } else if ret == -12 {
            // Stop signal
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Stop".to_string(),
                description: "Acquisition stopped".to_string(),
            }))
        } else {
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Unknown error in ReadData (OpenDPP)".to_string(),
            }))
        }
    }

    /// Read a single decoded event with waveform from the OpenDPP endpoint.
    ///
    /// This reads one event at a time with decoded fields and waveform data.
    /// Use with configure_opendpp_endpoint(true).
    ///
    /// # Arguments
    /// * `timeout_ms` - Timeout in milliseconds (-1 for infinite)
    /// * `user_info_buffer` - Pre-allocated buffer for user info words
    /// * `waveform_buffer` - Pre-allocated buffer for waveform samples
    ///
    /// # Returns
    /// * `Ok(Some(OpenDppEvent))` - Event was read successfully
    /// * `Ok(None)` - Timeout (no data available)
    /// * `Err(...)` - Error occurred
    pub fn read_opendpp_event_with_waveform(
        &self,
        timeout_ms: i32,
        user_info_buffer: &mut [u64],
        waveform_buffer: &mut [u16],
    ) -> Result<Option<OpenDppEvent>, CaenError> {
        let mut channel: u8 = 0;
        let mut timestamp: u64 = 0;
        let mut fine_timestamp: u16 = 0;
        let mut energy: u16 = 0;
        let mut flags_b: u16 = 0;
        let mut flags_a: u16 = 0;
        let mut psd: u16 = 0;
        let mut user_info_size: usize = 0;
        let mut waveform_size: usize = 0;
        let mut event_size: usize = 0;

        let ret = unsafe {
            caen_read_data_opendpp_waveform(
                self.handle,
                timeout_ms,
                &mut channel,
                &mut timestamp,
                &mut fine_timestamp,
                &mut energy,
                &mut flags_b,
                &mut flags_a,
                &mut psd,
                user_info_buffer.as_mut_ptr(),
                &mut user_info_size,
                waveform_buffer.as_mut_ptr(),
                &mut waveform_size,
                &mut event_size,
            )
        };

        if ret == 0 {
            // Success — clamp sizes to buffer capacity and warn on overflow
            if user_info_size > user_info_buffer.len() {
                eprintln!(
                    "[CAEN] WARNING: user_info_size {} exceeds buffer {} — data truncated!",
                    user_info_size,
                    user_info_buffer.len()
                );
                user_info_size = user_info_buffer.len();
            }
            if waveform_size > waveform_buffer.len() {
                eprintln!(
                    "[CAEN] WARNING: waveform_size {} exceeds buffer {} — data truncated!",
                    waveform_size,
                    waveform_buffer.len()
                );
                waveform_size = waveform_buffer.len();
            }
            let user_info = user_info_buffer[..user_info_size].to_vec();
            let waveform = if waveform_size > 0 {
                Some(waveform_buffer[..waveform_size].to_vec())
            } else {
                None
            };
            Ok(Some(OpenDppEvent {
                channel,
                timestamp,
                fine_timestamp,
                energy,
                flags_b,
                flags_a,
                psd,
                user_info,
                waveform,
                event_size,
            }))
        } else if ret == -11 {
            // Timeout
            Ok(None)
        } else if ret == -12 {
            // Stop signal
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Stop".to_string(),
                description: "Acquisition stopped".to_string(),
            }))
        } else {
            Err(CaenError::from_code(ret).unwrap_or(CaenError {
                code: ret,
                name: "Unknown".to_string(),
                description: "Unknown error in ReadData (OpenDPP with waveform)".to_string(),
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

// ⚠️ Thread-safety (TODO 58 L8): CaenHandle is technically Send + Sync — it is
// just a `u64`, so the auto-traits derive. The FELib documentation, however,
// says CAEN_FELib_Open/Close (and per-handle calls generally) are NOT
// thread-safe. That constraint is enforced by **usage convention only**: each
// handle is created and used inside a single read-loop thread. Do not share a
// handle across threads; if that is ever needed, wrap it in Arc<Mutex<>> (or
// add a `PhantomData<*mut ()>` field to make the type system enforce !Send —
// deliberately not done yet to avoid churning every read-loop signature).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_equivalent_handles_numeric_and_case() {
        // Identical strings.
        assert!(values_equivalent("100", "100"));
        // Different formattings of the same number.
        assert!(values_equivalent("100", "100.0"));
        assert!(values_equivalent("1.0", "1"));
        assert!(values_equivalent("5000", "5e3"));
        // Whitespace tolerance.
        assert!(values_equivalent("  42 ", "42"));
        // Case-insensitive enum strings.
        assert!(values_equivalent("True", "true"));
        assert!(values_equivalent("LowAVG", "lowavg"));
        // Real differences.
        assert!(!values_equivalent("100", "104"));
        assert!(!values_equivalent("True", "False"));
        assert!(!values_equivalent("LowAVG", "MediumAVG"));
        // Float jitter under 1 ppm tolerance.
        assert!(values_equivalent("1.0000001", "1.0"));
        // Float jitter above 1 ppm tolerance.
        assert!(!values_equivalent("1.0001", "1.0"));
    }

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
            increment: Some("0.1".to_string()),
            default_value: Some("20".to_string()),
            expuom: Some(0),
        };
        assert_eq!(info.name, "DCOffset");
        assert!(info.setinrun);
        assert_eq!(info.min_value, Some("0".to_string()));
        assert_eq!(info.increment, Some("0.1".to_string()));
        assert_eq!(info.expuom, Some(0));
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
            increment: None,
            default_value: None,
            expuom: None,
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
            increment: Some("1".to_string()),
            default_value: Some("100".to_string()),
            expuom: Some(0),
        };
        let cloned = info.clone();
        assert_eq!(info.name, cloned.name);
        assert_eq!(info.setinrun, cloned.setinrun);
        assert_eq!(info.increment, cloned.increment);
    }

    #[test]
    fn test_extract_param_info_from_json() {
        // Simulate DevTree parameter node structure (DC offset with all metadata)
        let json_str = r#"{
            "accessmode": { "value": "READ_WRITE" },
            "datatype": { "value": "NUMBER" },
            "setinrun": { "value": "true" },
            "minvalue": { "value": "0.0" },
            "maxvalue": { "value": "100.0" },
            "increment": { "value": "0.1" },
            "defaultvalue": { "value": "20" },
            "expuom": { "value": "0" },
            "uom": { "value": "%" }
        }"#;

        let node: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let info = CaenHandle::extract_param_info("DCOffset", &node).unwrap();

        assert_eq!(info.name, "DCOffset");
        assert_eq!(info.datatype, "NUMBER");
        assert!(info.setinrun);
        assert_eq!(info.min_value, Some("0.0".to_string()));
        assert_eq!(info.max_value, Some("100.0".to_string()));
        assert_eq!(info.unit, Some("%".to_string()));
        assert_eq!(info.increment, Some("0.1".to_string()));
        assert_eq!(info.default_value, Some("20".to_string()));
        assert_eq!(info.expuom, Some(0));
    }

    #[test]
    fn test_extract_param_info_time_param() {
        // Simulate DevTree node for a time parameter (ch_trg_holdoff from PSD1)
        let json_str = r#"{
            "accessmode": { "value": "READ_WRITE" },
            "datatype": { "value": "NUMBER" },
            "setinrun": { "value": "true" },
            "minvalue": { "value": "0" },
            "maxvalue": { "value": "524280" },
            "increment": { "value": "8" },
            "defaultvalue": { "value": "1024" },
            "expuom": { "value": "-9" },
            "uom": { "value": "s" }
        }"#;

        let node: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let info = CaenHandle::extract_param_info("ch_trg_holdoff", &node).unwrap();

        assert_eq!(info.increment, Some("8".to_string()));
        assert_eq!(info.expuom, Some(-9));
        assert_eq!(info.default_value, Some("1024".to_string()));
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

    /// Test build_param_cache logic by parsing a real DevTree JSON file
    #[test]
    fn test_build_param_cache_from_devtree_json() {
        // Load real DevTree JSON from docs/devtree_examples/
        let json_str =
            std::fs::read_to_string("docs/devtree_examples/dt5730b_psd1_sn990.json").unwrap();
        let tree: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        let mut cache = HashMap::new();

        // Collect board-level parameters from /par/
        if let Some(par) = tree.get("par").and_then(|v| v.as_object()) {
            for (name, node) in par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    if obj.contains_key("datatype") {
                        if let Ok(info) = CaenHandle::extract_param_info(name, node) {
                            cache.insert(name.clone(), info);
                        }
                    }
                }
            }
        }

        // Collect channel-level parameters from /ch/0/par/
        if let Some(ch0_par) = tree
            .get("ch")
            .and_then(|ch| ch.get("0"))
            .and_then(|ch0| ch0.get("par"))
            .and_then(|v| v.as_object())
        {
            for (name, node) in ch0_par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    if obj.contains_key("datatype") && !cache.contains_key(name) {
                        if let Ok(info) = CaenHandle::extract_param_info(name, node) {
                            cache.insert(name.clone(), info);
                        }
                    }
                }
            }
        }

        // Verify we got a reasonable number of params
        assert!(cache.len() > 20, "Expected >20 params, got {}", cache.len());

        // Verify specific board-level params exist
        assert!(cache.contains_key("startmode"), "Missing startmode");
        assert!(
            cache.contains_key("dt_ext_clock"),
            "Missing dt_ext_clock (Desktop)"
        );

        // Verify specific channel-level params exist with correct metadata
        let pretrg = cache.get("ch_pretrg").expect("Missing ch_pretrg");
        assert_eq!(pretrg.datatype, "NUMBER");
        assert_eq!(pretrg.increment.as_deref(), Some("8"));
        assert_eq!(pretrg.min_value.as_deref(), Some("40"));
        assert_eq!(pretrg.expuom, Some(-9));

        let gate = cache.get("ch_gate").expect("Missing ch_gate");
        assert_eq!(gate.increment.as_deref(), Some("2"));
        assert_eq!(gate.min_value.as_deref(), Some("4"));

        let holdoff = cache.get("ch_trg_holdoff").expect("Missing ch_trg_holdoff");
        assert_eq!(holdoff.increment.as_deref(), Some("8"));

        let dc_offset = cache.get("ch_dcoffset").expect("Missing ch_dcoffset");
        assert_eq!(dc_offset.increment.as_deref(), Some("0.1"));
        assert_eq!(dc_offset.min_value.as_deref(), Some("0.0"));
        assert_eq!(dc_offset.max_value.as_deref(), Some("100.0"));
    }

    /// Test that param_cache correctly validates real PSD1 parameters
    #[test]
    fn test_param_cache_validation_integration() {
        let json_str =
            std::fs::read_to_string("docs/devtree_examples/dt5730b_psd1_sn990.json").unwrap();
        let tree: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        let mut cache = HashMap::new();

        // Build cache (same logic as build_param_cache)
        if let Some(ch0_par) = tree
            .get("ch")
            .and_then(|ch| ch.get("0"))
            .and_then(|ch0| ch0.get("par"))
            .and_then(|v| v.as_object())
        {
            for (name, node) in ch0_par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    if obj.contains_key("datatype") {
                        if let Ok(info) = CaenHandle::extract_param_info(name, node) {
                            cache.insert(name.clone(), info);
                        }
                    }
                }
            }
        }

        // Validate ch_pretrg: 101 → 104 (step=8, min=40)
        let pretrg = cache.get("ch_pretrg").unwrap();
        let result = validation::validate_param("101", pretrg);
        assert!(result.adjusted);
        assert_eq!(result.value, "104");

        // Validate ch_gate: 301 → 302 (step=2, min=4)
        let gate = cache.get("ch_gate").unwrap();
        let result = validation::validate_param("301", gate);
        assert!(result.adjusted);
        assert_eq!(result.value, "302");

        // Validate ch_dcoffset: 50.0 → 50.0 (step=0.1, exact)
        let dc = cache.get("ch_dcoffset").unwrap();
        let result = validation::validate_param("50.0", dc);
        assert!(!result.adjusted);
        assert_eq!(result.value, "50.0");

        // Validate ch_dcoffset: 50.35 → 50.4 (step=0.1)
        let result = validation::validate_param("50.35", dc);
        assert!(result.adjusted);
        assert_eq!(result.value, "50.4");
    }

    /// Regression: 2026-05-04 we discovered that `to_caen_parameters`
    /// emits CamelCase param names (`ChRecordLengthT`) while the DevTree
    /// uses lowercase (`chrecordlengtht`), so every CamelCase path was
    /// silently bypassing validation. The visible bug was a user typing
    /// record_length_ns=17000 (above the 16200-ns DevTree max) into the
    /// PHA2 settings — the FW rejected the broadcast write with CAEN
    /// error -6 and the channel got stuck at the previous value.
    /// This test pins the case-insensitive cache contract.
    #[test]
    fn param_cache_lookup_is_case_insensitive() {
        let json_str =
            std::fs::read_to_string("docs/devtree_examples/vx2730_pha2_sn52622.json").unwrap();
        let tree: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        let mut cache = HashMap::new();
        if let Some(ch0_par) = tree
            .get("ch")
            .and_then(|ch| ch.get("0"))
            .and_then(|ch0| ch0.get("par"))
            .and_then(|v| v.as_object())
        {
            for (name, node) in ch0_par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    if obj.contains_key("datatype") {
                        if let Ok(info) = CaenHandle::extract_param_info(name, node) {
                            // Same normalization the production cache uses.
                            cache.insert(name.to_lowercase(), info);
                        }
                    }
                }
            }
        }

        // CamelCase as emitted by to_caen_parameters → must hit the cache.
        let path = "/ch/0..31/par/ChRecordLengthT";
        let param_name_raw = path.rsplit('/').next().unwrap();
        let lookup_key = param_name_raw.to_lowercase();
        let info = cache
            .get(lookup_key.as_str())
            .expect("CamelCase param must resolve via lowercased key");

        // 17000 ns is above the PHA2 DevTree max (16200) → must clamp.
        let result = validation::validate_param("17000", info);
        assert!(result.adjusted, "out-of-range value must be adjusted");
        assert_eq!(
            result.value, "16200",
            "17000 ns must clamp to the 16200-ns max",
        );
    }

    /// Regression: unknown parameter names must miss the cache so that
    /// `apply_params_validated` routes them through the **loud** no-cache
    /// branch (logged at `info!`, counted in `result.no_cache`). The 5/4
    /// silent-clamp-bypass bug was a CamelCase path that *should* have
    /// hit the cache but didn't — this test pins the inverse: when a name
    /// genuinely doesn't exist in the DevTree, the cache must say None
    /// rather than fishing for partial matches.
    #[test]
    fn unknown_param_name_misses_cache() {
        let json_str =
            std::fs::read_to_string("docs/devtree_examples/vx2730_pha2_sn52622.json").unwrap();
        let tree: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        let mut cache = HashMap::new();
        if let Some(ch0_par) = tree
            .get("ch")
            .and_then(|ch| ch.get("0"))
            .and_then(|ch0| ch0.get("par"))
            .and_then(|v| v.as_object())
        {
            for (name, node) in ch0_par {
                if name == "handle" {
                    continue;
                }
                if let Some(obj) = node.as_object() {
                    if obj.contains_key("datatype") {
                        if let Ok(info) = CaenHandle::extract_param_info(name, node) {
                            cache.insert(name.to_lowercase(), info);
                        }
                    }
                }
            }
        }

        let raw = "ThisParamDefinitelyDoesNotExist";
        let lower = raw.to_lowercase();
        assert!(
            !cache.contains_key(raw) && !cache.contains_key(lower.as_str()),
            "unknown param name must miss both raw and lowercase cache lookup",
        );
    }

    /// Pin the wire format of `ApplyConfigResult.no_cache` + the new
    /// `ParamApplyStatus::NoCache` variant. The Operator REST API exposes
    /// these in `/api/digitizers/.../apply` responses, so the contract
    /// must roundtrip cleanly through serde_json.
    #[test]
    fn apply_config_result_round_trips_no_cache_status() {
        use crate::reader::caen::validation::{
            ApplyConfigResult, ParamApplyResult, ParamApplyStatus,
        };

        let original = ApplyConfigResult {
            total: 1,
            ok: 0,
            adjusted: 0,
            failed: 0,
            skipped: 0,
            no_cache: 1,
            details: vec![ParamApplyResult {
                path: "/par/dt_ext_clock".into(),
                original_value: "true".into(),
                applied_value: "true".into(),
                status: ParamApplyStatus::NoCache,
                message: Some("no cache entry — passed unvalidated to FW".into()),
            }],
        };

        let json = serde_json::to_string(&original).unwrap();
        let restored: ApplyConfigResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.no_cache, 1);
        assert_eq!(restored.details.len(), 1);
        assert_eq!(restored.details[0].status, ParamApplyStatus::NoCache);

        // Backward-compat: an old wire payload without `no_cache` must
        // still deserialize (default to 0). #[serde(default)] on the field
        // gates this.
        let old_wire = serde_json::json!({
            "total": 0, "ok": 0, "adjusted": 0, "failed": 0,
            "skipped": 0, "details": []
        });
        let restored: ApplyConfigResult = serde_json::from_value(old_wire).unwrap();
        assert_eq!(restored.no_cache, 0);
    }
}
