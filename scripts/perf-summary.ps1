# Sums up darkshot-perf.log, the resource log `--trace` writes (see src/perf.rs).
#
#   scripts\perf-summary.ps1                  the log in the temp folder
#   scripts\perf-summary.ps1 path\to\perf.log another one
#
# For each run of the tray or of `--trace-cycles`: memory and handles at its first and
# last sample. Growth that keeps going over many captures is a leak; a step up after the
# first capture or two that then stays flat is not. For the capture windows: how long
# each took from start until it was on screen, and its memory and GPU memory then.

param([string]$Path = (Join-Path $env:TEMP 'darkshot-perf.log'))

if (-not (Test-Path $Path)) { Write-Error "No log at $Path. Run Darkshot with --trace first."; exit 1 }
$rows = Import-Csv $Path | ForEach-Object {
    foreach ($n in 'time_ms','private_kb','working_set_kb','peak_working_set_kb','cpu_ms','handles',
                   'gdi_objects','user_objects','gpu_local_kb','gpu_shared_kb','sample_us') {
        $_.$n = [double]$_.$n
    }
    $_
}

function Median($values) {
    $s = @($values | Sort-Object)
    if ($s.Count -eq 0) { return $null }
    if ($s.Count % 2) { $s[[int][math]::Floor($s.Count / 2)] } else { ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2 }
}

$mb = { param($kb) '{0:N1}' -f ($kb / 1024) }

'Long-running processes (tray, --trace-cycles): first sample -> last'
$rows | Where-Object role -in 'tray', 'cycles' | Group-Object pid | ForEach-Object {
    $first = $_.Group[0]; $last = $_.Group[-1]
    [pscustomobject]@{
        pid        = $_.Name
        role       = $first.role
        started    = [DateTimeOffset]::FromUnixTimeMilliseconds($first.time_ms).LocalDateTime
        samples    = $_.Count
        private_mb = "$(& $mb $first.private_kb) -> $(& $mb $last.private_kb)"
        handles    = "$($first.handles) -> $($last.handles)"
        gdi_user   = "$($first.gdi_objects)/$($first.user_objects) -> $($last.gdi_objects)/$($last.user_objects)"
        cpu_ms     = $last.cpu_ms
    }
} | Format-Table -AutoSize

$windows = $rows | Where-Object role -eq 'overlay' | Group-Object pid | ForEach-Object {
    $start = $_.Group | Where-Object event -eq 'start' | Select-Object -First 1
    $shown = $_.Group | Where-Object event -eq 'shown' | Select-Object -First 1
    if ($start -and $shown) {
        [pscustomobject]@{
            to_screen_ms  = $shown.time_ms - $start.time_ms
            private_mb    = $shown.private_kb / 1024
            peak_ws_mb    = ($_.Group | Measure-Object peak_working_set_kb -Maximum).Maximum / 1024
            gpu_local_mb  = $shown.gpu_local_kb / 1024
            gpu_shared_mb = $shown.gpu_shared_kb / 1024
        }
    }
}
"Capture windows: $(@($windows).Count) (medians)"
if ($windows) {
    [pscustomobject]@{
        to_screen_ms  = Median ($windows.to_screen_ms)
        private_mb    = '{0:N1}' -f (Median ($windows.private_mb))
        peak_ws_mb    = '{0:N1}' -f (Median ($windows.peak_ws_mb))
        gpu_local_mb  = '{0:N1}' -f (Median ($windows.gpu_local_mb))
        gpu_shared_mb = '{0:N1}' -f (Median ($windows.gpu_shared_mb))
    } | Format-Table -AutoSize
}

"Cost of sampling: median $(Median ($rows.sample_us)) us, max $(($rows | Measure-Object sample_us -Maximum).Maximum) us per sample"
