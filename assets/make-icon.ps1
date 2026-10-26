# Genera assets/pega.ico y assets/pega-64.png a partir de una imagen cuadrada de borde a borde.
# Uso: powershell -File assets/make-icon.ps1 ruta\a\imagen.png
# Recorta las esquinas (cuadrado redondeado, como los íconos de Windows 11) con transparencia
# y arma el .ico con los tamaños que pide Windows (barra de tareas, bandeja, Explorador).
param([Parameter(Mandatory)] [string] $Source, [string] $OutDir)

Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'
$here = if ($OutDir) { $OutDir } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$src = [Drawing.Image]::FromFile((Resolve-Path $Source))
$side = [Math]::Min($src.Width, $src.Height)
$cropX = [int](($src.Width - $side) / 2); $cropY = [int](($src.Height - $side) / 2)

function Render([int]$size) {
    $bmp = New-Object Drawing.Bitmap $size, $size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'; $g.CompositingQuality = 'HighQuality'
    $g.Clear([Drawing.Color]::Transparent)
    # Cuadrado redondeado que llena todo el cuadro (radio ~22 %, como los de Windows 11).
    $r = [single]($size * 0.22); $d = 2 * $r; $s = [single]$size
    $path = New-Object Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $d, $d, 180, 90); $path.AddArc($s - $d, 0, $d, $d, 270, 90)
    $path.AddArc($s - $d, $s - $d, $d, $d, 0, 90); $path.AddArc(0, $s - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $brush = New-Object Drawing.TextureBrush($src, (New-Object Drawing.Rectangle $cropX, $cropY, $side, $side))
    $brush.WrapMode = 'Clamp'
    $brush.ScaleTransform([single]($size / $side), [single]($size / $side))
    $g.FillPath($brush, $path)
    $g.Dispose()
    $ms = New-Object IO.MemoryStream
    $bmp.Save($ms, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    , $ms.ToArray()
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$pngs = @{}; foreach ($s in $sizes) { $pngs[$s] = Render $s }
[IO.File]::WriteAllBytes((Join-Path $here 'pega-64.png'), $pngs[64])

# .ico con entradas PNG (Windows Vista en adelante).
$ms = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter $ms
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($s in $sizes) {
    $dim = if ($s -ge 256) { 0 } else { $s }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$pngs[$s].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$s].Length
}
foreach ($s in $sizes) { $w.Write($pngs[$s]) }
$w.Flush(); [IO.File]::WriteAllBytes((Join-Path $here 'pega.ico'), $ms.ToArray())
$src.Dispose()
"pega.ico ({0:N0} KB) y pega-64.png listos" -f ((Get-Item (Join-Path $here 'pega.ico')).Length / 1KB)
