# 008J — Gate E runtime evidence reconciliation

Status: `CANDIDATE_RECONCILIATION_ONLY`.

This record reconciles the canonical 008H/008I corrective lineage with the completed 7200-second Windows Gate E run. It does not close `O008`, does not close Specification 008, and does not authorize Specification 009.

## Exact canonical identity

```text
BASE_SHA = 629cc078abf47fd97e2cee07e1e428be5cc4ff90
BASE_TREE = c0b56b8d0d6ff98c01a0091ba1f664e56c5cdea1
PR_233_HEAD = 1331c4e5770138361fe07cbe2f9d34e4bf56d6b6
PR_233_MERGE = 4485f6d927bc2ff6d0649df11084c2d7337524cf
PR_233_POSTMERGE_CI = 37079192138 SUCCESS
PR_233_POSTMERGE_R3 = 37079192243 SUCCESS
PR_234_HEAD = 3ca95e95cef08fe1fda290bf1b606dcd03e6e0d5
PR_234_MERGE = 629cc078abf47fd97e2cee07e1e428be5cc4ff90
PR_234_POSTMERGE_CI = 37130270548 SUCCESS
PR_234_POSTMERGE_R3 = 37130270564 SUCCESS
```

PR #233 made WASAPI `Xrun` / data-discontinuity evidence non-terminal without hiding it. PR #234 then reconciled the multi-hour acceptance logic with the Specification 008 requirement of zero unexplained loss rather than zero discontinuities.
## Canonical bundle and preflight

```text
ARTIFACT = himsat-gate-e-windows-629cc078abf47fd97e2cee07e1e428be5cc4ff90
SOURCE_SHA = 629cc078abf47fd97e2cee07e1e428be5cc4ff90
TEST_BINARY_SHA256 = 64c58aa886a98a7231203382806e24342089732c3320505aa8447985a9da4fcd
OS = Microsoft Windows NT 10.0.26300.0
POWERSHELL = 5.1.26100.9549
AUDIO_SERVICE = Audiosrv Running
AUDIO_ENDPOINT_COUNT = 4
LIVE_CAPTURE_CONSENT = true
```

The manifest source SHA matched canonical `main`, and an independent SHA-256 calculation matched the manifest before execution. Preflight recorded four present AudioEndpoint devices. No microphone audio content was persisted; the evidence contains only qualification metadata and counters.

## 7200-second multi-hour qualification

The canonical post-merge bundle ran the exact `capture_windows_gate_e_residuals::live_multi_hour_capture_reports_continuity` test against `Onboard MIC`. `-RequireSignal` was not asserted. A process-level execution-state keeper prevented sleep without permanently changing the Balanced power plan; no sleep/resume or Kernel-Power sleep event occurred inside the measurement window.
```text
MULTIHOUR_STATUS = PASS
START = 2026-10-03T18:05:20.7201821+03:00
FINISH = 2026-10-03T20:05:25.5176869+03:00
REQUIRED_SECONDS = 7200
MEASURED_SECONDS = 7200
TEST_WALL_RUNTIME_SECONDS = 7202.93
CALLBACKS = 720004
FRAMES = 345600000
EXPECTED_FRAMES = 345600000
CALLBACK_TOLERANCE_FRAMES = 480
NON_SILENT_FRAMES = 107403936
WARMUP_DISCONTINUITIES = 2
RUNTIME_DISCONTINUITIES = 8
ROUTE_REROUTES = 0
TERMINAL_RUNTIME_ERRORS = 0
UNEXPLAINED_SHORTFALL = 0
EXIT_CODE = 0
```

The eight measured-window data discontinuities remain explicit evidence. They are neither erased nor treated as an automatic pass. The run passes because capture continued for the full required duration, callback progress did not stall beyond the canonical bound, terminal errors and reroutes remained zero, delivered frames exactly equalled wall-clock expected frames, and unexplained shortfall was zero even before applying the one-callback settlement tolerance.
## Preserved historical failure

The earlier `4485f6d927bc2ff6d0649df11084c2d7337524cf` multi-hour attempt remains historical `FAIL` evidence. It stopped after about 170.66 seconds because the older harness treated a measured-window data discontinuity as immediate failure (`left 3`, `right 2`, exit 101). That result is not overwritten or relabelled.

## Other live Gate E rows now proven

The following previously executed rows remain valid evidence at their recorded exact bundle revisions:

- microphone first-audio and repeated strict signal passes;
- loopback first-audio with deliberate-signal differential and honest-silence controls;
- privacy revoke fail-closed behavior plus restore recovery;
- real Modern Standby suspend/resume plus post-resume microphone and loopback recovery;
- 60-second sustained microphone hold with mid-hold pause/resume;
- live microphone-to-journal round-trip through 008E -> 008D -> 005A/005B with byte-exact decrypt;
- measured host storage-threshold refusal as typed `storage-critical`;
- real external exclusive-mode holder producing typed `exclusive-mode-conflict`, followed by microphone recovery with non-silent frames.

The Device Manager disable experiment remains degraded-PnP robustness evidence only because Windows kept the engine endpoint usable. It is not physical-removal evidence.
## Remaining O008 frontier

```text
MULTI_HOUR_CAPTURE = PASS
EXCLUSIVE_MODE_LIVE = PASS
EXCLUSIVE_MODE_RECOVERY = PASS
MEASURED_STORAGE_THRESHOLD_REFUSAL = PASS
TRUE_PHYSICAL_REMOVAL = NOT_PROVEN
USB_BT_ATTACH_DETACH = UNAVAILABLE
LITERAL_HOST_VOLUME_EXHAUSTION = NOT_PROVEN
```

The live host exposed only INTELAUDIO endpoints during the final inventory; no USB/Bluetooth audio endpoint was available. The prior physical-removal attempt armed after proven callback flow but timed out with fault code 0 because the selected endpoint remained present. No `endpoint-unavailable` event was observed.

The canonical Gate E runbook still names `real storage-exhaustion refusal` separately. The measured-threshold probe is therefore preserved as PASS for the real measured policy/refusal boundary but is not silently promoted to literal host-volume exhaustion. A later closeout may remove this residual only through canonical policy interpretation or genuine safe disposable-volume evidence.

`O008` remains OPEN. Specification 008 remains OPEN. Specification 009 shaping remains unauthorized.

## Off-repository raw evidence

The exact 7200-second raw artifacts are retained on the founder-authorized Windows host under:

```text
C:\Users\Shehr\AppData\Local\Temp\opencode\ottari-gate-e-629cc078-20261003\evidence-7200
```

The repository record intentionally captures only the bounded metrics and hashes required for qualification; it does not import raw audio or machine-unique endpoint identifiers.