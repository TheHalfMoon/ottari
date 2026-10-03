#![cfg(all(test, target_os = "windows"))]

use crate::capture_windows::{
    CpalMicrophoneBackend, WindowsMicError, open_f32_input_stream, select_input,
};
use crate::capture_windows_lifecycle::StreamLossAccount;
use crate::capture_windows_pressure::{
    AdmissionDecision, RefusalReason, StorageBudgets, decide_admission,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn env_u64(name: &str) -> u64 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be an integer"))
}

fn fault_code(error: &WindowsMicError) -> usize {
    match error {
        WindowsMicError::EndpointUnavailable => 1,
        WindowsMicError::RouteRerouted => 2,
        WindowsMicError::ExclusiveModeConflict => 3,
        WindowsMicError::PermissionDenied => 4,
        WindowsMicError::AudioServiceUnavailable => 5,
        WindowsMicError::NoInputDevices => 6,
        WindowsMicError::StreamFault(_) => 7,
        WindowsMicError::DataDiscontinuity => 8,
    }
}

#[test]
fn measured_storage_threshold_refuses_at_real_host_free_space() {
    if std::env::var("HIMSAT_LIVE_STORAGE_THRESHOLD_TEST").is_err() {
        return;
    }

    let free_bytes = env_u64("HIMSAT_GATE_E_FREE_BYTES");
    let budgets = StorageBudgets {
        warn_bytes: free_bytes.saturating_add(1),
        critical_bytes: free_bytes,
        queue_capacity: 8,
    };
    let decision = decide_admission(budgets, free_bytes, 0);
    assert_eq!(
        decision,
        AdmissionDecision::Refuse(RefusalReason::StorageCritical),
        "measured host free space at the caller critical boundary must refuse explicitly"
    );
    println!(
        "HIMSAT_GATE_E_RESULT={{\"path\":\"storage-threshold\",\"outcome\":\"refused\",\"classifier\":\"storage-critical\",\"free_bytes\":{free_bytes}}}"
    );
}

#[test]
fn live_expected_exclusive_mode_conflict_is_typed() {
    if std::env::var("HIMSAT_LIVE_EXPECT_EXCLUSIVE_CONFLICT").is_err() {
        return;
    }

    let backend = CpalMicrophoneBackend;
    let preferred = std::env::var("HIMSAT_GATE_E_DEVICE_SELECTOR").ok();
    let selected = select_input(&backend, preferred.as_deref())
        .expect("exclusive-mode probe needs a selected endpoint");

    match open_f32_input_stream(&selected.info.device_id, |_frames| {}, |_error, _detail| {}) {
        Err(WindowsMicError::ExclusiveModeConflict) => {
            println!(
                "HIMSAT_GATE_E_RESULT={{\"path\":\"exclusive-mode\",\"outcome\":\"classified_fault\",\"classifier\":\"exclusive-mode-conflict\"}}"
            );
        }
        Err(other) => panic!(
            "exclusive-mode probe returned {}, expected exclusive-mode-conflict",
            other.classifier()
        ),
        Ok(stream) => {
            drop(stream);
            panic!("exclusive-mode holder did not block the Himsat stream open");
        }
    }
}

#[test]
fn live_multi_hour_capture_reports_continuity() {
    if std::env::var("HIMSAT_LIVE_MULTI_HOUR_TEST").is_err() {
        return;
    }

    let seconds = env_u64("HIMSAT_LIVE_MULTI_HOUR_SECS");
    assert!(
        (7_200..=14_400).contains(&seconds),
        "multi-hour Gate E duration must be between 7200 and 14400 seconds"
    );
    let strict_signal = std::env::var("HIMSAT_GATE_E_REQUIRE_SIGNAL").is_ok();
    let backend = CpalMicrophoneBackend;
    let preferred = std::env::var("HIMSAT_GATE_E_DEVICE_SELECTOR").ok();
    let selected = select_input(&backend, preferred.as_deref())
        .expect("multi-hour probe needs a selected endpoint");

    let callbacks = Arc::new(AtomicUsize::new(0));
    let samples = Arc::new(AtomicUsize::new(0));
    let signal_samples = Arc::new(AtomicUsize::new(0));
    let discontinuities = Arc::new(AtomicUsize::new(0));
    let reroutes = Arc::new(AtomicUsize::new(0));
    let fatal_errors = Arc::new(AtomicUsize::new(0));
    let max_callback_samples = Arc::new(AtomicUsize::new(0));
    let callback_counter = Arc::clone(&callbacks);
    let sample_counter = Arc::clone(&samples);
    let signal_counter = Arc::clone(&signal_samples);
    let discontinuity_counter = Arc::clone(&discontinuities);
    let reroute_counter = Arc::clone(&reroutes);
    let fatal_counter = Arc::clone(&fatal_errors);
    let max_callback_counter = Arc::clone(&max_callback_samples);

    let stream = open_f32_input_stream(
        &selected.info.device_id,
        move |input: &[f32]| {
            callback_counter.fetch_add(1, Ordering::Relaxed);
            sample_counter.fetch_add(input.len(), Ordering::Relaxed);
            max_callback_counter.fetch_max(input.len(), Ordering::Relaxed);
            if input.iter().any(|sample| sample.abs() > 0.000_01) {
                signal_counter.fetch_add(input.len(), Ordering::Relaxed);
            }
        },
        move |error, _detail| match error {
            WindowsMicError::DataDiscontinuity => {
                discontinuity_counter.fetch_add(1, Ordering::Relaxed);
            }
            WindowsMicError::RouteRerouted => {
                reroute_counter.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                fatal_counter.fetch_add(1, Ordering::Relaxed);
            }
        },
    )
    .unwrap_or_else(|error| panic!("multi-hour stream open failed: {}", error.classifier()));

    // WASAPI may report startup xruns while the shared engine settles. They are
    // preserved as warm-up evidence, but the endurance window starts only after
    // two seconds of proven flow. A measured-window data discontinuity is
    // counted Windows-specific evidence, never hidden, and never an automatic
    // PASS or FAIL: the window passes only when callbacks continue for the
    // full duration with no terminal error and delivered frames reconcile
    // against wall-clock expectation within one-callback settlement tolerance,
    // leaving zero unexplained shortfall beyond that tolerance.
    let warmup = Duration::from_secs(2);
    let warmup_start = Instant::now();
    let mut last_callbacks = 0_usize;
    let mut last_progress = Instant::now();
    while warmup_start.elapsed() < warmup {
        std::thread::sleep(Duration::from_millis(50));
        let current = callbacks.load(Ordering::Relaxed);
        if current > last_callbacks {
            last_callbacks = current;
            last_progress = Instant::now();
        } else if last_progress.elapsed() > Duration::from_secs(1) {
            panic!("multi-hour warm-up made no callback progress for more than one second");
        }
        assert_eq!(
            fatal_errors.load(Ordering::Relaxed),
            0,
            "multi-hour warm-up reported a terminal runtime stream error"
        );
    }
    assert!(
        callbacks.load(Ordering::Relaxed) > 0,
        "multi-hour warm-up delivered no callbacks"
    );

    let base_callbacks = callbacks.load(Ordering::Relaxed);
    let base_samples = samples.load(Ordering::Relaxed);
    let base_signal = signal_samples.load(Ordering::Relaxed);
    let base_discontinuities = discontinuities.load(Ordering::Relaxed);
    let base_reroutes = reroutes.load(Ordering::Relaxed);
    let base_fatal = fatal_errors.load(Ordering::Relaxed);
    let channels = usize::from(stream.config().channels);
    assert!(channels > 0, "multi-hour stream reported zero channels");

    let hold = Duration::from_secs(seconds);
    let expected_frames = StreamLossAccount::expected_frames(stream.config().sample_rate_hz, hold);
    let start = Instant::now();
    let hard_deadline = hold + Duration::from_secs(10);
    let mut last_callbacks = base_callbacks;
    let mut last_progress = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let current = callbacks.load(Ordering::Relaxed);
        if current > last_callbacks {
            last_callbacks = current;
            last_progress = Instant::now();
        } else if last_progress.elapsed() > Duration::from_secs(10) {
            panic!("multi-hour capture made no callback progress for more than 10 seconds");
        }
        assert_eq!(
            fatal_errors.load(Ordering::Relaxed),
            base_fatal,
            "multi-hour capture reported a terminal runtime stream error"
        );
        // Measured-window data discontinuities are counted explicitly below;
        // they are not an automatic FAIL here. A discontinuity only fails the
        // run when it produces unexplained loss or another canonical failure.

        let delivered_samples = samples.load(Ordering::Relaxed).saturating_sub(base_samples);
        let delivered_frames = delivered_samples / channels;
        if start.elapsed() >= hold
            && u64::try_from(delivered_frames).unwrap_or(u64::MAX) >= expected_frames
        {
            break;
        }
        assert!(
            start.elapsed() < hard_deadline,
            "multi-hour capture did not deliver the expected frame count within the settlement allowance"
        );
    }
    let duration_seconds = start.elapsed().as_secs();
    drop(stream);

    let final_callbacks = callbacks.load(Ordering::Relaxed);
    let final_samples = samples.load(Ordering::Relaxed);
    let final_signal = signal_samples.load(Ordering::Relaxed);
    let final_discontinuities = discontinuities.load(Ordering::Relaxed);
    let final_reroutes = reroutes.load(Ordering::Relaxed);
    let final_fatal = fatal_errors.load(Ordering::Relaxed);
    let max_samples = max_callback_samples.load(Ordering::Relaxed);
    assert!(
        final_callbacks >= base_callbacks
            && final_samples >= base_samples
            && final_signal >= base_signal
            && final_discontinuities >= base_discontinuities
            && final_reroutes >= base_reroutes
            && final_fatal >= base_fatal,
        "multi-hour capture counters regressed, indicating counter saturation or wrap"
    );
    assert!(
        final_callbacks < usize::MAX && final_samples < usize::MAX && final_signal < usize::MAX,
        "multi-hour capture counter saturation makes loss accounting unusable"
    );
    let callback_count = final_callbacks.saturating_sub(base_callbacks);
    let frame_count = final_samples.saturating_sub(base_samples) / channels;
    let non_silent = final_signal.saturating_sub(base_signal) / channels;
    let warmup_discontinuities = base_discontinuities;
    let runtime_discontinuities = final_discontinuities.saturating_sub(base_discontinuities);
    let route_reroutes = final_reroutes.saturating_sub(base_reroutes);
    let terminal_runtime_errors = final_fatal.saturating_sub(base_fatal);
    let callback_tolerance_frames = u64::try_from(max_samples / channels).unwrap_or(u64::MAX);
    let frame_count_u64 = u64::try_from(frame_count).unwrap_or(u64::MAX);
    let unexplained_shortfall = expected_frames.saturating_sub(frame_count_u64);
    assert!(
        callback_count > 0,
        "multi-hour capture delivered no callbacks"
    );
    assert_eq!(
        terminal_runtime_errors, 0,
        "multi-hour capture reported a terminal runtime stream error"
    );
    assert!(
        duration_seconds >= seconds,
        "multi-hour capture did not run the full required Gate E duration"
    );
    assert!(
        frame_count_u64.saturating_add(callback_tolerance_frames) >= expected_frames,
        "multi-hour capture has unexplained frame loss beyond one-callback tolerance: frames={frame_count} expected={expected_frames} tolerance={callback_tolerance_frames} shortfall={unexplained_shortfall} discontinuities={runtime_discontinuities}"
    );
    assert!(
        unexplained_shortfall <= callback_tolerance_frames,
        "multi-hour capture has unexplained frame loss beyond one-callback tolerance"
    );
    if strict_signal {
        assert!(
            non_silent > 0,
            "multi-hour capture observed no non-silent frames"
        );
    }
    println!(
        "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-multi-hour\",\"outcome\":\"continuous\",\"seconds\":{seconds},\"duration_seconds\":{duration_seconds},\"callbacks\":{callback_count},\"frames\":{frame_count},\"expected_frames\":{expected_frames},\"callback_tolerance_frames\":{callback_tolerance_frames},\"unexplained_shortfall\":{unexplained_shortfall},\"non_silent_frames\":{non_silent},\"warmup_discontinuities\":{warmup_discontinuities},\"data_discontinuities\":{runtime_discontinuities},\"runtime_discontinuities\":{runtime_discontinuities},\"route_reroutes\":{route_reroutes},\"terminal_runtime_errors\":{terminal_runtime_errors}}}"
    );
}

#[test]
fn live_physical_removal_reports_endpoint_unavailable() {
    if std::env::var("HIMSAT_LIVE_PHYSICAL_REMOVAL_TEST").is_err() {
        return;
    }

    let wait_seconds = env_u64("HIMSAT_LIVE_PHYSICAL_REMOVAL_SECS");
    assert!(
        (10..=300).contains(&wait_seconds),
        "physical-removal window must be between 10 and 300 seconds"
    );
    let selector = std::env::var("HIMSAT_GATE_E_DEVICE_SELECTOR")
        .expect("physical-removal probe requires HIMSAT_GATE_E_DEVICE_SELECTOR");
    let backend = CpalMicrophoneBackend;
    let selected = select_input(&backend, Some(&selector))
        .expect("physical-removal probe could not resolve the selected endpoint");

    let callbacks = Arc::new(AtomicUsize::new(0));
    let observed_fault = Arc::new(AtomicUsize::new(0));
    let discontinuities = Arc::new(AtomicUsize::new(0));
    let reroutes = Arc::new(AtomicUsize::new(0));
    let callback_counter = Arc::clone(&callbacks);
    let fault_slot = Arc::clone(&observed_fault);
    let discontinuity_counter = Arc::clone(&discontinuities);
    let reroute_counter = Arc::clone(&reroutes);
    let stream = open_f32_input_stream(
        &selected.info.device_id,
        move |_frames| {
            callback_counter.fetch_add(1, Ordering::Relaxed);
        },
        move |error, _detail| match error {
            WindowsMicError::DataDiscontinuity => {
                discontinuity_counter.fetch_add(1, Ordering::Relaxed);
            }
            WindowsMicError::RouteRerouted => {
                reroute_counter.fetch_add(1, Ordering::Relaxed);
            }
            other => {
                let code = fault_code(&other);
                let _ = fault_slot.compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
            }
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "physical-removal stream open failed: {}",
            error.classifier()
        )
    });

    // Do not arm on stream-open alone. Prove live callback flow first so a
    // startup xrun cannot be mistaken for the detach result.
    let warmup_deadline = Instant::now() + Duration::from_secs(2);
    while callbacks.load(Ordering::Relaxed) < 10 && Instant::now() < warmup_deadline {
        assert_eq!(
            observed_fault.load(Ordering::Relaxed),
            0,
            "physical-removal probe saw a terminal fault before arming"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let callbacks_before_arm = callbacks.load(Ordering::Relaxed);
    assert!(
        callbacks_before_arm >= 10,
        "physical-removal probe did not prove stable pre-removal live flow"
    );
    let discontinuities_before_arm = discontinuities.load(Ordering::Relaxed);
    let reroutes_before_arm = reroutes.load(Ordering::Relaxed);

    println!(
        "HIMSAT_GATE_E_ARMED={{\"path\":\"physical-removal\",\"device_selector\":\"configured\",\"wait_seconds\":{wait_seconds},\"callbacks_before_arm\":{callbacks_before_arm},\"warmup_discontinuities\":{discontinuities_before_arm}}}"
    );
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    while observed_fault.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);

    let callback_count = callbacks.load(Ordering::Relaxed);
    let code = observed_fault.load(Ordering::Relaxed);
    let runtime_discontinuities = discontinuities
        .load(Ordering::Relaxed)
        .saturating_sub(discontinuities_before_arm);
    let route_reroutes = reroutes
        .load(Ordering::Relaxed)
        .saturating_sub(reroutes_before_arm);
    assert_eq!(
        code, 1,
        "physical removal must surface endpoint-unavailable; observed fault code {code}"
    );
    println!(
        "HIMSAT_GATE_E_RESULT={{\"path\":\"physical-removal\",\"outcome\":\"classified_fault\",\"classifier\":\"endpoint-unavailable\",\"callbacks_before_arm\":{callbacks_before_arm},\"callbacks_total\":{callback_count},\"warmup_discontinuities\":{discontinuities_before_arm},\"runtime_discontinuities\":{runtime_discontinuities},\"route_reroutes\":{route_reroutes}}}"
    );
}
