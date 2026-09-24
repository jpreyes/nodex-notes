# Genera los íconos de la app en assets/ (Windows, PowerShell):
#   powershell -ExecutionPolicy Bypass -File packaging/make-icon.ps1 -OutDir assets
param([string]$OutDir)
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $OutDir | Out-Null

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = 2 * $r
  $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure(); return $p
}

$N = 1024
$bmp = New-Object System.Drawing.Bitmap $N, $N, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'; $g.Clear([System.Drawing.Color]::Transparent)

# Fondo: cuadrado redondeado azul con un degradado suave
$bg = RoundRect 48 48 928 928 210
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush ([System.Drawing.Point]::new(0, 48)), ([System.Drawing.Point]::new(0, 976)), ([System.Drawing.Color]::FromArgb(255, 64, 132, 240)), ([System.Drawing.Color]::FromArgb(255, 34, 88, 196))
$g.FillPath($grad, $bg)

# Hoja blanca con la esquina superior derecha doblada
$fold = 120
$page = New-Object System.Drawing.Drawing2D.GraphicsPath
$px = 292; $py = 214; $pw = 440; $ph = 596; $pr = 44
$page.AddArc($px, $py, 2*$pr, 2*$pr, 180, 90)
$page.AddLine($px + $pr, $py, $px + $pw - $fold, $py)
$page.AddLine($px + $pw - $fold, $py, $px + $pw, $py + $fold)
$page.AddArc($px + $pw - 2*$pr, $py + $ph - 2*$pr, 2*$pr, 2*$pr, 0, 90)
$page.AddArc($px, $py + $ph - 2*$pr, 2*$pr, 2*$pr, 90, 90)
$page.CloseFigure()
$shadow = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(45, 10, 30, 80))
$m = New-Object System.Drawing.Drawing2D.Matrix; $m.Translate(0, 14)
$sp = $page.Clone(); $sp.Transform($m); $g.FillPath($shadow, $sp)
$g.FillPath([System.Drawing.Brushes]::White, $page)
$foldPath = New-Object System.Drawing.Drawing2D.GraphicsPath
$foldPath.AddPolygon(@([System.Drawing.PointF]::new($px + $pw - $fold, $py), [System.Drawing.PointF]::new($px + $pw - $fold, $py + $fold - 18), [System.Drawing.PointF]::new($px + $pw, $py + $fold)))
$g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 200, 218, 247))), $foldPath)

# Tres líneas de texto
$pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 150, 180, 232)), 38
$pen.StartCap = 'Round'; $pen.EndCap = 'Round'
$g.DrawLine($pen, 372, 420, 652, 420)
$g.DrawLine($pen, 372, 516, 652, 516)
$g.DrawLine($pen, 372, 612, 540, 612)

# Chispa (IA): estrella de 4 puntas amarilla con borde blanco
function Star([float]$cx, [float]$cy, [float]$outer, [float]$inner) {
  $pts = @()
  for ($i = 0; $i -lt 8; $i++) {
    $ang = [math]::PI / 4 * $i - [math]::PI / 2
    $rad = if ($i % 2 -eq 0) { $outer } else { $inner }
    $pts += [System.Drawing.PointF]::new($cx + $rad * [math]::Cos($ang), $cy + $rad * [math]::Sin($ang))
  }
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath; $p.AddPolygon($pts); return $p
}
$star = Star 716 742 150 46
$outline = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), 30
$outline.LineJoin = 'Round'
$g.DrawPath($outline, $star)
$g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 255, 196, 46))), $star)
$g.Dispose()
$bmp.Save((Join-Path $OutDir "icon-1024.png"), [System.Drawing.Imaging.ImageFormat]::Png)

# Tamaños más chicos (reducción de alta calidad)
$pngs = @{}
foreach ($s in 16, 24, 32, 48, 64, 128, 256, 512) {
  $b = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gg = [System.Drawing.Graphics]::FromImage($b)
  $gg.InterpolationMode = 'HighQualityBicubic'; $gg.SmoothingMode = 'AntiAlias'; $gg.PixelOffsetMode = 'HighQuality'
  $gg.DrawImage($bmp, 0, 0, $s, $s); $gg.Dispose()
  $ms = New-Object System.IO.MemoryStream; $b.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  $pngs[$s] = $ms.ToArray()
  if ($s -in 64, 256, 512) { [IO.File]::WriteAllBytes((Join-Path $OutDir "icon-$s.png"), $pngs[$s]) }
}

# .ico de Windows con imágenes PNG de 16 a 256
$sizes = 16, 24, 32, 48, 64, 128, 256
$ico = New-Object System.IO.MemoryStream; $w = New-Object System.IO.BinaryWriter $ico
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($s in $sizes) {
  $dim = if ($s -ge 256) { 0 } else { $s }
  $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([UInt16]1); $w.Write([UInt16]32); $w.Write([UInt32]$pngs[$s].Length); $w.Write([UInt32]$offset)
  $offset += $pngs[$s].Length
}
foreach ($s in $sizes) { $w.Write($pngs[$s]) }
$w.Flush(); [IO.File]::WriteAllBytes((Join-Path $OutDir "icon.ico"), $ico.ToArray())
Get-ChildItem $OutDir | ForEach-Object { "$($_.Name)  $($_.Length) bytes" }
