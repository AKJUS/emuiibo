use alloc::sync::Arc;
use nx::result::*;
use nx::ipc::sf;
use nx::ipc::sf::nfp;
use nx::ipc::sf::ncm;
use nx::sync::Mutex;
use nx::wait;
use nx::sync;
use nx::service::hid;
use nx::input;
use nx::thread;
use crate::area;
use crate::emu;

static G_INPUT_CTX: generic_once_cell::OnceCell<sync::sys::mutex::Mutex,input::Context> = generic_once_cell::OnceCell::new();

pub fn initialize() -> Result<()> {
    let supported_tags = hid::NpadStyleTag::FullKey() | hid::NpadStyleTag::Handheld() | hid::NpadStyleTag::JoyDual() | hid::NpadStyleTag::JoyLeft() | hid::NpadStyleTag::JoyRight();

    let _ = G_INPUT_CTX.set(input::Context::new(supported_tags, 1)?);

    Ok(())
}

pub fn get_input_context() -> &'static input::Context {
    G_INPUT_CTX.get().unwrap()
}

pub struct EmulationHandler {
    application_id: ncm::ProgramId,
    emulation_state: Arc<Mutex<EmulationState>>,
    current_opened_area: area::ApplicationArea,
    emu_handler_thread: Option<thread::JoinHandle<()>>
}

pub struct EmulationState {
    activate_event: wait::SystemEvent,
    deactivate_event: wait::SystemEvent,
    availability_change_event: wait::SystemEvent,
    state: nfp::State,
    device_state: nfp::DeviceState,
    should_end_thread: bool,
}

impl EmulationState {
    pub fn new() -> Result<Self> {
        Ok(Self { activate_event: wait::SystemEvent::new()?, deactivate_event: wait::SystemEvent::new()?, availability_change_event: wait::SystemEvent::new()?, state: nfp::State::NonInitialized, device_state: nfp::DeviceState::Unavailable, should_end_thread: false })
    }

    fn handle_virtual_amiibo_status(&mut self, status: emu::VirtualAmiiboStatus) {
        match status {
            emu::VirtualAmiiboStatus::Connected => match self.device_state {
                nfp::DeviceState::SearchingForTag => {
                    self.device_state = nfp::DeviceState::TagFound;
                    self.activate_event.signal().expect("signaling our own event should never fail");
                },
                _ => {}
            },
            emu::VirtualAmiiboStatus::Disconnected => match self.device_state{
                nfp::DeviceState::TagFound | nfp::DeviceState::TagMounted => {
                    self.device_state =nfp::DeviceState::SearchingForTag;
                    self.deactivate_event.signal().expect("signaling our own event should never fail");
                },
                _ => {}
            },
            _ => {}
        };
    }
}

impl EmulationHandler {
    pub fn new(application_id: ncm::ProgramId) -> Result<Self> {
        log!("\n[{:#X}] New handler!\n", application_id.0);
        
        Ok(Self { application_id, emulation_state: Arc::new(Mutex::new(EmulationState::new()?)), emu_handler_thread: None, current_opened_area: area::ApplicationArea::new() })
    }

    #[inline]
    pub fn get_application_id(&self) -> ncm::ProgramId {
        self.application_id
    }

    pub fn is_state(&mut self, state: nfp::State) -> bool {
        self.emulation_state.lock().state == state
    }

    pub fn is_device_state(&mut self, device_state: nfp::DeviceState) -> bool {
        self.emulation_state.lock().device_state == device_state
    }

    fn emu_handler_thread_fn(handler: Arc<Mutex<EmulationState>>) {
        loop {
            {
                let handle_handle = handler.lock();
                if handle_handle.should_end_thread == true {
                    return;
                }
            }
            
            let mut handle_handle = handler.lock();
            let status = emu::get_active_virtual_amiibo_status();
            handle_handle.handle_virtual_amiibo_status(status);
            let _ = thread::sleep(100_000_000);
        }

    }

    pub fn initialize(&mut self, aruid: sf::AppletResourceUserId, mcu_data: sf::InMapAliasBuffer<nfp::McuVersionData>) -> Result<()> {
        // TODO: make use of aruid or mcu data?
        result_return_unless!(self.is_state(nfp::State::NonInitialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Initialize -- aruid: {}, process_id: {}, mcu_version_data: (count: {})\n", self.application_id.0, aruid.aruid, aruid.process_id, mcu_data.get_count());
        let mcu_ver_datas = mcu_data.get_maybe_unaligned();
        for mcu_ver_data in mcu_ver_datas {
            log!("[{:#X}] Initialize -- mcu version: {}\n", self.application_id.0, mcu_ver_data.version);
        }

        {
            let mut emulation_state_handle = self.emulation_state.lock();
            emulation_state_handle.state = nfp::State::Initialized;
            emulation_state_handle.device_state = nfp::DeviceState::Initialized;
        }

        // TODO: different thread names depending on the app id?
        let thread_name = format!("emuWoker:{:X?}", self.application_id.0);
        let handle = self.emulation_state.clone();
        self.emu_handler_thread = Some(thread::Builder::new().core(thread::ThreadStartCore::Default).priority(thread::ThreadPriority::Set(0x2B)).name(thread_name).stack_size(0x1000).spawn(move || {
            Self::emu_handler_thread_fn(handle);
        })?);
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Finalize -- (...)\n", self.application_id.0);

        let mut emulation_state_handle = self.emulation_state.lock();
        emulation_state_handle.state = nfp::State::NonInitialized;
        emulation_state_handle.device_state = nfp::DeviceState::Finalized;
        Ok(())
    }

    pub fn list_devices(&mut self, mut out_devices: sf::OutPointerBuffer<nfp::DeviceHandle>) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] ListDevices -- out_devices: (count: {})\n", self.application_id.0, out_devices.get_count());

        // Note: a DeviceHandle's id != npad_id on official nfp, but we treat them as the same thing since we don't care about it
        // Official nfp would store the npad_id somewhere else for the command below which retrieves it
        // Send a single fake device handle

        let fake_device_npad_id = {
            let p1 = get_input_context().get_player(hid::NpadIdType::No1);
            if p1.get_style_tag_attributes(hid::NpadStyleTag::FullKey()).contains(hid::NpadAttribute::IsConnected())
                || p1.get_style_tag_attributes(hid::NpadStyleTag::JoyDual()).contains(hid::NpadAttribute::IsConnected())
                || p1.get_style_tag_attributes(hid::NpadStyleTag::JoyLeft()).contains(hid::NpadAttribute::IsConnected()) 
                || p1.get_style_tag_attributes(hid::NpadStyleTag::JoyRight()).contains(hid::NpadAttribute::IsConnected()) {
                hid::NpadIdType::No1
            }
            else {
                hid::NpadIdType::Handheld
            }
        };

        let devices = out_devices.as_maybeuninit_mut()?;
        unsafe {devices[0].assume_init_mut().id = fake_device_npad_id as u32};
        Ok(1)
    }

    pub fn start_detection(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::Initialized) || self.is_device_state(nfp::DeviceState::TagRemoved), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] StartDetection -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        self.emulation_state.lock().device_state = nfp::DeviceState::SearchingForTag;
        Ok(())
    }

    pub fn stop_detection(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] StopDetection -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        self.emulation_state.lock().device_state = nfp::DeviceState::Initialized;
        Ok(())
    }

    pub fn mount(&mut self, device_handle: nfp::DeviceHandle, model_type: nfp::ModelType, mount_target: nfp::MountTarget) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Mount -- device_handle: (fake id: {}), model_type: {:?}, mount_target: {:?}\n", self.application_id.0, device_handle.id, model_type, mount_target);
        
        self.emulation_state.lock().device_state = nfp::DeviceState::TagMounted;
        Ok(())
    }

    pub fn unmount(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Unmount -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);
        
        self.emulation_state.lock().device_state = nfp::DeviceState::TagFound;
        Ok(())
    }

    pub fn open_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] OpenApplicationArea -- device_handle: (fake id: {}), access_id: {:#X}\n", self.application_id.0, device_handle.id, access_id);

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                let application_area = area::ApplicationArea::from_id(&amiibo, self.application_id.0, access_id);
                result_return_unless!(application_area.exists(), nfp::rc::ResultAreaNeedsToBeCreated);

                amiibo.update_area_program_id(access_id, self.application_id)?;
                self.current_opened_area = application_area;
                Ok(())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn get_application_area(&mut self, device_handle: nfp::DeviceHandle, out_data: sf::OutMapAliasBuffer<u8>) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), nfp::rc::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] GetApplicationArea -- device_handle: (fake id: {}), out_data: (buf_size: {:#X})\n", self.application_id.0, device_handle.id, out_data.get_size());

        let area_size = self.current_opened_area.get_size()?;
        let size = core::cmp::min(area_size, out_data.get_size());
        
        // TODO check the pointer for nulls and return appropriate error code
        unsafe {self.current_opened_area.read(out_data.get_address(), size)?};
        Ok(size as u32)
    }

    pub fn set_application_area(&mut self, device_handle: nfp::DeviceHandle, data: sf::InMapAliasBuffer<u8>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), nfp::rc::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] SetApplicationArea -- device_handle: (fake id: {}), data: (buf_size: {:#X})\n", self.application_id.0, device_handle.id, data.get_size());

        let area_size = self.current_opened_area.get_size()?;
        let size = core::cmp::min(area_size, data.get_size());

        // TODO check the pointer for nulls and return appropriate error code
        unsafe {self.current_opened_area.write(data.get_address(), size)?;}
        if let Some(amiibo)  = emu::get_active_virtual_amiibo().as_mut() {
            amiibo.notify_written()?;
        }
        Ok(())
    }

    pub fn flush(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Flush -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        Ok(())
    }

    pub fn restore(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] Restore -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        Ok(())
    }

    pub fn create_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId, data: sf::InMapAliasBuffer<u8>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] CreateApplicationArea -- device_handle: (fake id: {}), access_id: {:#X}, data: (buf_size: {:#X})\n", self.application_id.0, device_handle.id, access_id, data.get_size());

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                let application_area = area::ApplicationArea::from_id(&amiibo, self.application_id.0, access_id);
                result_return_if!(application_area.exists(), nfp::rc::ResultAreaNeedsToBeCreated);
                // TODO check the pointer for nulls and return appropriate error code
                unsafe { application_area.create(data.get_address(), data.get_size(), false)?; }
                amiibo.notify_written()
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn get_tag_info(&mut self, device_handle: nfp::DeviceHandle, mut out_tag_info: sf::OutFixedPointerBuffer<nfp::TagInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagFound) || self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetTagInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_tag_info.set_var(amiibo.produce_tag_info()?);
                Ok(())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn get_register_info(&mut self, device_handle: nfp::DeviceHandle, mut out_register_info: sf::OutFixedPointerBuffer<nfp::RegisterInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetRegisterInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_register_info.set_var(amiibo.produce_register_info()?);
                Ok(())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn get_common_info(&mut self, device_handle: nfp::DeviceHandle, mut out_common_info: sf::OutFixedPointerBuffer<nfp::CommonInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetCommonInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_common_info.set_var(amiibo.produce_common_info()?);
                Ok(())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn get_model_info(&mut self, device_handle: nfp::DeviceHandle, mut out_model_info: sf::OutFixedPointerBuffer<nfp::ModelInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetModelInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_model_info.set_var(amiibo.produce_model_info()?);
                Ok(())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn attach_activate_event(&mut self, device_handle: nfp::DeviceHandle) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] AttachActivateEvent -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        Ok(sf::Handle::from(self.emulation_state.lock().activate_event.client_handle))
    }

    pub fn attach_deactivate_event(&mut self, device_handle: nfp::DeviceHandle) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] AttachDeactivateEvent -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        Ok(sf::Handle::from(self.emulation_state.lock().deactivate_event.client_handle))
    }

    pub fn get_state(&mut self) -> Result<nfp::State> {
        log!("[{:#X}] GetState -- (...)\n", self.application_id.0);
        Ok(self.emulation_state.lock().state)
    }

    pub fn get_device_state(&mut self, device_handle: nfp::DeviceHandle) -> Result<nfp::DeviceState> {
        log!("[{:#X}] GetDeviceState -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);
        Ok(self.emulation_state.lock().device_state)
    }

    pub fn get_npad_id(&mut self, device_handle: nfp::DeviceHandle) -> Result<hid::NpadIdType> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetNpadId -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);
        
        Ok(unsafe { core::mem::transmute(device_handle.id) })
    }

    pub fn get_application_area_size(&mut self, device_handle: nfp::DeviceHandle) -> Result<u32> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.current_opened_area.exists(), nfp::rc::ResultAreaNeedsToBeCreated);
        log!("[{:#X}] GetApplicationAreaSize -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        let area_size = self.current_opened_area.get_size()?;
        Ok(area_size as u32)
    }

    pub fn attach_availability_change_event(&mut self) -> Result<sf::CopyHandle> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] AttachAvailabilityChangeEvent -- (...)\n", self.application_id.0);

        Ok(sf::Handle::from(self.emulation_state.lock().availability_change_event.client_handle))
    }

    pub fn recreate_application_area(&mut self, device_handle: nfp::DeviceHandle, access_id: nfp::AccessId, data: sf::InMapAliasBuffer<u8>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] RecreateApplicationArea -- device_handle: (fake id: {}), access_id: {:#X}, data: (buf_size: {:#X})\n", self.application_id.0, device_handle.id, access_id, data.get_size());

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                let application_area = area::ApplicationArea::from_id(&amiibo, self.application_id.0, access_id);
                // TODO check the pointer for nulls and return appropriate error code
                unsafe {application_area.create(data.get_address(), data.get_size(), true)?; }
                amiibo.notify_written()
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }

    pub fn format(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        log!("[{:#X}] Format -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        Ok(())
    }

    pub fn get_admin_info(&mut self, device_handle: nfp::DeviceHandle, mut out_admin_info: sf::OutFixedPointerBuffer<nfp::AdminInfo>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetAdminInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);
        
        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_admin_info.set_var(amiibo.produce_admin_info()?);
            },
            None => {
                return Err(nfp::rc::ResultDeviceNotFound::make());
            }
        }

        Ok(())
    }

    pub fn get_register_info_private(&mut self, device_handle: nfp::DeviceHandle, mut out_register_info_private: sf::OutFixedPointerBuffer<nfp::RegisterInfoPrivate>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] GetRegisterInfoPrivate -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                out_register_info_private.set_var(amiibo.produce_register_info_private()?);
            },
            None => {
                return Err(nfp::rc::ResultDeviceNotFound::make());
            }
        }

        Ok(())
    }

    pub fn set_register_info_private(&mut self, device_handle: nfp::DeviceHandle, register_info_private: sf::InFixedPointerBuffer<nfp::RegisterInfoPrivate>) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] SetRegisterInfoPrivate -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                amiibo.update_from_register_info_private(register_info_private.get_var()?)?;
            },
            None => {
                return Err(nfp::rc::ResultDeviceNotFound::make());
            }
        }

        Ok(())
    }

    pub fn delete_register_info(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] DeleteRegisterInfo -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                amiibo.delete_all_areas()?;
            },
            None => {
                return Err(nfp::rc::ResultDeviceNotFound::make());
            }
        }
        Ok(())
    }

    pub fn delete_application_area(&mut self, device_handle: nfp::DeviceHandle) -> Result<()> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] DeleteApplicationArea -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_mut() {
            Some(amiibo) => {
                amiibo.delete_current_area()?;
            },
            None => {
                return Err(nfp::rc::ResultDeviceNotFound::make());
            }
        }

        Ok(())
    }

    pub fn exists_application_area(&mut self, device_handle: nfp::DeviceHandle) -> Result<bool> {
        result_return_unless!(self.is_state(nfp::State::Initialized), nfp::rc::ResultDeviceNotFound);
        result_return_unless!(self.is_device_state(nfp::DeviceState::TagMounted), nfp::rc::ResultDeviceNotFound);
        log!("[{:#X}] ExistsApplicationArea -- device_handle: (fake id: {})\n", self.application_id.0, device_handle.id);

        match emu::get_active_virtual_amiibo().as_ref() {
            Some(amiibo) => {
                Ok(amiibo.has_any_application_areas())
            },
            None => {
                Err(nfp::rc::ResultDeviceNotFound::make())
            }
        }
    }
}

impl Drop for EmulationHandler {
    fn drop(&mut self) {
        log!("[{:#X}] Dropping handler...\n", self.application_id.0);
        self.emulation_state.lock().should_end_thread = true;
        if let Some(thread_handle) = self.emu_handler_thread.take() {
            thread_handle.join().expect("We shouldn't expect any of the threads to panic. There isn't anything very interesting happening");
        }
    }
}

pub mod user;

pub mod sys;