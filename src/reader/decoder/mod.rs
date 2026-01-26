//! Decoder module for CAEN digitizer raw data
//!
//! Converts raw binary data from digitizers into structured EventData.

pub mod common;
pub mod psd1;
pub mod psd2;

pub use common::{DataType, DecodeResult, EventData, RawData, Waveform};
pub use psd1::{Psd1Config, Psd1Decoder};
pub use psd2::{Psd2Config, Psd2Decoder};
