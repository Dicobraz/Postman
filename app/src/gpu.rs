//! Выбор графического API и адаптера. Рендер только на аппаратном GPU:
//! программные адаптеры (WARP, llvmpipe, lavapipe) отвергаются, если явно не
//! разрешены. Vulkan — только по запросу: на Intel он даёт чёрный кадр при ресайзе.

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu::{self, Adapter, DeviceType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Api {
    Dx12,
    Vulkan,
    Gl,
}

impl Api {
    /// `--gpu dx12|vulkan|gl` или `POSTMAN_GPU`; по умолчанию DX12 на Windows, GL на Linux.
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let arg = args.iter().position(|a| a == "--gpu").and_then(|i| args.get(i + 1)).cloned();
        let Some(name) = arg.or_else(|| std::env::var("POSTMAN_GPU").ok()) else {
            return Ok(if cfg!(windows) { Api::Dx12 } else { Api::Gl });
        };
        match name.to_ascii_lowercase().as_str() {
            "dx12" => Ok(Api::Dx12),
            "vulkan" | "vk" => Ok(Api::Vulkan),
            "gl" | "gles" | "opengl" => Ok(Api::Gl),
            other => Err(format!("неизвестный GPU API «{other}»: dx12 | vulkan | gl")),
        }
    }

    fn backends(self) -> wgpu::Backends {
        match self {
            Api::Dx12 => wgpu::Backends::DX12,
            Api::Vulkan => wgpu::Backends::VULKAN,
            Api::Gl => wgpu::Backends::GL,
        }
    }
}

/// `allow_software` — для CI/контейнеров без GPU (`POSTMAN_GPU_SOFTWARE=1`).
pub fn configuration(api: Api, allow_software: bool, no_vsync: bool) -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = api.backends();
    setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| pick(adapters, surface, allow_software)));
    let mut config = WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() };
    if no_vsync {
        config.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    config
}

/// Встроенная карта предпочтительнее дискретной: почтовому клиенту хватает,
/// а ноутбук не будит вторую видеокарту.
fn pick(adapters: &[Adapter], surface: Option<&wgpu::Surface<'_>>, allow_software: bool) -> Result<Adapter, String> {
    let rank = |t: DeviceType| match t {
        DeviceType::IntegratedGpu => Some(0),
        DeviceType::DiscreteGpu => Some(1),
        DeviceType::VirtualGpu | DeviceType::Other => Some(2),
        DeviceType::Cpu => allow_software.then_some(3),
    };
    adapters
        .iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .filter_map(|a| Some((rank(a.get_info().device_type)?, a)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, a)| a.clone())
        .ok_or_else(|| {
            let found: Vec<String> =
                adapters.iter().map(|a| format!("{} ({:?})", a.get_info().name, a.get_info().device_type)).collect();
            format!("нет аппаратного GPU для выбранного API; найдены: [{}]", found.join(", "))
        })
}
