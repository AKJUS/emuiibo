use nx::result::*;
use nx::results;
use nx::ipc::sf;
use nx::ipc::sf::nfp;
use nx::ipc::sf::applet;
use nx::wait;
use nx::sync;
use nx::service::hid;
use nx::input;
use nx::thread;

use crate::area;
use crate::emu;

pub struct EmulationHandler {
    session: sf::Session,
    application_id: u64,
    activate_event: wait::SystemEvent,
    deactivate_event: wait::SystemEvent,
    availability_change_event: wait::SystemEvent,
    state: sync::Locked<nfp::State>,
    device_state: sync::Locked<nfp::DeviceState>,
    should_end_thread: sync::Locked<bool>,
    current_opened_area: area::ApplicationArea,
    emu_handler_thread: thread::Thread,
    input_ctx: input::InputContext
}

impl EmulationHandler {
    pub fn new(application_id: u64) -> Result<Self> {
        log!("\n[{:#X}] New handler!\n", application_id);
        let supported_tags = hid::NpadStyleTag::ProController() | hid::NpadStyleTag::Handheld() | hid::NpadStyleTag::JoyconPair() | hid::NpadStyleTag::JoyconLeft() | hid::NpadStyleTag::JoyconRight() | hid::NpadStyleTag::SystemExt() | hid::NpadStyleTag::System();
        let controllers = [hid::ControllerId::Player1, hid::ControllerId::Handheld];
        let input_ctx = input::InputContext::new(0, supported_tags, &controllers)?;
        Ok(Self { session: sf::Session::new(), application_id: application_id, activate_event: wait::SystemEvent::empty(), deactivate_event: wait::SystemEvent::empty(), availability_change_event: wait::SystemEvent::empty(), state: sync::Locked::new(false, nfp::State::NonInitialized), device_state: sync::Locked::new(false, nfp::DeviceState::Unavailable), should_end_thread: sync::Locked::new(false, false), emu_handler_thread: thread::Thread::empty(), current_opened_area: area::ApplicationArea::new(), input_ctx: input_ctx })
    }

    pub fn get_application_id(&self) -> u64 {
        self.application_id
    }

    pub fn is_state(&mut self, state: nfp::State) -> bool {
        self.state.get_val() == state
    }

    pub fn is_device_state(&mut self, device_state: nfp::DeviceState) -> bool {
        self.device_state.get_val() == device_state
    }

    pub fn get_session(&mut self) -> &mut sf::Session {
        &mut self.session
    }

    fn handle_virtual_amiibo_status(&mut self, status: emu::VirtualAmiiboStatus) {
        match status {
            emu::VirtualAmiiboStatus::Connected => match self.device_state.get_val() {
                nfp::DeviceState::SearchingForTag => {
                    self.device_state.set(nfp::DeviceState::TagFound);
                    self.activate_event.signal().unwrap();
                },
                _ => {}
            },
            emu::VirtualAmiiboStatus::Disconnected => match self.device_state.get_val() {
                nfp::DeviceState::TagFound | nfp::DeviceState::TagMounted => {
                    self.device_state.set(nfp::DeviceState::SearchingForTag);
                    self.deactivate_event.signal().unwrap();
                },
                _ => {}
            },
            _ => {}
        };
    }

    fn emu_handler_thread_fn(handler: &*mut EmulationHandler) {
        unsafe {
            loop {
                if (*(*handler)).should_end_thread.get_val() {
                    break;
                }
                
                let status = emu::get_active_virtual_amiibo_status();
                (*(*handler)).handle_virtual_amiibo_status(status);
                let _ = thread::sleep(100_000_000);
            }
        }
    }

    pub fn initialize(&mut self, aruid: applet::AppletResourceUserId, process_id: sf::ProcessId, mcu_data: sf::InMapAliasBuffer) -> Result<()> {
        // TODO: make use of aruid or mcu data?
        result_return_unless!(self.is_state(nfp::State::NonInitialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Initialize -- aruid: {}, process_id: {}, mcu_data: (buf_size: {:#X})\n", self.application_id, aruid, process_id.process_id, mcu_data.size);

        self.state.set(nfp::State::Initialized);
        self.device_state.set(nfp::DeviceState::Initialized);
        
        self.activate_event = wait::SystemEvent::new()?;
        self.deactivate_event = wait::SystemEvent::new()?;
        self.availability_change_event = wait::SystemEvent::new()?;

        // TODO: different thread names depending on the app id?
        self.emu_handler_thread = thread::Thread::new(Self::emu_handler_thread_fn, &(self as *mut Self), "emuiibo.AmiiboEmulationHandler", 0x1000)?;
        self.emu_handler_thread.initialize(0x2B, -2)?;
        self.emu_handler_thread.start()?;
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Finalize -- (...)\n", self.application_id);

        self.state.set(nfp::State::NonInitialized);
        self.device_state.set(nfp::DeviceState::Finalized);
        Ok(())
    }

    pub fn list_devices(&mut self, out_devices: sf::OutPointerBuffer) -> Result<u32> {
        let mut devices: &mut [nfp::DeviceHandle] = out_devices.get_mut_slice();
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] ListDevices -- out_devices: (array_count: {})\n", self.application_id, devices.len());

        // Send a single fake device handle
        devices[0].id = match self.input_ctx.is_controller_connected(hid::ControllerId::Player1) {
            true => hid::ControllerId::Player1,
            false => hid::ControllerId::Handheld
        } as u32;
        Ok(1)
    }

    pub fn start_detection(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::Initialized) || self.is_device_state(nfp::DeviceState::TagRemoved), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] StartDetection -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        self.device_state.set(nfp::DeviceState::SearchingForTag);
        Ok(())
    }

    pub fn stop_detection(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] StopDetection -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        self.device_state.set(nfp::DeviceState::Initialized);
        Ok(())
    }

    pub fn mount(&mut self, device_handle: nfp::DeviceHandle, model_type: nfp::ModelType, mount_target: nfp::MountTarget) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Mount -- device_handle: (id: {}), model_type: {:?}, mount_target: {:?}\n", self.application_id, device_handle.id, model_type, mount_target);
        
        self.device_state.set(nfp::DeviceState::TagMounted);
        Ok(())
    }

    pub fn unmount(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Unmount -- device_handle: (id: {})\n", self.application_id, device_handle.id);
        
        self.device_state.set(nfp::DeviceState::TagFound);
        Ok(())
    }

    pub fn open_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] OpenApplicationArea -- device_handle: (id: {}), access_id: {:#X}\n", self.application_id, device_handle.id, access_id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let application_area = area::ApplicationArea::from(&amiibo, access_id);
        result_return_unless!(application_area.exists(), results::nfp::ResultAreaNeedsToBeCreated);

        amiibo.update_area_program_id(access_id, self.application_id)?;
        self.current_opened_area = application_area;
        Ok(())
    }

    pub fn get_application_area(&mut self, device_handle: nfp::DeviceHandle, out_data: sf::OutMapAliasBuffer) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), results::nfp::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] GetApplicationArea -- device_handle: (id: {}), out_data: (buf_size: {:#X})\n", self.application_id, device_handle.id, out_data.size);

        let area_size = self.current_opened_area.get_size()?;
        let size = core::cmp::min(area_size, out_data.size);
        
        self.current_opened_area.read(out_data.buf as *mut u8, size)?;
        Ok(size as u32)
    }

    pub fn set_application_area(&mut self, device_handle: nfp::DeviceHandle, data: sf::InMapAliasBuffer) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), results::nfp::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] SetApplicationArea -- device_handle: (id: {}), data: (buf_size: {:#X})\n", self.application_id, device_handle.id, data.size);

        let area_size = self.current_opened_area.get_size()?;
        let size = core::cmp::min(area_size, data.size);

        self.current_opened_area.write(data.buf, size)?;
        let amiibo = emu::get_active_virtual_amiibo();
        amiibo.notify_written()?;
        Ok(())
    }

    pub fn flush(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Flush -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        Ok(())
    }

    pub fn restore(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] Restore -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        Ok(())
    }

    pub fn create_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId, data: sf::InMapAliasBuffer) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] CreateApplicationArea -- device_handle: (id: {}), access_id: {:#X}, data: (buf_size: {:#X})\n", self.application_id, device_handle.id, access_id, data.size);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let application_area = area::ApplicationArea::from(&amiibo, access_id);
        result_return_if!(application_area.exists(), results::nfp::ResultAreaNeedsToBeCreated);

        application_area.create(data.buf, data.size, false)?;
        amiibo.notify_written()?;
        Ok(())
    }

    pub fn get_tag_info(&mut self, device_handle: nfp::DeviceHandle, mut out_tag_info: sf::OutFixedPointerBuffer<nfp::TagInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagFound) || self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetTagInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let tag_info = amiibo.produce_tag_info()?;
        out_tag_info.set_as(tag_info);
        Ok(())
    }

    pub fn get_register_info(&mut self, device_handle: nfp::DeviceHandle, mut out_register_info: sf::OutFixedPointerBuffer<nfp::RegisterInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetRegisterInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let register_info = amiibo.produce_register_info()?;
        out_register_info.set_as(register_info);
        Ok(())
    }

    pub fn get_common_info(&mut self, device_handle: nfp::DeviceHandle, mut out_common_info: sf::OutFixedPointerBuffer<nfp::CommonInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetCommonInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let common_info = amiibo.produce_common_info()?;
        out_common_info.set_as(common_info);
        Ok(())
    }

    pub fn get_model_info(&mut self, device_handle: nfp::DeviceHandle, mut out_model_info: sf::OutFixedPointerBuffer<nfp::ModelInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetModelInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let model_info = amiibo.produce_model_info()?;
        out_model_info.set_as(model_info);
        Ok(())
    }

    pub fn attach_activate_event(&mut self, device_handle: nfp::DeviceHandle) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] AttachActivateEvent -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        Ok(sf::Handle::from(self.activate_event.client_handle))
    }

    pub fn attach_deactivate_event(&mut self, device_handle: nfp::DeviceHandle) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] AttachDeactivateEvent -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        Ok(sf::Handle::from(self.deactivate_event.client_handle))
    }

    pub fn get_state(&mut self) -> Result<nfp::State> {
        log!("[{:#X}] GetState -- (...)\n", self.application_id);
        Ok(self.state.get_val())
    }

    pub fn get_device_state(&mut self, device_handle: nfp::DeviceHandle) -> Result<nfp::DeviceState> {
        log!("[{:#X}] GetDeviceState -- device_handle: (id: {})\n", self.application_id, device_handle.id);
        Ok(self.device_state.get_val())
    }

    pub fn get_npad_id(&mut self, device_handle: nfp::DeviceHandle) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetNpadId -- device_handle: (id: {})\n", self.application_id, device_handle.id);
        
        Ok(device_handle.id)
    }

    pub fn get_application_area_size(&mut self, device_handle: nfp::DeviceHandle) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), results::nfp::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] GetApplicationAreaSize -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let area_size = self.current_opened_area.get_size()?;
        Ok(area_size as u32)
    }

    pub fn attach_availability_change_event(&mut self) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] AttachAvailabilityChangeEvent -- (...)\n", self.application_id);

        Ok(sf::Handle::from(self.availability_change_event.client_handle))
    }

    pub fn recreate_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId, data: sf::InMapAliasBuffer) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] RecreateApplicationArea -- device_handle: (id: {}), access_id: {:#X}, data: (buf_size_: {:#X})\n", self.application_id, device_handle.id, access_id, data.size);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let application_area = area::ApplicationArea::from(&amiibo, access_id);
        application_area.create(data.buf, data.size, true)?;
        amiibo.notify_written()?;
        Ok(())
    }

    pub fn format(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        log!("[{:#X}] Format -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        Ok(())
    }

    pub fn get_admin_info(&mut self, device_handle: nfp::DeviceHandle, mut out_admin_info: sf::OutFixedPointerBuffer<nfp::AdminInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetAdminInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);
        
        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let admin_info = amiibo.produce_admin_info()?;
        out_admin_info.set_as(admin_info);
        Ok(())
    }

    pub fn get_register_info_private(&mut self, device_handle: nfp::DeviceHandle, mut out_register_info_private: sf::OutFixedPointerBuffer<nfp::RegisterInfoPrivate>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] GetRegisterInfoPrivate -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let register_info_private = amiibo.produce_register_info_private()?;
        out_register_info_private.set_as(register_info_private);
        Ok(())
    }

    pub fn set_register_info_private(&mut self, device_handle: nfp::DeviceHandle, register_info_private: sf::InFixedPointerBuffer<nfp::RegisterInfoPrivate>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] SetRegisterInfoPrivate -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        let register_info_private_type = register_info_private.get_as::<nfp::RegisterInfoPrivate>();
        amiibo.update_from_register_info_private(register_info_private_type)?;
        Ok(())
    }

    pub fn delete_register_info(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] DeleteRegisterInfo -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        amiibo.delete_all_areas()?;
        Ok(())
    }

    pub fn delete_application_area(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] DeleteApplicationArea -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);

        amiibo.delete_current_area()?;
        Ok(())
    }

    pub fn exists_application_area(&mut self, device_handle: nfp::DeviceHandle) -> Result<bool> {
        result_return_unless!(self.is_state(nfp::State::Initialized), results::nfp::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), results::nfp::ResultDeviceNotFound);
        log!("[{:#X}] ExistsApplicationArea -- device_handle: (id: {})\n", self.application_id, device_handle.id);

        let amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_valid(), results::nfp::ResultDeviceNotFound);
        Ok(amiibo.has_any_application_areas())
    }
}

impl Drop for EmulationHandler {
    fn drop(&mut self) {
        log!("[{:#X}] Dropping handler...\n", self.application_id);
        self.should_end_thread.set(true);
        self.emu_handler_thread.join().unwrap();
    }
}

pub mod user;

pub mod sys;