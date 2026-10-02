param(
    [string]$TestBinary = ".\himsat-core-tests.exe",
    [string]$ManifestPath = ".\manifest.json",
    [string]$EvidenceDir = ".\gate-e-evidence",
    [switch]$RunLiveSmoke,
    [switch]$ConsentLiveCapture,
    [switch]$RunResidualStorage,
    [switch]$RunResidualExclusive,
    [switch]$ExclusiveHolderConfirmed,
    [switch]$RunResidualMultiHour,
    [int]$MultiHourSeconds = 7200,
    [switch]$RunResidualRemoval,
    [int]$RemovalWaitSeconds = 90,
    [string]$PreferredDevice = "",
    [switch]$RequireSignal
)

$ErrorActionPreference = "Stop"

function Write-JsonFile {
    param([string]$Path, [object]$Value)
    $Value | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Path -Encoding utf8
}

function Invoke-HimsatTest {
    param(
        [string]$TestName,
        [string]$LogName
    )
    $logPath = Join-Path $EvidenceDir $LogName
    # Windows PowerShell 5.1 turns native stderr records into synthetic
    # NativeCommandError objects when they cross the PowerShell pipeline.
    # Merge stderr into stdout inside cmd.exe instead, preserving live output
    # (including HIMSAT_GATE_E_ARMED), the raw native text, and the real exit code.
    $resolvedTestBinary = (Resolve-Path -LiteralPath $TestBinary).Path
    $commandProcessor = (Get-Command cmd.exe -CommandType Application -ErrorAction Stop).Source
    $nativeCommand = ('"{0}" "{1}" --exact --nocapture 2>&1' -f $resolvedTestBinary, $TestName)
    & $commandProcessor /d /s /c $nativeCommand | Tee-Object -FilePath $logPath | Out-Host
    $exitCode = $LASTEXITCODE
    $logText = Get-Content -LiteralPath $logPath -Raw
    $gateResult = if ($logText -match 'HIMSAT_GATE_E_RESULT=(\{[^\r\n]+\})') {
        $Matches[1] | ConvertFrom-Json
    } else {
        $null
    }
    return [ordered]@{
        test = $TestName
        exit_code = $exitCode
        log = $LogName
        gate_e_result = $gateResult
    }
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw "Gate E hardware bundle must run on Windows."
}

if (Test-Path -LiteralPath $EvidenceDir) {
    $existingEvidence = @(Get-ChildItem -LiteralPath $EvidenceDir -Force)
    if ($existingEvidence.Count -ne 0) {
        throw "Evidence directory is not empty. Preserve the existing run and choose a new evidence directory: $EvidenceDir"
    }
} else {
    New-Item -ItemType Directory -Path $EvidenceDir | Out-Null
}

$manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
if (-not (Test-Path -LiteralPath $TestBinary)) {
    throw "Missing prebuilt test binary: $TestBinary"
}

$actualSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $TestBinary).Hash.ToLowerInvariant()
if ($manifest.test_binary_sha256 -ne $actualSha) {
    throw "Test binary digest mismatch. Expected $($manifest.test_binary_sha256), got $actualSha"
}

$audioService = Get-Service -Name Audiosrv -ErrorAction Stop
$pnpError = $null
$endpoints = @()
try {
    $endpoints = @(Get-PnpDevice -Class AudioEndpoint -PresentOnly -ErrorAction Stop)
} catch {
    $pnpError = $_.Exception.Message
}

$sanitizedEndpoints = @(
    $endpoints | ForEach-Object {
        [ordered]@{
            friendly_name = $_.FriendlyName
            status = [string]$_.Status
            class = [string]$_.Class
        }
    }
)

$testListPath = Join-Path $EvidenceDir "test-list.txt"
& $TestBinary --list *>&1 | Tee-Object -FilePath $testListPath | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "Prebuilt test binary could not list tests."
}

$micTest = "capture_windows::windows_tests::live_stream_open_reports_frames_or_classified_fault"
$loopbackTest = "capture_windows_system_audio::windows_tests::live_loopback_open_reports_frames_or_classified_fault"
$storageTest = "capture_windows_gate_e_residuals::measured_storage_threshold_refuses_at_real_host_free_space"
$exclusiveTest = "capture_windows_gate_e_residuals::live_expected_exclusive_mode_conflict_is_typed"
$multiHourTest = "capture_windows_gate_e_residuals::live_multi_hour_capture_reports_continuity"
$removalTest = "capture_windows_gate_e_residuals::live_physical_removal_reports_endpoint_unavailable"

$testList = Get-Content -LiteralPath $testListPath -Raw
foreach ($required in @($micTest, $loopbackTest, $storageTest, $exclusiveTest, $multiHourTest, $removalTest)) {
    if ($testList -notmatch [regex]::Escape($required)) {
        throw "Required Gate E test is absent from prebuilt binary: $required"
    }
}

$captureRequested = [bool]($RunLiveSmoke -or $RunResidualExclusive -or $RunResidualMultiHour -or $RunResidualRemoval)
$anyExecution = [bool]($captureRequested -or $RunResidualStorage)

$preflight = [ordered]@{
    schema = "himsat-gate-e-preflight-v1"
    source_sha = $manifest.source_sha
    test_binary_sha256 = $actualSha
    os = [System.Environment]::OSVersion.VersionString
    powershell = $PSVersionTable.PSVersion.ToString()
    audio_service = [ordered]@{
        name = $audioService.Name
        status = [string]$audioService.Status
    }
    endpoint_query = [ordered]@{
        count = $sanitizedEndpoints.Count
        error = $pnpError
        endpoints = $sanitizedEndpoints
    }
    live_smoke_requested = [bool]$RunLiveSmoke
    residuals_requested = [ordered]@{
        storage_threshold = [bool]$RunResidualStorage
        exclusive_mode = [bool]$RunResidualExclusive
        multi_hour = [bool]$RunResidualMultiHour
        physical_removal = [bool]$RunResidualRemoval
    }
    explicit_live_capture_consent = [bool]$ConsentLiveCapture
    generated_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
}
Write-JsonFile (Join-Path $EvidenceDir "preflight.json") $preflight

if (-not $anyExecution) {
    Write-Host "PREFLIGHT_COMPLETE"
    Write-Host "SOURCE_SHA=$($manifest.source_sha)"
    Write-Host "AUDIO_ENDPOINT_COUNT=$($sanitizedEndpoints.Count)"
    exit 0
}

if ($captureRequested -and -not $ConsentLiveCapture) {
    throw "Live capture qualification was requested without -ConsentLiveCapture."
}
if ($captureRequested -and $audioService.Status -ne "Running") {
    throw "Windows Audio service is not running."
}
if ($captureRequested -and $sanitizedEndpoints.Count -eq 0) {
    throw "No present AudioEndpoint device is visible to this session."
}

if ($RunLiveSmoke) {
    $results = @()

    $env:HIMSAT_GATE_E_REQUIRE_SIGNAL = "1"
    $env:HIMSAT_LIVE_MIC_TEST = "1"
    $results += Invoke-HimsatTest -TestName $micTest -LogName "live-mic.txt"
    Remove-Item Env:HIMSAT_LIVE_MIC_TEST -ErrorAction SilentlyContinue

    $env:HIMSAT_LIVE_LOOPBACK_TEST = "1"
    $results += Invoke-HimsatTest -TestName $loopbackTest -LogName "live-loopback.txt"
    Remove-Item Env:HIMSAT_LIVE_LOOPBACK_TEST -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_REQUIRE_SIGNAL -ErrorAction SilentlyContinue

    Write-JsonFile (Join-Path $EvidenceDir "live-smoke.json") ([ordered]@{
        schema = "himsat-gate-e-live-smoke-v1"
        source_sha = $manifest.source_sha
        test_binary_sha256 = $actualSha
        results = $results
        generated_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
    })

    if (($results | Where-Object { $_.exit_code -ne 0 }).Count -ne 0) {
        throw "One or more live Gate E smoke tests failed. Preserve the evidence directory."
    }

    Write-Host "LIVE_SMOKE_COMPLETE"
}

$residualResults = @()

if ($RunResidualStorage) {
    $evidenceRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($EvidenceDir))
    $drive = [System.IO.DriveInfo]::new($evidenceRoot)
    $freeBytes = [uint64]$drive.AvailableFreeSpace
    $env:HIMSAT_LIVE_STORAGE_THRESHOLD_TEST = "1"
    $env:HIMSAT_GATE_E_FREE_BYTES = [string]$freeBytes
    $residualResults += Invoke-HimsatTest -TestName $storageTest -LogName "residual-storage-threshold.txt"
    Remove-Item Env:HIMSAT_LIVE_STORAGE_THRESHOLD_TEST -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_FREE_BYTES -ErrorAction SilentlyContinue
}

if ($RunResidualExclusive) {
    if (-not $ExclusiveHolderConfirmed) {
        throw "-RunResidualExclusive requires -ExclusiveHolderConfirmed after an external client has acquired the selected endpoint exclusively."
    }
    $env:HIMSAT_LIVE_EXPECT_EXCLUSIVE_CONFLICT = "1"
    if ($PreferredDevice) {
        $env:HIMSAT_GATE_E_DEVICE_SELECTOR = $PreferredDevice
    }
    $residualResults += Invoke-HimsatTest -TestName $exclusiveTest -LogName "residual-exclusive-mode.txt"
    Remove-Item Env:HIMSAT_LIVE_EXPECT_EXCLUSIVE_CONFLICT -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_DEVICE_SELECTOR -ErrorAction SilentlyContinue
}

if ($RunResidualMultiHour) {
    if ($MultiHourSeconds -lt 7200 -or $MultiHourSeconds -gt 14400) {
        throw "-MultiHourSeconds must be between 7200 and 14400."
    }
    $env:HIMSAT_LIVE_MULTI_HOUR_TEST = "1"
    $env:HIMSAT_LIVE_MULTI_HOUR_SECS = [string]$MultiHourSeconds
    if ($PreferredDevice) {
        $env:HIMSAT_GATE_E_DEVICE_SELECTOR = $PreferredDevice
    }
    if ($RequireSignal) {
        $env:HIMSAT_GATE_E_REQUIRE_SIGNAL = "1"
    }
    $residualResults += Invoke-HimsatTest -TestName $multiHourTest -LogName "residual-multi-hour.txt"
    Remove-Item Env:HIMSAT_LIVE_MULTI_HOUR_TEST -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_LIVE_MULTI_HOUR_SECS -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_DEVICE_SELECTOR -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_REQUIRE_SIGNAL -ErrorAction SilentlyContinue
}

if ($RunResidualRemoval) {
    if (-not $PreferredDevice) {
        throw "-RunResidualRemoval requires -PreferredDevice for the physically detachable endpoint."
    }
    if ($RemovalWaitSeconds -lt 10 -or $RemovalWaitSeconds -gt 300) {
        throw "-RemovalWaitSeconds must be between 10 and 300."
    }
    $env:HIMSAT_LIVE_PHYSICAL_REMOVAL_TEST = "1"
    $env:HIMSAT_LIVE_PHYSICAL_REMOVAL_SECS = [string]$RemovalWaitSeconds
    $env:HIMSAT_GATE_E_DEVICE_SELECTOR = $PreferredDevice
    Write-Host "PHYSICAL_REMOVAL_WINDOW_ARMING"
    Write-Host "Detach the selected physical USB/Bluetooth endpoint only after the test prints HIMSAT_GATE_E_ARMED."
    $residualResults += Invoke-HimsatTest -TestName $removalTest -LogName "residual-physical-removal.txt"
    Remove-Item Env:HIMSAT_LIVE_PHYSICAL_REMOVAL_TEST -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_LIVE_PHYSICAL_REMOVAL_SECS -ErrorAction SilentlyContinue
    Remove-Item Env:HIMSAT_GATE_E_DEVICE_SELECTOR -ErrorAction SilentlyContinue
}

if ($residualResults.Count -ne 0) {
    Write-JsonFile (Join-Path $EvidenceDir "residuals.json") ([ordered]@{
        schema = "himsat-gate-e-residuals-v1"
        source_sha = $manifest.source_sha
        test_binary_sha256 = $actualSha
        results = $residualResults
        generated_at_utc = [DateTimeOffset]::UtcNow.ToString("o")
    })
    if (($residualResults | Where-Object { $_.exit_code -ne 0 }).Count -ne 0) {
        throw "One or more Gate E residual probes failed. Preserve the evidence directory."
    }
    Write-Host "RESIDUAL_PROBES_COMPLETE"
}
