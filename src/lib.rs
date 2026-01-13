//! DELILA-RS: High-performance DAQ system for nuclear physics experiments
//!
//! This crate provides a distributed data acquisition pipeline using ZeroMQ.

pub mod common;
pub mod config;
pub mod data_sink;
pub mod data_source_emulator;
pub mod merger;
pub mod operator;
pub mod reader;
