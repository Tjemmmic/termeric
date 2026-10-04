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
    /// What the selected adapter reports. It counts as software when the
    /// software fallback was asked for, whatever WGPU calls the adapter, or
    /// when the adapter is a CPU one.
    pub(super) fn new(
        preference: RendererBackend,
        adapter: &AdapterInfo,
        fallback_requested: bool,
    ) -> Self {
        Self {
            preference,
            adapter: adapter.name.clone(),
            backend: format!("{:?}", adapter.backend),
            device_type: format!("{:?}", adapter.device_type),
            software: fallback_requested || adapter.device_type == DeviceType::Cpu,
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "preference={} adapter={:?} backend={} device_type={} software={}",
            self.preference, self.adapter, self.backend, self.device_type, self.software
        )
    }
}

/// The adapter classes a backend preference tries, in order; `true` asks
/// for WGPU's software fallback.
fn fallback_attempts(preference: RendererBackend) -> &'static [bool] {
    match preference {
        RendererBackend::Auto => &[false, true],
        RendererBackend::Gpu => &[false],
        RendererBackend::Software => &[true],
    }
}

fn attempt_label(force_fallback: bool) -> &'static str {
    if force_fallback {
        "software fallback"
    } else {
        "hardware"
    }
}

/// Try the adapter classes `preference` allows, in order, and keep the first
/// that `attempt` (`request_adapter_device` outside tests) turns into a
/// device, with whether it was the software fallback. A failed attempt,
/// including one whose adapter could not create a device, moves on to the
/// next class; when none is left, the error names every attempt and why it
/// failed, and says how to choose software rendering.
pub(super) fn select_adapter<T>(
    preference: RendererBackend,
    mut attempt: impl FnMut(bool) -> std::result::Result<T, String>,
) -> Result<(T, bool)> {
    let mut failures = Vec::new();
    for &force_fallback in fallback_attempts(preference) {
        match attempt(force_fallback) {
            Ok(found) => return Ok((found, force_fallback)),
            Err(reason) => failures.push(format!(
                "{} adapter {reason}",
                attempt_label(force_fallback)
            )),
        }
    }
    anyhow::bail!(
        "renderer initialization failed with preference {preference}: {}. \
         Try `termeric --renderer software`, install current graphics/Mesa drivers, \
         or set renderer.backend = \"software\" in config.toml",
        failures.join("; ")
    )
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

    /// Run `select_adapter` with scripted attempt results, returning what it
    /// chose or its error, and the attempts it made.
    fn select_with(
        preference: RendererBackend,
        results: &[(bool, std::result::Result<&'static str, &'static str>)],
    ) -> (std::result::Result<(&'static str, bool), String>, Vec<bool>) {
        let mut attempts = Vec::new();
        let selected = select_adapter(preference, |force_fallback| {
            attempts.push(force_fallback);
            results
                .iter()
                .find(|(fallback, _)| *fallback == force_fallback)
                .expect("an attempt the preference does not allow")
                .1
                .map_err(str::to_string)
        })
        .map_err(|error| error.to_string());
        (selected, attempts)
    }

    #[test]
    fn selection_falls_back_after_a_failed_hardware_attempt() {
        let device_failure = "failed: could not create a device: out of memory";
        let (selected, attempts) = select_with(
            RendererBackend::Auto,
            &[(false, Err(device_failure)), (true, Ok("llvmpipe"))],
        );
        assert_eq!(selected, Ok(("llvmpipe", true)));
        assert_eq!(attempts, [false, true]);
    }

    #[test]
    fn selection_stops_at_the_first_success() {
        let (selected, attempts) = select_with(
            RendererBackend::Auto,
            &[(false, Ok("gpu")), (true, Ok("llvmpipe"))],
        );
        assert_eq!(selected, Ok(("gpu", false)));
        assert_eq!(attempts, [false]);
    }

    #[test]
    fn selection_failure_names_every_attempt_and_the_way_out() {
        let (selected, attempts) = select_with(
            RendererBackend::Auto,
            &[
                (false, Err("was unavailable")),
                (true, Err("failed: could not create a device: lost")),
            ],
        );
        let error = selected.unwrap_err();
        assert_eq!(attempts, [false, true]);
        assert!(
            error.starts_with(
                "renderer initialization failed with preference auto: hardware adapter was \
                 unavailable; software fallback adapter failed: could not create a device: lost. "
            ),
            "{error}"
        );
        assert!(error.contains("termeric --renderer software"), "{error}");

        // A strict preference makes its one attempt and never falls back.
        let (selected, attempts) =
            select_with(RendererBackend::Gpu, &[(false, Err("was unavailable"))]);
        assert!(
            selected
                .unwrap_err()
                .contains("preference gpu: hardware adapter was unavailable.")
        );
        assert_eq!(attempts, [false]);
    }

    #[test]
    fn diagnostics_count_a_requested_fallback_or_cpu_adapter_as_software() {
        let adapter = |device_type| AdapterInfo {
            name: "adapter".into(),
            vendor: 0,
            device: 0,
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend: Backend::Vulkan,
        };
        let software = |device_type, fallback_requested| {
            RendererDiagnostics::new(
                RendererBackend::Auto,
                &adapter(device_type),
                fallback_requested,
            )
            .software
        };
        assert!(!software(DeviceType::DiscreteGpu, false));
        assert!(software(DeviceType::DiscreteGpu, true));
        assert!(software(DeviceType::Cpu, false));
        let diagnostics =
            RendererDiagnostics::new(RendererBackend::Auto, &adapter(DeviceType::Cpu), false);
        assert_eq!(diagnostics.backend, "Vulkan");
        assert_eq!(diagnostics.device_type, "Cpu");
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
