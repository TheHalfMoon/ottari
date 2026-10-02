#![cfg(all(test, target_os = "windows"))]

use crate::capture_windows::{
    CpalMicrophoneBackend, WindowsMicError, open_f32_input_stream, select_input,
};
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
    let frames = Arc::new(AtomicUsize::new(0));
    let signal_frames = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));
    let callback_counter = Arc::clone(&callbacks);
    let frame_counter = Arc::clone(&frames);
    let signal_counter = Arc::clone(&signal_frames);
    let error_counter = Arc::clone(&errors);
    let (error_tx, error_rx) = std::sync::mpsc::sync_channel::<(&'static str, String)>(1);

    let stream = open_f32_input_stream(
        &selected.info.device_id,
        move |input: &[f32]| {
            callback_counter.fetch_add(1, Ordering::Relaxed);
            frame_counter.fetch_add(input.len(), Ordering::Relaxed);
            if input.iter().any(|sample| sample.abs() > 0.000_01) {
                signal_counter.fetch_add(input.len(), Ordering::Relaxed);
            }
        },
        move |error, detail| {
            let _ = error_tx.try_send((error.classifier(), detail));
            error_counter.fetch_add(1, Ordering::Release);
        },
    )
    .unwrap_or_else(|error| panic!("multi-hour stream open failed: {}", error.classifier()));

    let start = Instant::now();
    let hold = Duration::from_secs(seconds);
    let mut last_callbacks = 0_usize;
    let mut last_progress = Instant::now();
    while start.elapsed() < hold {
        std::thread::sleep(Duration::from_millis(250));
        let current = callbacks.load(Ordering::Relaxed);
        if current > last_callbacks {
            last_callbacks = current;
            last_progress = Instant::now();
        } else if last_progress.elapsed() > Duration::from_secs(10) {
            panic!("multi-hour capture made no callback progress for more than 10 seconds");
        }
        let error_count = errors.load(Ordering::Acquire);
        if error_count != 0 {
            let (classifier, detail) = error_rx
                .try_recv()
                .unwrap_or_else(|_| ("unavailable", "no first-error detail captured".to_owned()));
            panic!(
                "multi-hour capture reported {error_count} runtime stream errors; first_classifier={classifier}; first_detail={detail}"
            );
        }
    }
    drop(stream);

    let callback_count = callbacks.load(Ordering::Relaxed);
    let frame_count = frames.load(Ordering::Relaxed);
    let non_silent = signal_frames.load(Ordering::Relaxed);
    assert!(
        callback_count > 0,
        "multi-hour capture delivered no callbacks"
    );
    assert!(frame_count > 0, "multi-hour capture delivered no frames");
    if strict_signal {
        assert!(
            non_silent > 0,
            "multi-hour capture observed no non-silent frames"
        );
    }
    println!(
        "HIMSAT_GATE_E_RESULT={{\"path\":\"microphone-multi-hour\",\"outcome\":\"continuous\",\"seconds\":{seconds},\"callbacks\":{callback_count},\"frames\":{frame_count},\"non_silent_frames\":{non_silent},\"runtime_errors\":0}}"
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
    let callback_counter = Arc::clone(&callbacks);
    let fault_slot = Arc::clone(&observed_fault);
    let stream = open_f32_input_stream(
        &selected.info.device_id,
        move |_frames| {
            callback_counter.fetch_add(1, Ordering::Relaxed);
        },
        move |error, _detail| {
            let code = fault_code(&error);
            let _ = fault_slot.compare_exchange(0, code, Ordering::Relaxed, Ordering::Relaxed);
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "physical-removal stream open failed: {}",
            error.classifier()
        )
    });

    println!(
        "HIMSAT_GATE_E_ARMED={{\"path\":\"physical-removal\",\"device_selector\":\"configured\",\"wait_seconds\":{wait_seconds}}}"
    );
    let deadline = Instant::now() + Duration::from_secs(wait_seconds);
    while observed_fault.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);

    let callback_count = callbacks.load(Ordering::Relaxed);
    let code = observed_fault.load(Ordering::Relaxed);
    assert!(
        callback_count > 0,
        "physical-removal probe did not prove pre-removal live flow"
    );
    assert_eq!(
        code, 1,
        "physical removal must surface endpoint-unavailable; observed fault code {code}"
    );
    println!(
        "HIMSAT_GATE_E_RESULT={{\"path\":\"physical-removal\",\"outcome\":\"classified_fault\",\"classifier\":\"endpoint-unavailable\",\"callbacks_before_fault\":{callback_count}}}"
    );
}
