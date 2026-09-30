# Mide la app de verdad (con ventana), con las carpetas del banco de pruebas.
#
# Primero hay que generar las carpetas con el banco de pruebas:
#   cargo test --release rendimiento -- --ignored --nocapture
# y despues:
#   powershell -File tools\medir-ventana.ps1                  (usa target\release\nodex-notes.exe)
#   powershell -File tools\medir-ventana.ps1 -Exe "C:\Program Files\Notas\Notas.exe" -Sizes 1000,10000
#
# Para cada carpeta abre la app dos veces (en Inicio y en una nota) y mide:
#   Listo      = segundos hasta que la ventana termino su primer cuadro (cambia el titulo)
#   RAM        = memoria del proceso a los 5 segundos
#   CPU quieta = % de un nucleo que usa sin tocarla, durante 10 segundos
# Usa una carpeta de configuracion temporal: nunca toca la configuracion real.

param(
    [string]$Exe = "",
    [string]$Sizes = "1000,5000,10000,50000"
)

$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo "target\release\nodex-notes.exe" }
if (-not (Test-Path $Exe)) { Write-Error "No existe $Exe (compila con: cargo build --release)"; exit 1 }
$bench = Join-Path $env:TEMP "nodex-bench"
$today = Get-Date -Format "yyyy-MM-dd"
$cal = [Globalization.CultureInfo]::InvariantCulture.Calendar
$week = "{0}-W{1:D2}" -f (Get-Date).Year, $cal.GetWeekOfYear((Get-Date), [Globalization.CalendarWeekRule]::FirstFourDayWeek, [DayOfWeek]::Monday)
$rows = @()

foreach ($n in ($Sizes -split ',' | ForEach-Object { [int]$_.Trim() })) {
    $vault = Join-Path $bench "v$n"
    if (-not (Test-Path (Join-Path $vault ".banco"))) { Write-Warning "Falta la carpeta de $n notas: corre primero el banco de pruebas"; continue }
    $cfg = Join-Path $bench "ventana$n"
    Remove-Item $cfg -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $cfg | Out-Null
    $vaultToml = $vault.Replace('\', '/')
    [IO.File]::WriteAllText((Join-Path $cfg "config.toml"), "carpeta_notas = '$vaultToml'`nproveedor = `"ollama`"`nmodelo = `"x`"`nia_automatica = false`n")
    $note = (Get-ChildItem (Join-Path $vault "General") -Filter *.md | Where-Object { $_.BaseName -notmatch '^\d{4}-\d\d-\d\d$' } | Select-Object -First 1).BaseName

    foreach ($view in @("inicio", "nota")) {
        $tab = if ($view -eq "inicio") { "vista:inicio" } else { "nota:General/$note" }
        $estado = "espacio = `"General`"`nnota = `"General/$note`"`nhoy = `"$today`"`nsemana = `"$week`"`npestanas = [`"$tab`"]`npestana = 0`n"
        [IO.File]::WriteAllText((Join-Path $cfg "estado.toml"), $estado)
        $env:NODEX_CONFIG_DIR = $cfg
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process $Exe -PassThru
        $ready = $null
        while ($sw.ElapsedMilliseconds -lt 300000) {
            $p.Refresh()
            # El titulo cambia de "Notas" a "<nota> - Notas" al terminar el primer cuadro.
            if ($p.MainWindowTitle -like "*Notas" -and $p.MainWindowTitle -ne "Notas") { $ready = $sw.ElapsedMilliseconds; break }
            Start-Sleep -Milliseconds 5
        }
        Start-Sleep -Seconds 5
        $p.Refresh()
        $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
        Start-Sleep -Seconds 10
        $p.Refresh()
        $cpu1 = $p.TotalProcessorTime.TotalMilliseconds
        $rows += [pscustomobject]@{
            Notas      = $n
            Vista      = $view
            Listo_s    = if ($ready) { [math]::Round($ready / 1000, 2) } else { "> 300" }
            RAM_MB     = [math]::Round($p.WorkingSet64 / 1MB)
            CPU_quieta = "{0:N1} %" -f (($cpu1 - $cpu0) / 10000 * 100)
        }
        Stop-Process -Id $p.Id -Force
        Start-Sleep -Seconds 2
    }
    Remove-Item $cfg -Recurse -Force -ErrorAction SilentlyContinue
}
Remove-Item Env:\NODEX_CONFIG_DIR -ErrorAction SilentlyContinue
$rows | Format-Table -AutoSize
