# 008H — WASAPI xrun reconciliation and Gate E corrective grain

Status: `CANDIDATE_IMPLEMENTED_NOT_YET_CANONICAL_QUALIFIED`.
This record binds the real-hardware observations that exposed the defect to the bounded corrective implementation. It does not close `O008`, does not claim physical-removal or multi-hour PASS, and does not authorize Specification 009.

## Candidate identity

```text
BASE_SHA = 4632d2092190f16204eb94b7026cb16133080772
BASE_CI = 36238697504 SUCCESS
BASE_R3 = 36238697462 SUCCESS
BRANCH = fix/008h-wasapi-xrun-gate-e
BUNDLE_SHA256 = 02b22414218b36432c26157167af8872d99a15beebbb4d82955a5d15a15572f6
HOST = Microsoft Windows NT 10.0.26300.0
```

The pre-existing local `main` worktree was left untouched. This grain was authored in a clean worktree created directly from exact `origin/main`.

## New real-hardware evidence on the base binary

Measured host storage-threshold refusal is PASS on the exact base bundle:

```text
path = storage-threshold
outcome = refused
classifier = storage-critical
free_bytes = 22233206784
exit_code = 0
```
Evidence directory: `C:\Users\Shehr\AppData\Local\Temp\opencode\himsat-gate-e-storage-4632d209-20261003b`.
A later runner-regression execution independently reproduced the same typed refusal at measured `free_bytes = 20753076224` with the candidate PowerShell runner and an empty `$env:OS`.

Live exclusive-mode contention is PASS on the exact base bundle. An independent Windows Core Audio client acquired the default capture endpoint with `IAudioClient` exclusive mode using PCM stereo, 48 kHz, 16-bit. The holder reported:

```text
EXCLUSIVE_INIT_HRESULT = 0x00000000
EXCLUSIVE_HOLDER_READY = PCM,2,48000,16
```

While that independent holder remained live, Himsat returned:

```text
path = exclusive-mode
outcome = classified_fault
classifier = exclusive-mode-conflict
exit_code = 0
```

After the holder process was terminated, the strict microphone control returned `frames` with `non_silent_frames = 1920`, proving recovery rather than a persistent capture failure.

Durable local evidence directory: `C:\Users\Shehr\AppData\Local\Temp\opencode\himsat-gate-e-exclusive-durable-4632d209-20261003`.

## Defect exposed by residual execution

A detachable headset input appeared as a distinct Windows audio endpoint and passed a 30-second sustained-flow control (`callbacks = 2941`, `samples = 2822592`, `non_silent_samples = 960`). The canonical physical-removal harness nevertheless failed before any detach with fault code 7 (`StreamFault`).
The canonical multi-hour harness likewise failed in under one second on both the headset target and the explicit onboard endpoint, reporting runtime stream errors while other live controls continued to deliver frames.

Inspection of the pinned `cpal 0.18.2` WASAPI backend established the causal mismatch: `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` emits `cpal::ErrorKind::Xrun`, after which CPAL continues input processing. The Himsat adapter previously mapped every unlisted cpal error kind, including `Xrun`, to terminal-looking `StreamFault(Play)` and runtime `Interrupted(RouteChanged)` semantics.

The evidence therefore supports a corrective adapter/harness grain rather than marking either failed residual as a hardware failure or PASS.

## Corrective contract

008H keeps the closed portable 006 state/health contract unchanged and adds no dependency. On Windows only:

- `cpal::ErrorKind::Xrun` maps to typed `data-discontinuity` for microphone and loopback;
- `data-discontinuity` is non-terminal for the session machine and has no fabricated 006 health reason;
- the condition remains explicit evidence for loss accounting;
- the multi-hour harness uses a two-second settled warm-up, then requires zero measured-window discontinuities, zero terminal runtime errors, callback progress, expected per-channel frame delivery, zero unexplained shortfall, and signal when strict signal is requested;
- the physical-removal harness proves at least ten callbacks before `HIMSAT_GATE_E_ARMED`, records discontinuities/reroutes separately, and still passes only on typed `endpoint-unavailable`;
- the PowerShell runner resolves Windows from the platform API rather than `$env:OS` and preserves native stderr as raw streamed evidence without PowerShell `NativeCommandError` injection.

No physical-removal or multi-hour result is promoted by this implementation alone. Both require execution against the exact post-merge 008H bundle.
## Candidate-local qualification

```text
cargo fmt --all -- --check = PASS
python tools/provenance_gate.py validate = PASS
python tools/provenance_gate.py check-generated = PASS
OPENSSL_RUST_USE_NASM=0 python tools/004p_dependency_closure.py = PASS
git diff --check = PASS
```

Local `cargo check -p himsat-core --all-targets` is `ENVIRONMENT_BLOCKED`, not PASS: the vendored OpenSSL build first lacked `perl`; Git-for-Windows Perl was then found without installation, but that Perl distribution lacks `Locale::Maketext::Simple`, so OpenSSL Configure cannot complete locally. No dependency, gate, or test was weakened to bypass that host limitation. Exact-head GitHub CI must compile and test the candidate on Windows, macOS, and Ubuntu before merge.

PowerShell runner regression checks used the exact canonical base binary:

- with `$env:OS` absent, the candidate runner recognized real Windows and completed the storage probe with `residuals.json`;
- a deliberately failing canonical multi-hour probe produced `exit_code = 101`, preserved `residuals.json`, and the raw log contained no synthetic `NativeCommandError` after stderr merging was moved below the PowerShell pipeline.

## Remaining authority

`O008` remains OPEN. Existing privacy, suspend/resume, first-audio, sustained, journal round-trip, exclusive-mode, and measured-storage-threshold evidence is preserved. Physical removal and multi-hour capture remain unproven pending exact-head 008H qualification plus real-hardware reruns. Literal host-volume exhaustion is not claimed by the measured-threshold probe. Specification 009 remains unauthorized.
