<#
.SYNOPSIS
    Regenerates assets\notch.ico (16-256 px): a blue-violet tile with the black
    notch and its two eyes. Sizes up to 64 px are stored as 32-bit DIBs, 256 px
    as PNG, which is what Windows, windres and NSIS expect.
#>
param([string]$Out = (Join-Path $PSScriptRoot "notch.ico"))

Add-Type -AssemblyName System.Drawing

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    return $path
}

# Notch attached to y = $top: concave shoulders, rounded bottom corners.
function New-NotchPath([float]$cx, [float]$top, [float]$w, [float]$h, [float]$s, [float]$br) {
    $l = $cx - $w / 2
    $r = $cx + $w / 2
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddLine($l - $s, $top, $r + $s, $top)
    $path.AddArc($r, $top, 2 * $s, 2 * $s, 270, -90)
    $path.AddArc($r - 2 * $br, $top + $h - 2 * $br, 2 * $br, 2 * $br, 0, 90)
    $path.AddArc($l, $top + $h - 2 * $br, 2 * $br, 2 * $br, 90, 90)
    $path.AddArc($l - 2 * $s, $top, 2 * $s, 2 * $s, 0, -90)
    $path.CloseFigure()
    return $path
}

function New-IconBitmap([int]$size) {
    $s = [float]$size
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    $tile = New-RoundedRect 0 0 $s $s (0.22 * $s)
    $tileBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF(0, 0)),
        (New-Object System.Drawing.PointF($s, $s)),
        [System.Drawing.Color]::FromArgb(255, 167, 182, 255),
        [System.Drawing.Color]::FromArgb(255, 79, 70, 229))
    $g.FillPath($tileBrush, $tile)

    # Soft glow under the notch.
    $glowRect = New-Object System.Drawing.RectangleF((0.2 * $s), (0.2 * $s), (0.6 * $s), (0.14 * $s))
    $glow = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $glowRect,
        [System.Drawing.Color]::FromArgb(150, 34, 211, 238),
        [System.Drawing.Color]::FromArgb(150, 168, 85, 247),
        [System.Drawing.Drawing2D.LinearGradientMode]::Horizontal)
    $g.FillEllipse($glow, $glowRect)

    $notch = New-NotchPath (0.5 * $s) 0 (0.54 * $s) (0.27 * $s) (0.07 * $s) (0.115 * $s)
    $g.FillPath([System.Drawing.Brushes]::Black, $notch)

    $eyeW = [Math]::Max(1.2, 0.062 * $s)
    $eyeH = [Math]::Max(2.0, 0.115 * $s)
    foreach ($dx in @(-0.085, 0.085)) {
        $ex = (0.5 + $dx) * $s - $eyeW / 2
        $ey = 0.135 * $s - $eyeH / 2
        $eye = New-RoundedRect $ex $ey $eyeW $eyeH ($eyeW / 2)
        $g.FillPath([System.Drawing.Brushes]::White, $eye)
    }
    $g.Dispose()
    return $bmp
}

function Get-DibBytes([System.Drawing.Bitmap]$bmp) {
    $size = $bmp.Width
    $rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $pixels = New-Object byte[] ($data.Stride * $size)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
    $bmp.UnlockBits($data)
    $maskStride = [int]([Math]::Ceiling($size / 32.0) * 4)
    $ms = New-Object System.IO.MemoryStream
    $w = New-Object System.IO.BinaryWriter($ms)
    # BITMAPINFOHEADER; height counts the XOR and AND bitmaps.
    $w.Write([int]40); $w.Write([int]$size); $w.Write([int]($size * 2))
    $w.Write([int16]1); $w.Write([int16]32); $w.Write([int]0)
    $w.Write([int]($size * $size * 4 + $maskStride * $size))
    $w.Write([int]0); $w.Write([int]0); $w.Write([int]0); $w.Write([int]0)
    for ($y = $size - 1; $y -ge 0; $y--) {
        $w.Write($pixels, $y * $data.Stride, $size * 4)
    }
    $w.Write((New-Object byte[] ($maskStride * $size)))
    $w.Flush()
    # The unary comma keeps PowerShell from unrolling the array.
    return , $ms.ToArray()
}

function Get-PngBytes([System.Drawing.Bitmap]$bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 256)
$images = New-Object 'System.Collections.Generic.List[byte[]]'
foreach ($size in $sizes) {
    $bmp = New-IconBitmap $size
    if ($size -ge 256) { $images.Add((Get-PngBytes $bmp)) } else { $images.Add((Get-DibBytes $bmp)) }
    $bmp.Dispose()
}

$icoStream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($icoStream)
$writer.Write([int16]0); $writer.Write([int16]1); $writer.Write([int16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $writer.Write([byte]$dim); $writer.Write([byte]$dim); $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([int16]1); $writer.Write([int16]32)
    $writer.Write([int]$images[$i].Length); $writer.Write([int]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write($image) }
$writer.Flush()
[System.IO.File]::WriteAllBytes($Out, $icoStream.ToArray())
"wrote $Out ($($icoStream.Length) bytes, sizes $($sizes -join ', '))"
