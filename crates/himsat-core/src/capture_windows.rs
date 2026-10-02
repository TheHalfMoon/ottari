//! 008A Windows microphone adapter core over an injected audio backend.
//!
//! Portable selection, identity, and fault-classification logic for the
//! Windows microphone pathway, plus the closed cpal/WASAPI binding
//! behind `cfg(target_os = "windows")`. The portable core drives the
//! closed 006A session machine without ever producing a refused
//! transition and maps every Windows fault class onto the portable
//! 006A/006B surface; because that core is portable it compiles and is
//! tested on every CI host, so the mapping stays proven independently of
//! the OS binding, which is exercised only on Windows.
//!
//! Windows differs materially from the macOS pathway: endpoint
//! identity is an OS endpoint identifier rather than a display name,
//! the microphone-privacy decision is a separate OS capability state,
//! and a shared/exclusive-mode conflict arrives as a distinct "device
//! busy" condition. This adapter therefore owns its own portable core
//! instead of importing 007's; the shared 006A/006B/006C contracts are
//! consumed unchanged and nothing here narrows them.

use himsat_events::{SessionId, SourceId};

use crate::capture_session::{CaptureEvent, HealthReason, SourceDescriptor, SourceKind};

/// Maximum display-label length in characters. Device names are
/// OS-provided and unbounded; descriptors stay bounded.
pub const MAX_LABEL_CHARS: usize = 128;

/// Fallback label when a device reports an empty name.
pub const UNNAMED_MICROPHONE_LABEL: &str = "Unnamed microphone";

/// Domain tag folded into stable source ids so Windows microphone ids
/// never collide with ids derived for other platforms or source kinds.
const SOURCE_ID_DOMAIN: &[u8] = b"himsat-008a-microphone";

/// Portable sample format of a device configuration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SampleFormat {
    /// 32-bit float samples (the only format this grain captures).
    F32,
    /// 16-bit signed integer samples.
    I16,
    /// 16-bit unsigned integer samples.
    U16,
}

/// Device configuration resolved from a backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DeviceConfig {
    /// Channel count of the configuration.
    pub channels: u16,
    /// Sample rate in hertz.
    pub sample_rate_hz: u32,
    /// Sample format of the configuration.
    pub format: SampleFormat,
}

/// Input device surfaced by a backend.
///
/// `device_id` is the OS endpoint identifier (on Windows the WASAPI
/// endpoint id string), not a display name: it is what the adapter
/// addresses and what stable 003 source identity derives from. The
/// endpoint id is stable across process restarts and reboots for a
/// given endpoint, but Windows may issue a new id when an endpoint is
/// re-enumerated or reinstalled, so this is not permanence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceInfo {
    /// OS endpoint identifier.
    pub device_id: String,
    /// OS-reported device name (unbounded, sanitized at use).
    pub name: String,
    /// Whether the OS marks this device as the default input.
    pub is_default: bool,
    /// Preferred configuration, or `None` when the endpoint's default
    /// input format is outside the modelled set. `None` is honest: the
    /// adapter refuses capture on such an endpoint rather than silently
    /// converting or mislabelling the format.
    pub preferred_config: Option<DeviceConfig>,
}

/// Stage at which stream handling failed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StreamStage {
    /// The stream could not be constructed.
    Build,
    /// A constructed stream would not start, or stopped while flowing.
    Play,
}

/// Adapter fault. OS detail travels with telemetry; the machine only
/// ever sees the classified outcome from the mapping functions below.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowsMicError {
    /// No usable input device exists right now.
    NoInputDevices,
    /// A device exists but its stream failed at the given stage.
    StreamFault(StreamStage),
    /// WASAPI reported a non-terminal buffer discontinuity (`Xrun`).
    /// The stream keeps flowing; callers record it in loss accounting
    /// rather than falsely treating it as a route failure.
    DataDiscontinuity,
    /// The OS denied or revoked microphone access (Windows microphone
    /// privacy). Distinct from absence: the endpoint exists but policy
    /// forbids capture.
    PermissionDenied,
    /// The endpoint is held by another client in a mode that blocks
    /// this capture (WASAPI `AUDCLNT_E_DEVICE_IN_USE`, shared/exclusive
    /// conflict).
    ExclusiveModeConflict,
    /// The endpoint disappeared while selected (disconnect, removal,
    /// invalidated endpoint).
    EndpointUnavailable,
    /// The Windows audio service is not running.
    AudioServiceUnavailable,
    /// The OS rerouted the stream to another endpoint. cpal documents
    /// that such a stream stays active and needs no rebuild, so this is
    /// telemetry detail and never an interrupting fault.
    RouteRerouted,
}

impl WindowsMicError {
    /// Short stable classifier for telemetry detail strings.
    #[must_use]
    pub const fn classifier(&self) -> &'static str {
        match self {
            Self::NoInputDevices => "no-input-devices",
            Self::StreamFault(StreamStage::Build) => "stream-build-fault",
            Self::StreamFault(StreamStage::Play) => "stream-play-fault",
            Self::DataDiscontinuity => "data-discontinuity",
            Self::PermissionDenied => "permission-denied",
            Self::ExclusiveModeConflict => "exclusive-mode-conflict",
            Self::EndpointUnavailable => "endpoint-unavailable",
            Self::AudioServiceUnavailable => "audio-service-unavailable",
            Self::RouteRerouted => "route-rerouted",
        }
    }
}

/// Backend contract the OS binding implements.
pub trait MicrophoneBackend {
    /// Lists currently usable input devices, each with its endpoint id.
    /// A backend must never report an empty endpoint id: identity derives
    /// from it, so empty ids would collide into one source. The OS
    /// binding skips any endpoint whose id it cannot resolve.
    fn input_devices(&self) -> Result<Vec<InputDeviceInfo>, WindowsMicError>;

    /// OS endpoint id of the default input, when the OS names one.
    fn default_input_id(&self) -> Option<String>;
}

/// Microphone selected for a session: resolved device plus its stable
/// 003 source identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedInput {
    /// Resolved device description.
    pub info: InputDeviceInfo,
    /// Stable source id derived from the endpoint id.
    pub source_id: SourceId,
}

impl SelectedInput {
    /// Binds the selection to a session as a microphone descriptor.
    /// The label is the sanitized OS device name: exactly what device
    /// pickers already show the user, nothing more (XIV minimization).
    #[must_use]
    pub fn descriptor(&self, session_id: SessionId) -> SourceDescriptor {
        SourceDescriptor::new(
            self.source_id,
            session_id,
            SourceKind::Microphone,
            sanitize_label(&self.info.name),
        )
    }
}

/// Selects the preferred device by exact endpoint-id match, then by
/// exact name match, then the OS default endpoint, then the device the
/// backend flags as default, then the first enumerated device. Empty
/// enumeration is `NoInputDevices`, never a panic and never a guessed
/// device.
pub fn select_input(
    backend: &dyn MicrophoneBackend,
    preferred: Option<&str>,
) -> Result<SelectedInput, WindowsMicError> {
    let devices = backend.input_devices()?;
    if devices.is_empty() {
        return Err(WindowsMicError::NoInputDevices);
    }
    let default_id = backend.default_input_id();
    let info = preferred
        .and_then(|want| devices.iter().find(|info| info.device_id == want))
        .or_else(|| preferred.and_then(|want| devices.iter().find(|info| info.name == want)))
        .or_else(|| {
            default_id
                .as_deref()
                .and_then(|want| devices.iter().find(|info| info.device_id == want))
        })
        .or_else(|| devices.iter().find(|info| info.is_default))
        .or(devices.first());
    match info {
        Some(info) => Ok(SelectedInput {
            info: info.clone(),
            source_id: stable_source_id(&info.device_id),
        }),
        None => Err(WindowsMicError::NoInputDevices),
    }
}

/// Classifies a start-time failure (session is `Preparing`) into a
/// machine event the `Preparing` state accepts. Best-fit portable
/// reasons; the exact OS condition stays in telemetry detail.
#[must_use]
pub const fn event_for_start_failure(error: &WindowsMicError) -> CaptureEvent {
    match error {
        WindowsMicError::NoInputDevices => {
            CaptureEvent::FailRecoverable(HealthReason::SourceSilent)
        }
        WindowsMicError::PermissionDenied => {
            CaptureEvent::FailRecoverable(HealthReason::PermissionRevoked)
        }
        // Stream faults, endpoint loss, exclusive-mode conflict, and an
        // unavailable audio service are route/availability failures at
        // start. `RouteRerouted` cannot occur at start (the backend
        // emits it only for a live stream); it maps defensively so this
        // function stays total and never strands `Preparing`.
        _ => CaptureEvent::FailRecoverable(HealthReason::RouteChanged),
    }
}

/// Classifies a runtime fault (session is `RecordingHealthy` or
/// `RecordingDegraded`) into a machine event those states accept.
/// `None` means "record the telemetry detail and do not move the
/// session machine": a rerouted stream keeps flowing, so interrupting
/// it would be a false stop.
#[must_use]
pub const fn event_for_runtime_fault(error: &WindowsMicError) -> Option<CaptureEvent> {
    match error {
        WindowsMicError::RouteRerouted | WindowsMicError::DataDiscontinuity => None,
        WindowsMicError::PermissionDenied => {
            Some(CaptureEvent::Interrupted(HealthReason::PermissionRevoked))
        }
        _ => Some(CaptureEvent::Interrupted(HealthReason::RouteChanged)),
    }
}

/// Portable 006B degradation classification for a runtime fault, or
/// `None` when there is no degradation to report. Kept consistent with
/// [`event_for_runtime_fault`] by a test rather than by convention.
#[must_use]
pub const fn health_reason_for(error: &WindowsMicError) -> Option<HealthReason> {
    match error {
        WindowsMicError::RouteRerouted | WindowsMicError::DataDiscontinuity => None,
        WindowsMicError::PermissionDenied => Some(HealthReason::PermissionRevoked),
        _ => Some(HealthReason::RouteChanged),
    }
}

/// Derives a stable 003 source id from an OS endpoint id. Deterministic
/// across processes (dual FNV-1a64 over domain tag plus id); equal ids
/// map to equal ids, distinct ids overwhelmingly differ.
#[must_use]
pub fn stable_source_id(device_id: &str) -> SourceId {
    let high = u128::from(fnv1a64(0x00, device_id));
    let low = u128::from(fnv1a64(0x01, device_id));
    SourceId::new(high << 64 | low)
}

/// 64-bit FNV-1a over the domain tag, one lane byte, and the value.
fn fnv1a64(lane: u8, value: &str) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in SOURCE_ID_DOMAIN
        .iter()
        .chain(std::slice::from_ref(&lane))
        .chain(value.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Sanitizes an OS device name into a bounded descriptor label:
/// trims surrounding whitespace, falls back for empty names, and
/// truncates to [`MAX_LABEL_CHARS`] characters on a char boundary.
#[must_use]
pub fn sanitize_label(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return UNNAMED_MICROPHONE_LABEL.to_owned();
    }
    if trimmed.chars().count() <= MAX_LABEL_CHARS {
        return trimmed.to_owned();
    }
    let end = trimmed
        .char_indices()
        .nth(MAX_LABEL_CHARS)
        .map_or(trimmed.len(), |(index, _)| index);
    trimmed[..end].to_owned()
}

/// In-memory backend for tests and for hosts without audio hardware.
/// Never touches the OS; failures are injected explicitly.
#[derive(Clone, Debug, Default)]
pub struct MockMicrophoneBackend {
    devices: Vec<InputDeviceInfo>,
    default_id: Option<String>,
    failure: Option<WindowsMicError>,
}

impl MockMicrophoneBackend {
    /// Backend with the given devices and optional default endpoint id.
    #[must_use]
    pub fn with_devices(devices: Vec<InputDeviceInfo>, default_id: Option<String>) -> Self {
        Self {
            devices,
            default_id,
            failure: None,
        }
    }

    /// Backend whose enumeration always fails with the given fault.
    #[must_use]
    pub const fn failing(failure: WindowsMicError) -> Self {
        Self {
            devices: Vec::new(),
            default_id: None,
            failure: Some(failure),
        }
    }
}

impl MicrophoneBackend for MockMicrophoneBackend {
    fn input_devices(&self) -> Result<Vec<InputDeviceInfo>, WindowsMicError> {
        match &self.failure {
            Some(failure) => Err(failure.clone()),
            None => Ok(self.devices.clone()),
        }
    }

    fn default_input_id(&self) -> Option<String> {
        self.default_id.clone()
    }
}

/// Builds a device description for tests and callers without a live
/// backend.
#[must_use]
pub fn device_info(
    device_id: &str,
    name: &str,
    is_default: bool,
    channels: u16,
    sample_rate_hz: u32,
) -> InputDeviceInfo {
    InputDeviceInfo {
        device_id: device_id.to_owned(),
        name: name.to_owned(),
        is_default,
        preferred_config: Some(DeviceConfig {
            channels,
            sample_rate_hz,
            format: SampleFormat::F32,
        }),
    }
}

/// cpal trait imports for the 008A OS binding (Windows only).
#[cfg(target_os = "windows")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Classifies a cpal error kind plus the stage it occurred in. Pure, so
/// it is unit-tested on the Windows CI host without audio hardware;
/// every Windows-specific classifier difference is recorded here.
#[cfg(target_os = "windows")]
#[must_use]
pub fn classify_error(kind: cpal::ErrorKind, stage: StreamStage) -> WindowsMicError {
    match kind {
        cpal::ErrorKind::PermissionDenied => WindowsMicError::PermissionDenied,
        cpal::ErrorKind::DeviceBusy => WindowsMicError::ExclusiveModeConflict,
        cpal::ErrorKind::DeviceNotAvailable => WindowsMicError::EndpointUnavailable,
        cpal::ErrorKind::HostUnavailable => WindowsMicError::AudioServiceUnavailable,
        cpal::ErrorKind::DeviceChanged => WindowsMicError::RouteRerouted,
        cpal::ErrorKind::Xrun => WindowsMicError::DataDiscontinuity,
        // Format negotiation, resource exhaustion, stream invalidation,
        // and backend-specific failures stay stream faults at the
        // stage they occurred; the OS detail keeps the distinction.
        _ => WindowsMicError::StreamFault(stage),
    }
}

/// Maps a cpal sample format into the modelled portable set. `None`
/// means the format is not one this grain captures, so the endpoint is
/// reported without a preferred configuration and capture is refused
/// there rather than silently converted.
#[cfg(target_os = "windows")]
#[must_use]
pub const fn portable_sample_format(format: cpal::SampleFormat) -> Option<SampleFormat> {
    match format {
        cpal::SampleFormat::F32 => Some(SampleFormat::F32),
        cpal::SampleFormat::I16 => Some(SampleFormat::I16),
        cpal::SampleFormat::U16 => Some(SampleFormat::U16),
        _ => None,
    }
}

/// Reads a cpal supported configuration into the portable form.
#[cfg(target_os = "windows")]
fn config_from_supported(supported: &cpal::SupportedStreamConfig) -> Option<DeviceConfig> {
    let format = portable_sample_format(supported.sample_format())?;
    let config = supported.config();
    Some(DeviceConfig {
        channels: config.channels,
        sample_rate_hz: config.sample_rate,
        format,
    })
}

/// cpal-backed implementation of [`MicrophoneBackend`]: the 008A OS
/// binding. Enumeration and default queries only probe; nothing streams
/// until [`open_f32_input_stream`] builds and plays.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default)]
pub struct CpalMicrophoneBackend;

/// Resolves the OS default input endpoint id, if the OS names one.
#[cfg(target_os = "windows")]
fn cpal_default_id(host: &cpal::Host) -> Option<String> {
    host.default_input_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.id().to_owned())
}

/// Reads one enumerated device. Endpoints whose id cannot be resolved
/// are skipped: without a stable id the adapter cannot address the
/// endpoint or derive honest identity, and a name-keyed fallback would
/// silently merge distinct endpoints that share a display name.
#[cfg(target_os = "windows")]
fn cpal_device_info(device: &cpal::Device, default_id: Option<&str>) -> Option<InputDeviceInfo> {
    let device_id = device.id().ok()?.id().to_owned();
    let name = device
        .description()
        .map(|description| description.name().to_owned())
        .unwrap_or_default();
    let preferred_config = device
        .default_input_config()
        .ok()
        .as_ref()
        .and_then(config_from_supported);
    Some(InputDeviceInfo {
        is_default: default_id == Some(device_id.as_str()),
        device_id,
        name,
        preferred_config,
    })
}

#[cfg(target_os = "windows")]
impl MicrophoneBackend for CpalMicrophoneBackend {
    fn input_devices(&self) -> Result<Vec<InputDeviceInfo>, WindowsMicError> {
        let host = cpal::default_host();
        let default_id = cpal_default_id(&host);
        let devices = host
            .input_devices()
            .map_err(|error| classify_error(error.kind(), StreamStage::Build))?;
        let mut out = Vec::new();
        for device in devices {
            if let Some(info) = cpal_device_info(&device, default_id.as_deref()) {
                out.push(info);
            }
        }
        Ok(out)
    }

    fn default_input_id(&self) -> Option<String> {
        cpal_default_id(&cpal::default_host())
    }
}

/// Live cpal input stream. Dropping stops capture: callers must drive
/// `StopRequested`/`StopCompleted` through the session machine first so
/// a drop is never a silent stop.
#[cfg(target_os = "windows")]
pub struct LiveMicStream {
    stream: cpal::Stream,
    device_id: String,
    config: DeviceConfig,
}

#[cfg(target_os = "windows")]
impl LiveMicStream {
    /// OS endpoint id of the streaming device.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Configuration the stream was built with.
    #[must_use]
    pub const fn config(&self) -> DeviceConfig {
        self.config
    }

    /// Pauses frame delivery without tearing down, for the
    /// `Interrupted` state. Failures classify as play-stage faults.
    pub fn pause(&self) -> Result<(), WindowsMicError> {
        self.stream
            .pause()
            .map_err(|error| classify_error(error.kind(), StreamStage::Play))
    }

    /// Resumes a paused stream, for the `Recovering` state.
    pub fn resume(&self) -> Result<(), WindowsMicError> {
        self.stream
            .play()
            .map_err(|error| classify_error(error.kind(), StreamStage::Play))
    }
}

/// Opens an F32 input stream on the named endpoint and starts it
/// flowing. Frames arrive on the cpal audio thread via `on_frames`;
/// faults arrive via `on_error` already classified, with the OS detail
/// string for telemetry. Non-F32 default configurations are refused as
/// build faults (format negotiation widens only with Gate E evidence,
/// never by silent conversion).
#[cfg(target_os = "windows")]
pub fn open_f32_input_stream(
    device_id: &str,
    mut on_frames: impl FnMut(&[f32]) + Send + 'static,
    mut on_error: impl FnMut(WindowsMicError, String) + Send + 'static,
) -> Result<LiveMicStream, WindowsMicError> {
    let host = cpal::default_host();
    let mut found = host
        .input_devices()
        .map_err(|error| classify_error(error.kind(), StreamStage::Build))?;
    let device = found
        .find(|candidate| candidate.id().is_ok_and(|id| id.id() == device_id))
        .ok_or(WindowsMicError::EndpointUnavailable)?;
    let supported = device
        .default_input_config()
        .map_err(|error| classify_error(error.kind(), StreamStage::Build))?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(WindowsMicError::StreamFault(StreamStage::Build));
    }
    let config = config_from_supported(&supported)
        .ok_or(WindowsMicError::StreamFault(StreamStage::Build))?;
    let stream = device
        .build_input_stream(
            supported.config(),
            move |frames: &[f32], _info: &cpal::InputCallbackInfo| {
                on_frames(frames);
            },
            move |error| {
                let detail = error.to_string();
                on_error(classify_error(error.kind(), StreamStage::Play), detail);
            },
            None,
        )
        .map_err(|error| classify_error(error.kind(), StreamStage::Build))?;
    stream
        .play()
        .map_err(|error| classify_error(error.kind(), StreamStage::Play))?;
    Ok(LiveMicStream {
        stream,
        device_id: device_id.to_owned(),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureEvent, HealthReason, MockMicrophoneBackend, StreamStage, WindowsMicError,
        device_info, event_for_runtime_fault, event_for_start_failure, health_reason_for,
        sanitize_label, select_input, stable_source_id,
    };
    use crate::capture_health::{HealthChange, HealthMonitor, SignalSample};
    use crate::capture_session::{CaptureSession, CaptureState, SourceKind};
    use himsat_events::SessionId;

    const SESSION: SessionId = SessionId::new(8);

    fn all_faults() -> Vec<WindowsMicError> {
        vec![
            WindowsMicError::NoInputDevices,
            WindowsMicError::StreamFault(StreamStage::Build),
            WindowsMicError::StreamFault(StreamStage::Play),
            WindowsMicError::DataDiscontinuity,
            WindowsMicError::PermissionDenied,
            WindowsMicError::ExclusiveModeConflict,
            WindowsMicError::EndpointUnavailable,
            WindowsMicError::AudioServiceUnavailable,
            WindowsMicError::RouteRerouted,
        ]
    }

    fn two_mic_backend() -> MockMicrophoneBackend {
        MockMicrophoneBackend::with_devices(
            vec![
                device_info("endpoint-builtin", "Microphone (Built-in)", true, 1, 48_000),
                device_info("endpoint-usb", "Microphone (USB Headset)", false, 2, 44_100),
            ],
            Some("endpoint-builtin".to_owned()),
        )
    }

    fn quiet_sample(reason: Option<HealthReason>) -> SignalSample {
        SignalSample {
            elapsed_millis: 0,
            level_dbfs_tenths: None,
            silent: false,
            clipping: false,
            queue_depth: 0,
            queue_capacity: 16,
            free_bytes: 1024,
            warn_bytes: 256,
            critical_bytes: 64,
            reason,
        }
    }

    #[test]
    fn stable_ids_match_for_equal_endpoints() {
        assert_eq!(
            stable_source_id("endpoint-builtin"),
            stable_source_id("endpoint-builtin")
        );
    }

    #[test]
    fn stable_ids_differ_for_distinct_endpoints() {
        assert_ne!(
            stable_source_id("endpoint-builtin"),
            stable_source_id("endpoint-usb")
        );
    }

    #[test]
    fn stable_ids_are_domain_separated_from_the_macos_adapter() {
        // The same endpoint/device string must not collide across the two
        // platform adapters, because the domain tag is part of the hash.
        assert_ne!(
            stable_source_id("shared-name"),
            crate::capture_macos::stable_source_id("shared-name")
        );
    }

    #[test]
    fn selection_prefers_endpoint_id_then_name_then_default() {
        let backend = two_mic_backend();
        let by_id = select_input(&backend, Some("endpoint-usb")).expect("id match");
        assert_eq!(by_id.info.name, "Microphone (USB Headset)");
        assert_eq!(by_id.source_id, stable_source_id("endpoint-usb"));
        assert_eq!(by_id.descriptor(SESSION).kind(), SourceKind::Microphone);
        let by_name = select_input(&backend, Some("Microphone (USB Headset)")).expect("name match");
        assert_eq!(by_name.source_id, stable_source_id("endpoint-usb"));
        let defaulted = select_input(&backend, None).expect("default");
        assert_eq!(defaulted.source_id, stable_source_id("endpoint-builtin"));
    }

    #[test]
    fn selection_falls_back_to_flagged_default_then_first() {
        let flagged = MockMicrophoneBackend::with_devices(
            vec![
                device_info("endpoint-a", "A", false, 1, 48_000),
                device_info("endpoint-b", "B", true, 1, 48_000),
            ],
            None,
        );
        assert_eq!(
            select_input(&flagged, None).expect("flagged").source_id,
            stable_source_id("endpoint-b")
        );
        let unnamed = MockMicrophoneBackend::with_devices(
            vec![device_info("endpoint-only", "", false, 1, 48_000)],
            None,
        );
        let first = select_input(&unnamed, Some("missing")).expect("first");
        assert_eq!(first.source_id, stable_source_id("endpoint-only"));
        assert_eq!(first.descriptor(SESSION).label(), "Unnamed microphone");
    }

    #[test]
    fn empty_enumeration_is_no_input_devices() {
        let backend = MockMicrophoneBackend::default();
        assert_eq!(
            select_input(&backend, None),
            Err(WindowsMicError::NoInputDevices)
        );
    }

    #[test]
    fn enumeration_faults_propagate_unchanged() {
        let backend = MockMicrophoneBackend::failing(WindowsMicError::PermissionDenied);
        assert_eq!(
            select_input(&backend, None),
            Err(WindowsMicError::PermissionDenied)
        );
    }

    #[test]
    fn labels_truncate_on_char_boundary_with_fallback() {
        assert_eq!(sanitize_label("  Microphone (USB)  "), "Microphone (USB)");
        assert_eq!(sanitize_label("   "), "Unnamed microphone");
        assert_eq!(sanitize_label("").chars().count(), 18);
        let long = "e".repeat(200);
        assert_eq!(sanitize_label(&long).chars().count(), 128);
        let wide = "h".repeat(127) + "é";
        assert_eq!(sanitize_label(&wide), wide);
        let over = "é".repeat(200);
        assert_eq!(sanitize_label(&over), "é".repeat(128));
    }

    #[test]
    fn start_failures_map_to_preparing_accepted_events() {
        for fault in all_faults() {
            let event = event_for_start_failure(&fault);
            let mut probe = CaptureSession::new(SESSION);
            assert!(probe.apply(CaptureEvent::Prepare).is_ok());
            assert!(
                probe.apply(event).is_ok(),
                "start mapping refused for {}",
                fault.classifier()
            );
        }
        assert_eq!(
            event_for_start_failure(&WindowsMicError::NoInputDevices),
            CaptureEvent::FailRecoverable(HealthReason::SourceSilent)
        );
        assert_eq!(
            event_for_start_failure(&WindowsMicError::PermissionDenied),
            CaptureEvent::FailRecoverable(HealthReason::PermissionRevoked)
        );
        assert_eq!(
            event_for_start_failure(&WindowsMicError::ExclusiveModeConflict),
            CaptureEvent::FailRecoverable(HealthReason::RouteChanged)
        );
    }

    #[test]
    fn runtime_faults_map_to_flowing_accepted_events() {
        for fault in all_faults() {
            let mut session = CaptureSession::new(SESSION);
            assert!(session.apply(CaptureEvent::Prepare).is_ok());
            assert!(
                session
                    .attach(
                        select_input(&two_mic_backend(), None)
                            .expect("device")
                            .descriptor(SESSION)
                    )
                    .is_ok()
            );
            assert!(session.apply(CaptureEvent::SourcesReady).is_ok());
            match event_for_runtime_fault(&fault) {
                Some(event) => assert!(
                    session.apply(event).is_ok(),
                    "runtime mapping refused for {}",
                    fault.classifier()
                ),
                None => assert!(
                    matches!(
                        fault,
                        WindowsMicError::RouteRerouted | WindowsMicError::DataDiscontinuity
                    ),
                    "only flowing telemetry conditions may omit a machine event"
                ),
            }
        }
    }

    #[test]
    fn rerouted_stream_keeps_flowing_without_a_machine_event() {
        let mut session = CaptureSession::new(SESSION);
        assert!(session.apply(CaptureEvent::Prepare).is_ok());
        let selected = select_input(&two_mic_backend(), None).expect("device");
        assert!(session.attach(selected.descriptor(SESSION)).is_ok());
        assert!(session.apply(CaptureEvent::SourcesReady).is_ok());
        assert_eq!(session.state(), CaptureState::RecordingHealthy);
        assert_eq!(
            event_for_runtime_fault(&WindowsMicError::RouteRerouted),
            None
        );
        assert_eq!(health_reason_for(&WindowsMicError::RouteRerouted), None);
    }

    #[test]
    fn runtime_health_reason_agrees_with_the_machine_mapping() {
        for fault in all_faults() {
            let from_event = match event_for_runtime_fault(&fault) {
                Some(CaptureEvent::Interrupted(reason)) => Some(reason),
                Some(other) => panic!("unexpected runtime mapping {other:?}"),
                None => None,
            };
            assert_eq!(
                health_reason_for(&fault),
                from_event,
                "health and machine mappings diverged for {}",
                fault.classifier()
            );
        }
    }

    #[test]
    fn permission_denial_surfaces_through_006b_health() {
        let mut monitor = HealthMonitor::new(SESSION);
        let state = CaptureState::RecordingHealthy;
        assert!(monitor.observe(state, &quiet_sample(None)).is_empty());
        let reason = health_reason_for(&WindowsMicError::PermissionDenied);
        let events = monitor.observe(state, &quiet_sample(reason));
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].change(),
            HealthChange::ReasonChanged {
                from: None,
                to: Some(HealthReason::PermissionRevoked),
            }
        );
        assert_eq!(events[0].session(), SESSION);
    }

    #[test]
    fn adapter_drives_session_without_refusal() {
        let backend = two_mic_backend();
        let mut session = CaptureSession::new(SESSION);
        assert!(session.apply(CaptureEvent::Prepare).is_ok());
        let selected = select_input(&backend, None).expect("device");
        assert!(session.attach(selected.descriptor(SESSION)).is_ok());
        assert!(session.apply(CaptureEvent::SourcesReady).is_ok());
        let fault = WindowsMicError::ExclusiveModeConflict;
        let event = event_for_runtime_fault(&fault).expect("interrupting fault");
        assert!(session.apply(event).is_ok());
        assert!(session.apply(CaptureEvent::ResumeRequested).is_ok());
        assert!(session.apply(CaptureEvent::RecoveryConfirmed).is_ok());
        assert!(session.apply(CaptureEvent::StopRequested).is_ok());
        assert!(session.apply(CaptureEvent::StopCompleted).is_ok());
        assert!(session.apply(CaptureEvent::CheckpointSealed).is_ok());
    }

    #[test]
    fn start_failure_path_recovers_through_retry() {
        let backend = MockMicrophoneBackend::default();
        let mut session = CaptureSession::new(SESSION);
        assert!(session.apply(CaptureEvent::Prepare).is_ok());
        let failure = select_input(&backend, None).expect_err("no devices");
        assert!(session.apply(event_for_start_failure(&failure)).is_ok());
        assert!(session.apply(CaptureEvent::RetryRequested).is_ok());
    }

    #[test]
    fn error_classifiers_are_stable_and_distinct() {
        let classifiers: Vec<&str> = all_faults()
            .iter()
            .map(WindowsMicError::classifier)
            .collect();
        assert_eq!(classifiers[0], "no-input-devices");
        assert_eq!(classifiers[1], "stream-build-fault");
        assert_eq!(classifiers[2], "stream-play-fault");
        assert_eq!(classifiers[3], "data-discontinuity");
        assert_eq!(classifiers[4], "permission-denied");
        assert_eq!(classifiers[5], "exclusive-mode-conflict");
        assert_eq!(classifiers[6], "endpoint-unavailable");
        assert_eq!(classifiers[7], "audio-service-unavailable");
        assert_eq!(classifiers[8], "route-rerouted");
        let mut unique = classifiers.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), classifiers.len());
    }
}

/// Windows-only binding tests. Classification and format mapping are
/// pure and therefore run without audio hardware; enumeration tolerates
/// hosts with no endpoints; stream opening stays behind
/// `HIMSAT_LIVE_MIC_TEST=1` so CI never touches capture hardware.
#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::{
        CpalMicrophoneBackend, MicrophoneBackend, SampleFormat, StreamStage, WindowsMicError,
        classify_error, event_for_runtime_fault, health_reason_for, open_f32_input_stream,
        portable_sample_format, select_input,
    };
    use crate::capture_checkpoint::ChunkCodec;
    use crate::capture_journal_commit::{ChunkJournal, SinkBinding};
    use crate::capture_payload_accum::{AccumConfig, PayloadAccum};
    use crate::capture_windows_pressure::StorageBudgets;
    use crate::vault::{KeyGeneration, VaultId};
    use crate::vault_keys::OwnedKeyMaterial;
    use crate::vault_media_chunk::{
        MEDIA_CHUNK_MAX_PLAINTEXT_BYTES, MEDIA_CHUNK_NONCE_BYTES, MediaChunkContext,
        decrypt_media_chunk, encrypt_media_chunk,
    };
    use himsat_events::{SessionId, SourceId};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn cpal_kinds_classify_deterministically() {
        assert_eq!(
            classify_error(cpal::ErrorKind::PermissionDenied, StreamStage::Build),
            WindowsMicError::PermissionDenied
        );
        assert_eq!(
            classify_error(cpal::ErrorKind::DeviceBusy, StreamStage::Play),
            WindowsMicError::ExclusiveModeConflict
        );
        assert_eq!(
            classify_error(cpal::ErrorKind::DeviceNotAvailable, StreamStage::Play),
            WindowsMicError::EndpointUnavailable
        );
        assert_eq!(
            classify_error(cpal::ErrorKind::HostUnavailable, StreamStage::Build),
            WindowsMicError::AudioServiceUnavailable
        );
        assert_eq!(
            classify_error(cpal::ErrorKind::DeviceChanged, StreamStage::Play),
            WindowsMicError::RouteRerouted
        );
        assert_eq!(
            classify_error(cpal::ErrorKind::UnsupportedConfig, StreamStage::Build),
            WindowsMicError::StreamFault(StreamStage::Build)
        );
        let xrun = classify_error(cpal::ErrorKind::Xrun, StreamStage::Play);
        assert_eq!(xrun, WindowsMicError::DataDiscontinuity);
        assert_eq!(event_for_runtime_fault(&xrun), None);
        assert_eq!(health_reason_for(&xrun), None);
        assert_eq!(
            classify_error(cpal::ErrorKind::Other, StreamStage::Build),
            WindowsMicError::StreamFault(StreamStage::Build)
        );
    }

    #[test]
    fn only_modelled_sample_formats_map() {
        assert_eq!(
            portable_sample_format(cpal::SampleFormat::F32),
            Some(SampleFormat::F32)
        );
        assert_eq!(
            portable_sample_format(cpal::SampleFormat::I16),
            Some(SampleFormat::I16)
        );
        assert_eq!(
            portable_sample_format(cpal::SampleFormat::U16),
            Some(SampleFormat::U16)
        );
        assert_eq!(portable_sample_format(cpal::SampleFormat::I32), None);
        assert_eq!(portable_sample_format(cpal::SampleFormat::F64), None);
        assert_eq!(portable_sample_format(cpal::SampleFormat::U8), None);
        assert_eq!(portable_sample_format(cpal::SampleFormat::I24), None);
    }

    #[test]
    fn cpal_enumeration_never_panics_and_selects_consistently() {
        let backend = CpalMicrophoneBackend;
        match backend.input_devices() {
            Ok(devices) => {
                let selected = select_input(&backend, None);
                if devices.is_empty() {
                    assert!(selected.is_err());
                } else {
                    let selected = selected.expect("non-empty enumeration selects");
                    let descriptor = selected.descriptor(himsat_events::SessionId::new(3));
                    assert!(!descriptor.label().is_empty());
                    assert!(devices.iter().all(|info| !info.device_id.is_empty()));
                }
            }
            Err(error) => {
                assert!(
                    matches!(
                        error,
                        WindowsMicError::NoInputDevices | WindowsMicError::AudioServiceUnavailable
                    ),
                    "enumeration fails only as absence or service unavailability"
                );
            }
        }
    }

    #[test]
    fn cpal_default_id_agrees_with_selection() {
        let backend = CpalMicrophoneBackend;
        if backend.default_input_id().is_some() {
            assert!(select_input(&backend, None).is_ok());
        }
    }

    #[test]
    fn live_stream_open_reports_frames_or_classified_fault() {
        if std::env::var("HIMSAT_LIVE_MIC_TEST").is_err() {
            return;
        }
        let strict = std::env::var("HIMSAT_GATE_E_REQUIRE_SIGNAL").is_ok();
        let backend = CpalMicrophoneBackend;
        let selected = select_input(&backend, None).expect("live test needs an endpoint");
        assert!(!selected.info.device_id.is_empty());
        let non_silent_frames = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&non_silent_frames);
        match open_f32_input_stream(
            &selected.info.device_id,
            move |frames: &[f32]| {
                if frames.iter().any(|sample| sample.abs() > 0.000_01) {
                    counter.fetch_add(frames.len(), Ordering::Relaxed);
                }
            },
            move |_error, _detail| {},
        ) {
            Ok(stream) => {
                assert_eq!(stream.device_id(), selected.info.device_id);
                assert!(stream.config().sample_rate_hz > 0);
                assert!(stream.config().channels > 0);
                assert_eq!(stream.config().format, SampleFormat::F32);
                let deadline = Instant::now() + Duration::from_secs(10);
                while non_silent_frames.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(25));
                }
                let non_silent_frames = non_silent_frames.load(Ordering::Relaxed);
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone\",\"outcome\":\"{}\",\"non_silent_frames\":{non_silent_frames}}}",
                    if non_silent_frames == 0 {
                        "silence"
                    } else {
                        "frames"
                    }
                );
                if strict {
                    assert!(non_silent_frames > 0, "microphone observed no signal");
                }
                assert!(stream.pause().is_ok());
                assert!(stream.resume().is_ok());
                drop(stream);
            }
            Err(error) => {
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone\",\"outcome\":\"classified_fault\",\"classifier\":\"{}\"}}",
                    error.classifier()
                );
                assert!(
                    !strict,
                    "microphone live open failed with {}",
                    error.classifier()
                );
                assert!(
                    matches!(
                        error,
                        WindowsMicError::PermissionDenied
                            | WindowsMicError::ExclusiveModeConflict
                            | WindowsMicError::EndpointUnavailable
                            | WindowsMicError::AudioServiceUnavailable
                            | WindowsMicError::StreamFault(_)
                    ),
                    "live open failed with an unexpected classification"
                );
            }
        }
    }

    /// Gate E sustained-hold probe (008F): keeps one live microphone stream
    /// open for `HIMSAT_LIVE_SUSTAINED_SECS` (default 60, clamped 5..600),
    /// pauses and resumes it mid-hold, then reports flow counters. Only the
    /// `push`-equivalent callback runs on the audio thread (two counters and
    /// one threshold scan, no allocation, lock, I/O, or logging). NOT RUN in
    /// CI; runs only with `HIMSAT_LIVE_SUSTAINED_TEST=1`, and the non-silent
    /// assertion additionally requires `HIMSAT_GATE_E_REQUIRE_SIGNAL=1`.
    #[test]
    fn live_sustained_hold_reports_signal_without_interruption() {
        if std::env::var("HIMSAT_LIVE_SUSTAINED_TEST").is_err() {
            return;
        }
        let strict = std::env::var("HIMSAT_GATE_E_REQUIRE_SIGNAL").is_ok();
        let seconds: u64 = match std::env::var("HIMSAT_LIVE_SUSTAINED_SECS") {
            Err(_) => 60,
            Ok(value) => value
                .parse()
                .expect("HIMSAT_LIVE_SUSTAINED_SECS must be an integer"),
        }
        .clamp(5, 600);
        let backend = CpalMicrophoneBackend;
        let selected = match select_input(&backend, None) {
            Ok(selected) => selected,
            Err(error) => {
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-sustained\",\"outcome\":\"classified_fault\",\"classifier\":\"{}\"}}",
                    error.classifier()
                );
                assert!(
                    !strict,
                    "sustained selection failed with {}",
                    error.classifier()
                );
                assert!(
                    matches!(
                        error,
                        WindowsMicError::NoInputDevices
                            | WindowsMicError::PermissionDenied
                            | WindowsMicError::ExclusiveModeConflict
                            | WindowsMicError::EndpointUnavailable
                            | WindowsMicError::AudioServiceUnavailable
                            | WindowsMicError::RouteRerouted
                            | WindowsMicError::StreamFault(_)
                    ),
                    "sustained selection failed with an unexpected classification"
                );
                return;
            }
        };
        let callbacks = Arc::new(AtomicUsize::new(0));
        let frames_total = Arc::new(AtomicUsize::new(0));
        let non_silent = Arc::new(AtomicUsize::new(0));
        let on_frames_callbacks = Arc::clone(&callbacks);
        let on_frames_total = Arc::clone(&frames_total);
        let on_frames_signal = Arc::clone(&non_silent);
        match open_f32_input_stream(
            &selected.info.device_id,
            move |frames: &[f32]| {
                on_frames_callbacks.fetch_add(1, Ordering::Relaxed);
                on_frames_total.fetch_add(frames.len(), Ordering::Relaxed);
                if frames.iter().any(|sample| sample.abs() > 0.000_01) {
                    on_frames_signal.fetch_add(frames.len(), Ordering::Relaxed);
                }
            },
            move |_error, _detail| {},
        ) {
            Ok(stream) => {
                let hold = Duration::from_secs(seconds);
                let start = Instant::now();
                while start.elapsed() < hold / 2 {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Err(error) = stream.pause() {
                    panic!("sustained pause failed: {}", error.classifier());
                }
                std::thread::sleep(Duration::from_millis(500));
                if let Err(error) = stream.resume() {
                    panic!("sustained resume failed: {}", error.classifier());
                }
                while start.elapsed() < hold {
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(stream);
                let callback_count = callbacks.load(Ordering::Relaxed);
                let frame_count = frames_total.load(Ordering::Relaxed);
                let signal_frames = non_silent.load(Ordering::Relaxed);
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-sustained\",\"outcome\":\"{}\",\"seconds\":{seconds},\"callbacks\":{callback_count},\"frames\":{frame_count},\"non_silent_frames\":{signal_frames}}}",
                    if signal_frames == 0 {
                        "silence"
                    } else {
                        "frames"
                    }
                );
                assert!(callback_count > 0, "sustained hold delivered no callbacks");
                assert!(frame_count > 0, "sustained hold delivered no frames");
                if strict {
                    assert!(signal_frames > 0, "sustained hold observed no signal");
                }
            }
            Err(error) => {
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-sustained\",\"outcome\":\"classified_fault\",\"classifier\":\"{}\"}}",
                    error.classifier()
                );
                assert!(!strict, "sustained hold failed with {}", error.classifier());
                assert!(
                    matches!(
                        error,
                        WindowsMicError::PermissionDenied
                            | WindowsMicError::ExclusiveModeConflict
                            | WindowsMicError::EndpointUnavailable
                            | WindowsMicError::AudioServiceUnavailable
                            // A device-change race during open surfaces here;
                            // a reroute keeps flowing and is not a fault.
                            | WindowsMicError::RouteRerouted
                            | WindowsMicError::StreamFault(_)
                    ),
                    "sustained hold failed with an unexpected classification"
                );
            }
        }
    }

    /// Gate E live round-trip probe (008F): captures one second of real
    /// microphone audio, then moves the bytes through the genuine 008E
    /// accumulator and 008D commit path under a test-only VRK, closes the
    /// journal, and decrypts the exact captured bytes. The audio callback is
    /// test-only scaffolding around a pre-reserved buffer: it never blocks
    /// (`try_lock`), never grows the buffer past its cap, and accounts every
    /// skipped byte in a drop counter that must read zero afterwards.
    /// Production capture uses `PayloadAccum::push`, never this mutex.
    /// Sealing, committing, and verification run after the stream is dropped,
    /// on the test thread. NOT RUN in CI; runs only with
    /// `HIMSAT_LIVE_ROUNDTRIP_TEST=1`.
    #[test]
    fn live_mic_frames_round_trip_into_the_journal() {
        if std::env::var("HIMSAT_LIVE_ROUNDTRIP_TEST").is_err() {
            return;
        }
        let strict = std::env::var("HIMSAT_GATE_E_REQUIRE_SIGNAL").is_ok();
        const ROUNDTRIP_VRK: [u8; 32] = [0x5B; 32];
        const ROUNDTRIP_VAULT: [u8; 16] = [0x33; 16];
        const ROUNDTRIP_SESSION: u128 = 0x0304_0506_0708_090a_0b0c_0d0e_0f10_1112;
        const ROUNDTRIP_SOURCE: u128 = 0x2e2d_2c2b_2a29_2827_2625_2423_2221_201f;
        // Live device proved stereo F32 at 48 kHz on the qualification host
        // (768 KiB per two seconds); one second fits the cap with margin
        // while staying far under the 005A plaintext ceiling.
        const CAPTURE_MILLIS: u64 = 1_000;
        const CAPTURE_BYTE_CAP: usize = 512 * 1_024;
        const {
            assert!(
                CAPTURE_BYTE_CAP <= MEDIA_CHUNK_MAX_PLAINTEXT_BYTES,
                "round-trip capture must fit one sealed chunk"
            );
        }

        let backend = CpalMicrophoneBackend;
        let selected = match select_input(&backend, None) {
            Ok(selected) => selected,
            Err(error) => {
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-roundtrip\",\"outcome\":\"classified_fault\",\"classifier\":\"{}\"}}",
                    error.classifier()
                );
                assert!(
                    !strict,
                    "round-trip selection failed with {}",
                    error.classifier()
                );
                assert!(
                    matches!(
                        error,
                        WindowsMicError::NoInputDevices
                            | WindowsMicError::PermissionDenied
                            | WindowsMicError::ExclusiveModeConflict
                            | WindowsMicError::EndpointUnavailable
                            | WindowsMicError::AudioServiceUnavailable
                            | WindowsMicError::RouteRerouted
                            | WindowsMicError::StreamFault(_)
                    ),
                    "round-trip selection failed with an unexpected classification"
                );
                return;
            }
        };
        let captured = Arc::new(Mutex::new(Vec::with_capacity(CAPTURE_BYTE_CAP)));
        let dropped = Arc::new(AtomicUsize::new(0));
        let signal = Arc::new(AtomicUsize::new(0));
        let on_frames_bytes = Arc::clone(&captured);
        let on_frames_dropped = Arc::clone(&dropped);
        let on_frames_signal = Arc::clone(&signal);
        let stream = match open_f32_input_stream(
            &selected.info.device_id,
            move |frames: &[f32]| {
                if frames.iter().any(|sample| sample.abs() > 0.000_01) {
                    on_frames_signal.fetch_add(frames.len(), Ordering::Relaxed);
                }
                let wanted = frames.len().saturating_mul(4);
                if let Ok(mut bytes) = on_frames_bytes.try_lock() {
                    let room = CAPTURE_BYTE_CAP.saturating_sub(bytes.len());
                    // Whole samples only: the take rounds down so the buffer
                    // can never exceed its cap by a partial F32 frame.
                    let take = wanted.min(room) & !3;
                    let mut taken = 0_usize;
                    for sample in frames {
                        if taken >= take {
                            break;
                        }
                        bytes.extend_from_slice(&sample.to_le_bytes());
                        taken += 4;
                    }
                    on_frames_dropped.fetch_add(wanted - taken, Ordering::Relaxed);
                } else {
                    on_frames_dropped.fetch_add(wanted, Ordering::Relaxed);
                }
            },
            move |_error, _detail| {},
        ) {
            Ok(stream) => stream,
            Err(error) => {
                println!(
                    "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-roundtrip\",\"outcome\":\"classified_fault\",\"classifier\":\"{}\"}}",
                    error.classifier()
                );
                assert!(
                    !strict,
                    "round-trip capture failed with {}",
                    error.classifier()
                );
                assert!(
                    matches!(
                        error,
                        WindowsMicError::PermissionDenied
                            | WindowsMicError::ExclusiveModeConflict
                            | WindowsMicError::EndpointUnavailable
                            | WindowsMicError::AudioServiceUnavailable
                            // A device-change race during open surfaces here;
                            // a reroute keeps flowing and is not a fault.
                            | WindowsMicError::RouteRerouted
                            | WindowsMicError::StreamFault(_)
                    ),
                    "round-trip capture failed with an unexpected classification"
                );
                return;
            }
        };
        std::thread::sleep(Duration::from_millis(CAPTURE_MILLIS));
        drop(stream);
        let dropped_bytes = dropped.load(Ordering::Relaxed);
        assert_eq!(
            dropped_bytes, 0,
            "round-trip dropped {dropped_bytes} captured bytes on the audio thread"
        );
        let captured = captured.lock().expect("capture buffer lock").clone();
        let signal_frames = signal.load(Ordering::Relaxed);
        if captured.is_empty() {
            println!(
                "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-roundtrip\",\"outcome\":\"silence\",\"captured_bytes\":0,\"non_silent_frames\":{signal_frames}}}"
            );
            assert!(
                !strict,
                "round-trip captured no bytes under a required signal"
            );
            return;
        }
        if strict {
            assert!(signal_frames > 0, "round-trip captured no signal");
        }

        struct RoundtripDir {
            path: PathBuf,
        }

        impl RoundtripDir {
            fn new(id: u64) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "himsat-gate-e-roundtrip-{}-{id}",
                    std::process::id()
                ));
                if path.exists() {
                    fs::remove_dir_all(&path).expect("stale round-trip dir clears");
                }
                fs::create_dir_all(&path).expect("round-trip temp dir");
                Self { path }
            }

            fn journal(&self) -> PathBuf {
                self.path.join("session.jrn")
            }
        }

        impl Drop for RoundtripDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }

        static ROUNDTRIP_COUNTER: AtomicU64 = AtomicU64::new(0);
        let roundtrip_id = ROUNDTRIP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = RoundtripDir::new(roundtrip_id);
        let binding = SinkBinding {
            vault_id: VaultId::from_bytes(ROUNDTRIP_VAULT),
            session: SessionId::new(ROUNDTRIP_SESSION),
            generation: KeyGeneration::new(3).expect("test generation is non-zero"),
        };
        let mut journal =
            ChunkJournal::create(&dir.journal(), binding).expect("round-trip journal");
        let mut accum = PayloadAccum::new(AccumConfig {
            binding,
            // Raw F32-LE bytes: no PCM conversion exists yet (010 owns DSP),
            // so the codec stays honestly unlabelled instead of misfiling
            // microphone float frames as Pcm16.
            codec: ChunkCodec::Unknown,
            source: SourceId::new(ROUNDTRIP_SOURCE),
            max_chunk_bytes: MEDIA_CHUNK_MAX_PLAINTEXT_BYTES,
            starting_index: 0,
            max_pending_chunks: 4,
            budgets: StorageBudgets {
                warn_bytes: 1_000_000,
                critical_bytes: 100_000,
                queue_capacity: 8,
            },
            cadence_millis: 1_000,
        })
        .expect("round-trip accumulator");
        let report = accum
            .push(&captured, 0, CAPTURE_MILLIS, 10_000_000, 0)
            .expect("round-trip push");
        assert_eq!(report.accepted_bytes, captured.len());
        let index = accum.next_chunk_index();
        // Test-only nonce: chunk index plus process identity, so repeated
        // runs never re-encrypt under one keystream. Production nonces stay
        // under the B203 lifecycle and are never derived here.
        let mut nonce = [0_u8; MEDIA_CHUNK_NONCE_BYTES];
        nonce[..8].copy_from_slice(&index.to_be_bytes());
        nonce[8..16].copy_from_slice(&(std::process::id() as u64).to_be_bytes());
        nonce[16..24].copy_from_slice(&roundtrip_id.to_be_bytes());
        let (chunk, facts) = accum
            .seal_next(
                nonce,
                |context: MediaChunkContext,
                 nonce: [u8; MEDIA_CHUNK_NONCE_BYTES],
                 plaintext: &[u8]| {
                    encrypt_media_chunk(
                        &OwnedKeyMaterial::from_bytes(ROUNDTRIP_VRK),
                        context,
                        nonce,
                        plaintext,
                    )
                },
            )
            .expect("round-trip seal");
        assert_eq!(facts.plaintext_len, captured.len() as u64);
        let commit = journal.commit(chunk.clone(), facts).expect("commit");
        assert_eq!(commit.chunk_index, 0);
        accum.note_committed().expect("acknowledge");
        let plaintext = decrypt_media_chunk(
            &OwnedKeyMaterial::from_bytes(ROUNDTRIP_VRK),
            binding.chunk_context(0),
            &chunk.envelope,
        )
        .expect("round-trip chunk decrypts");
        assert_eq!(plaintext, captured);
        let summary = accum.close().expect("round-trip close");
        assert_eq!(summary.chunks_sealed, 1);
        let closed = journal.close().expect("round-trip journal close");
        assert_eq!(closed.commits, 1);
        println!(
            "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-roundtrip\",\"outcome\":\"frames\",\"captured_bytes\":{},\"non_silent_frames\":{signal_frames}}}",
            captured.len()
        );
    }
}
