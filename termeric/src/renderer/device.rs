//! Adapter and device selection, and the diagnostics they report.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererDeviceFailure {
    pub reason: DeviceLostReason,
    pub message: String,
}

pub(crate) type RendererFailureCallback =
    Arc<dyn Fn(RendererDeviceFailure) + Send + Sync + 'static>;

/// Stable adapter information included in startup and recovery diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererDiagnostics {
    pub preference: RendererBackend,
    pub adapter: String,
    pub backend: String,
    pub device_type: String,
    pub software: bool,
}

impl RendererDiagnostics {
    pub(crate) fn summary(&self) -> String {
        format!(
            "preference={} adapter={:?} backend={} device_type={} software={}",
            self.preference, self.adapter, self.backend, self.device_type, self.software
        )
    }
}

/// The adapter classes a backend preference tries, in order; `true` asks
/// for WGPU's software fallback.
pub(super) fn fallback_attempts(preference: RendererBackend) -> &'static [bool] {
    match preference {
        RendererBackend::Auto => &[false, true],
        RendererBackend::Gpu => &[false],
        RendererBackend::Software => &[true],
    }
}

pub(super) fn attempt_label(force_fallback: bool) -> &'static str {
    if force_fallback {
        "software fallback"
    } else {
        "hardware"
    }
}

/// `force_fallback_adapter: false` is a preference, not a guarantee: WGPU may
/// return a CPU adapter when no GPU is available. Reject that candidate so
/// `gpu` remains strict and `auto` advances to its explicit software attempt
/// instead of reporting a CPU adapter as hardware.
fn accepts_adapter(force_fallback: bool, device_type: DeviceType) -> bool {
    force_fallback || device_type != DeviceType::Cpu
}

/// One adapter attempt: the adapter and its device, or why it failed.
pub(super) fn request_adapter_device(
    instance: &Instance,
    backends: Backends,
    surface: &Surface<'_>,
    force_fallback: bool,
) -> std::result::Result<(Adapter, Device, Queue), String> {
    let adapter = discover_adapter(
        instance,
        backends,
        Some(surface),
        PowerPreference::HighPerformance,
        force_fallback,
    )
    .ok_or_else(|| "was unavailable".to_string())?;
    let adapter_info = adapter.get_info();
    if !accepts_adapter(force_fallback, adapter_info.device_type) {
        return Err(format!(
            "failed: WGPU returned CPU adapter {:?}; a GPU adapter is required",
            adapter_info.name
        ));
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &DeviceDescriptor {
            required_features: Features::empty(),
            required_limits: Limits::default(),
            memory_hints: MemoryHints::default(),
            label: Some("Termeric Device"),
        },
        None,
    ))
    .map_err(|error| format!("failed: could not create a device: {error}"))?;
    Ok((adapter, device, queue))
}

/// Ask WGPU for the requested adapter class and recover from backends that
/// enumerate a CPU adapter but do not return it for `force_fallback_adapter`.
/// The enumeration path is deliberately CPU-only so strict software mode can
/// never become an undocumented hardware selection.
pub(crate) fn discover_adapter(
    instance: &Instance,
    backends: Backends,
    compatible_surface: Option<&Surface<'_>>,
    power_preference: PowerPreference,
    force_fallback_adapter: bool,
) -> Option<Adapter> {
    let requested = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference,
        compatible_surface,
        force_fallback_adapter,
    }));
    if requested.is_some() || !force_fallback_adapter {
        return requested;
    }

    instance
        .enumerate_adapters(backends)
        .into_iter()
        .find(|adapter| {
            is_enumerated_software_candidate(
                adapter.get_info().device_type,
                compatible_surface.is_none_or(|surface| adapter.is_surface_supported(surface)),
            )
        })
}

fn is_enumerated_software_candidate(device_type: DeviceType, surface_supported: bool) -> bool {
    device_type == DeviceType::Cpu && surface_supported
}

#[cfg(test)]
mod backend_selection_tests {
    use super::*;

    #[test]
    fn automatic_selection_falls_back_but_forced_modes_do_not() {
        assert_eq!(fallback_attempts(RendererBackend::Auto), [false, true]);
        assert_eq!(fallback_attempts(RendererBackend::Gpu), [false]);
        assert_eq!(fallback_attempts(RendererBackend::Software), [true]);
        assert!(!accepts_adapter(false, DeviceType::Cpu));
        assert!(accepts_adapter(false, DeviceType::DiscreteGpu));
        assert!(accepts_adapter(true, DeviceType::Cpu));
        assert_eq!(attempt_label(false), "hardware");
        assert_eq!(attempt_label(true), "software fallback");
        assert!(is_enumerated_software_candidate(DeviceType::Cpu, true));
        assert!(!is_enumerated_software_candidate(
            DeviceType::DiscreteGpu,
            true
        ));
        assert!(!is_enumerated_software_candidate(DeviceType::Cpu, false));
    }

    #[test]
    fn diagnostics_identify_adapter_backend_type_and_software_state() {
        let diagnostics = RendererDiagnostics {
            preference: RendererBackend::Software,
            adapter: "llvmpipe".into(),
            backend: "Vulkan".into(),
            device_type: "Cpu".into(),
            software: true,
        };
        let summary = diagnostics.summary();
        assert!(summary.contains("preference=software"));
        assert!(summary.contains("adapter=\"llvmpipe\""));
        assert!(summary.contains("backend=Vulkan"));
        assert!(summary.contains("device_type=Cpu"));
        assert!(summary.contains("software=true"));
    }
}
