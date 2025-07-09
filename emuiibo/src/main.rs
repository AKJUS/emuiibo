#![no_std]
#![no_main]

#[macro_use]
extern crate nx;

extern crate alloc;

extern crate emuiibo;

use nx::diag::abort;
use nx::fs;
use nx::ipc::server;
use nx::result::*;
use nx::thread;
use nx::util;

use emuiibo::*;

use core::panic;

rrt0_define_default_module_name!();

const CUSTOM_HEAP_SIZE: usize = 0x8000;
static mut CUSTOM_HEAP: [u8; CUSTOM_HEAP_SIZE] = [0; CUSTOM_HEAP_SIZE];

#[unsafe(no_mangle)]
#[allow(static_mut_refs)]
pub fn initialize_heap(_hbl_heap: util::PointerAndSize) -> util::PointerAndSize {
    unsafe {
        // SAFETY: CUSTOM_HEAP must only ever be referenced from here, and nowhere else.
        util::PointerAndSize::new(&raw mut CUSTOM_HEAP as _, CUSTOM_HEAP.len())
    }
}

#[unsafe(no_mangle)]
pub fn main() -> Result<()> {
    thread::set_current_thread_name("emuiibo.Main");
    fs::initialize_fspsrv_session()?;
    fs::mount_sd_card("sdmc")?;
    fsext::ensure_directories()?;

    if let Err(rc) = logger::initialize() {
        let _a = rc;
    }
    log!("Logging Initialized!\n");

    if let Err(e) = nx::rand::initialize() {
        log!("Error initlializing rand provider: {:?}", e);
        return Ok(());
    }

    if let Err(e) = nx::mii::initialize() {
        log!("Error initlializing mii module provider: {:?}", e);
        return Ok(());
    }

    if let Err(e) = miiext::export_miis() {
        log!("Error exporting mii module provider: {:?}", e);
        return Ok(());
    }

    amiibo::compat::convert_deprecated_virtual_amiibos();
    emu::load_emulation_status();

    if let Err(e) = ipc::nfp::initialize() {
        log!("Error initializing nfp module provider: {:?}", e);
        return Ok(());
    }

    const POINTER_BUF_SIZE: usize = 0x1000;
    type Manager = server::ServerManager<POINTER_BUF_SIZE>;

    let mut manager = Manager::new()?;
    manager.register_mitm_service_server::<ipc::nfp::user::UserManager>()?;
    manager.register_mitm_service_server::<ipc::nfp::sys::SystemManager>()?;
    manager.register_service_server::<ipc::emu::EmulationServer>()?;

    if let Err(e) = manager.loop_process() {
        log!("Error occured running server manager loop: {:?}", e);
    }

    panic!("exiting MitM Servers is not supported.");
}

#[panic_handler]
fn panic_handler(info: &panic::PanicInfo) -> ! {
    util::simple_panic_handler::<nx::diag::log::lm::LmLogger>(info, abort::AbortLevel::SvcBreak())
}
