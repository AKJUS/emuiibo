#![no_std]

// let it go for now.
#![allow(unsafe_op_in_unsafe_fn)]

#[macro_use]
extern crate nx;

#[macro_use]
extern crate static_assertions;

#[macro_use]
extern crate alloc;

extern crate paste;

#[macro_use]
pub mod logger;

pub mod rc;

#[macro_use]
pub mod fsext;

pub mod miiext;

pub mod ipc;

pub mod emu;

pub mod amiibo;

pub mod area;

pub use ipc::emu::{EmulationService, IEmulationServiceClient};
