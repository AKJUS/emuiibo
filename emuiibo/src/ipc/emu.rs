use crate::amiibo;
use crate::amiibo::VirtualAmiiboFormat;
use crate::emu;
use crate::rc;
use alloc::string::ToString;
use nx::ipc::server;
use nx::ipc::sf;
use nx::ipc::sf::ncm;
use nx::ipc::sf::nfp;
use nx::ipc::sf::sm;
use nx::result::*;
use nx::service;
use nx::version;

ipc_sf_define_default_client_for_interface!(EmulationService);
ipc_sf_define_interface_trait! {
    trait EmulationService {
        get_version [0, version::VersionInterval::all()]: () => (version: emu::Version) (version: emu::Version);
        get_virtual_amiibo_directory [1, version::VersionInterval::all()]: (out_path: sf::OutMapAliasBuffer<u8>) => () ();
        get_emulation_status [2, version::VersionInterval::all()]: () => (status: emu::EmulationStatus) (status: emu::EmulationStatus);
        set_emulation_status [3, version::VersionInterval::all()]: (status: emu::EmulationStatus) => () ();
        get_active_virtual_amiibo [4, version::VersionInterval::all()]: (out_path: sf::OutMapAliasBuffer<u8>) => (virtual_amiibo: amiibo::fmt::VirtualAmiiboData) (virtual_amiibo: amiibo::fmt::VirtualAmiiboData);
        set_active_virtual_amiibo [5, version::VersionInterval::all()]: (path: sf::InMapAliasBuffer<u8>) => () ();
        reset_active_virtual_amiibo [6, version::VersionInterval::all()]: () => () ();
        get_active_virtual_amiibo_status [7, version::VersionInterval::all()]: () => (status: emu::VirtualAmiiboStatus) (status: emu::VirtualAmiiboStatus);
        set_active_virtual_amiibo_status [8, version::VersionInterval::all()]: (status: emu::VirtualAmiiboStatus) => () ();
        is_application_id_intercepted [9, version::VersionInterval::all()]: (application_id: ncm::ProgramId) => (is_intercepted: bool) (is_intercepted: bool);
        try_parse_virtual_amiibo [10, version::VersionInterval::all()]: (path: sf::InMapAliasBuffer<u8>) => (virtual_amiibo: amiibo::fmt::VirtualAmiiboData) (virtual_amiibo: amiibo::fmt::VirtualAmiiboData);
        get_active_virtual_amiibo_areas [11, version::VersionInterval::all()]: (out_areas: sf::OutMapAliasBuffer<amiibo::fmt::VirtualAmiiboAreaEntry>) => (count: u32) (count: u32);
        get_active_virtual_amiibo_current_area [12, version::VersionInterval::all()]: () => (access_id: nfp::AccessId) (access_id: nfp::AccessId);
        set_active_virtual_amiibo_current_area [13, version::VersionInterval::all()]: (access_id: nfp::AccessId) => () ();
        set_active_virtual_amiibo_uuid_info [14, version::VersionInterval::all()]: (uuid_info: amiibo::fmt::VirtualAmiiboUuidInfo) => () ();
    }
}

pub struct EmulationServer;

impl IEmulationServiceServer for EmulationServer {
    fn get_version(&mut self) -> Result<emu::Version> {
        log!("GetVersion -- (...)\n");
        Ok(emu::CURRENT_VERSION)
    }

    fn get_virtual_amiibo_directory(
        &mut self,
        mut out_path: sf::OutMapAliasBuffer<u8>,
    ) -> Result<()> {
        log!("GetVirtualAmiiboDirectory -- (...)\n");
        out_path.set_string(amiibo::VIRTUAL_AMIIBO_DIR.to_string());
        Ok(())
    }

    fn get_emulation_status(&mut self) -> Result<emu::EmulationStatus> {
        log!("GetEmulationStatus -- (...)\n");
        let status = emu::get_emulation_status();
        Ok(status)
    }

    fn set_emulation_status(&mut self, status: emu::EmulationStatus) -> Result<()> {
        log!("SetEmulationStatus -- status: {:?}\n", status);
        emu::set_emulation_status(status);
        Ok(())
    }

    fn get_active_virtual_amiibo(
        &mut self,
        mut out_path: sf::OutMapAliasBuffer<u8>,
    ) -> Result<amiibo::fmt::VirtualAmiiboData> {
        log!("GetActiveVirtualAmiibo -- (...)\n");
        let amiibo = emu::get_active_virtual_amiibo();

        let amiibo = amiibo
            .as_ref()
            .ok_or(rc::ResultInvalidActiveVirtualAmiibo::make())?;

        let data = amiibo.produce_data()?;
        out_path.set_string(amiibo.path.clone());
        Ok(data)
    }

    fn set_active_virtual_amiibo(&mut self, path: sf::InMapAliasBuffer<u8>) -> Result<()> {
        let path_str = path.get_string();
        log!("SetActiveVirtualAmiibo -- path: '{}'\n", path_str);
        let amiibo = amiibo::fmt::VirtualAmiibo::try_load(path_str)?;
        result_return_unless!(amiibo.is_valid(), rc::ResultInvalidLoadedVirtualAmiibo);

        emu::set_active_virtual_amiibo(Some(amiibo));
        Ok(())
    }

    fn reset_active_virtual_amiibo(&mut self) -> Result<()> {
        log!("ResetActiveVirtualAmiibo -- (...)\n");
        emu::set_active_virtual_amiibo(None);
        Ok(())
    }

    fn get_active_virtual_amiibo_status(&mut self) -> Result<emu::VirtualAmiiboStatus> {
        log!("GetActiveVirtualAmiiboStatus -- (...)\n");
        let status = emu::get_active_virtual_amiibo_status();
        Ok(status)
    }

    fn set_active_virtual_amiibo_status(&mut self, status: emu::VirtualAmiiboStatus) -> Result<()> {
        log!("SetActiveVirtualAmiiboStatus -- status: {:?}\n", status);
        emu::set_active_virtual_amiibo_status(status);
        Ok(())
    }

    fn is_application_id_intercepted(&mut self, application_id: ncm::ProgramId) -> Result<bool> {
        log!(
            "IsApplicationIdIntercepted -- app_id: {:#X}\n",
            application_id.0
        );
        Ok(emu::is_application_id_intercepted(application_id))
    }

    fn try_parse_virtual_amiibo(
        &mut self,
        path: sf::InMapAliasBuffer<u8>,
    ) -> Result<amiibo::fmt::VirtualAmiiboData> {
        let path_str = path.get_string();
        log!("TryParseVirtualAmiibo -- path: '{}'\n", path_str);
        let amiibo = amiibo::fmt::VirtualAmiibo::try_load(path_str)?;
        result_return_unless!(amiibo.is_valid(), rc::ResultInvalidLoadedVirtualAmiibo);

        let data = amiibo.produce_data()?;
        Ok(data)
    }

    fn get_active_virtual_amiibo_areas(
        &mut self,
        mut out_areas: sf::OutMapAliasBuffer<amiibo::fmt::VirtualAmiiboAreaEntry>,
    ) -> Result<u32> {
        log!("GetActiveVirtualAmiiboAreas -- (...)\n");
        let amiibo = emu::get_active_virtual_amiibo();

        let amiibo = amiibo
            .as_ref()
            .ok_or(rc::ResultInvalidActiveVirtualAmiibo::make())?;

        let areas = out_areas.as_maybeuninit_mut()?;

        let count = areas.len().min(amiibo.areas.areas.len());
        for i in 0..count {
            areas[i].write(amiibo.areas.areas[i]);
        }

        Ok(count as u32)
    }

    fn get_active_virtual_amiibo_current_area(&mut self) -> Result<nfp::AccessId> {
        log!("GetActiveVirtualAmiiboCurrentArea -- (...)\n");
        let amiibo = emu::get_active_virtual_amiibo();

        Ok(amiibo
            .as_ref()
            .ok_or(rc::ResultInvalidActiveVirtualAmiibo::make())?
            .get_current_area()
            .ok_or(rc::ResultInvalidVirtualAmiiboAccessId::make())?
            .access_id)
    }

    fn set_active_virtual_amiibo_current_area(&mut self, access_id: nfp::AccessId) -> Result<()> {
        log!(
            "SetActiveVirtualAmiiboCurrentArea -- access_id: {:#X}\n",
            access_id
        );
        let mut amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_some(), rc::ResultInvalidActiveVirtualAmiibo);

        if amiibo.as_mut().unwrap().set_current_area(access_id) {
            Ok(())
        } else {
            Err(rc::ResultInvalidVirtualAmiiboAccessId::make())
        }
    }

    fn set_active_virtual_amiibo_uuid_info(
        &mut self,
        uuid_info: amiibo::fmt::VirtualAmiiboUuidInfo,
    ) -> Result<()> {
        log!(
            "SetActiveVirtualAmiiboUuidInfo -- uuid_info: {:?}\n",
            uuid_info
        );
        let mut amiibo = emu::get_active_virtual_amiibo();
        result_return_unless!(amiibo.is_some(), rc::ResultInvalidActiveVirtualAmiibo);

        amiibo.as_mut().unwrap().set_uuid_info(uuid_info)
    }
}

impl server::ISessionObject for EmulationServer {
    fn try_handle_request_by_id(
        &mut self,
        req_id: u32,
        protocol: nx::ipc::CommandProtocol,
        server_ctx: &mut server::ServerContext,
    ) -> Option<Result<()>> {
        <Self as IEmulationServiceServer>::try_handle_request_by_id(
            self, req_id, protocol, server_ctx,
        )
    }
}

impl server::IServerObject for EmulationServer {
    fn new() -> Self {
        Self
    }
}

impl server::IService for EmulationServer {
    fn get_name() -> sm::ServiceName {
        sm::ServiceName::new("emuiibo")
    }

    fn get_max_sesssions() -> i32 {
        20
    }
}

impl service::IService for EmulationService {
    fn get_name() -> sm::ServiceName {
        sm::ServiceName::new("emuiibo")
    }

    fn as_domain() -> bool {
        false
    }

    fn post_initialize(&mut self) -> Result<()> {
        Ok(())
    }
}
