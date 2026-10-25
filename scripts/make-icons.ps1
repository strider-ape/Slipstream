# Draws the Slipstream icon (white download arrow on a blue circle) at every size we ship:
#   assets/icons/slipstream-<size>.png   window, Linux menus, browser extension
#   assets/slipstream.ico                 Windows programs, installer, tray
# Keep the shapes in step with assets/icon.svg. Run with Windows PowerShell:
#   powershell -ExecutionPolicy Bypass -File scripts\make-icons.ps1
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $root 'assets\icons'
New-Item -ItemType Directory -Force $dir | Out-Null

function Draw([int]$n) {
    $bmp = New-Object System.Drawing.Bitmap $n, $n, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    # Draw on a 128×128 grid, like the SVG, scaled to this size.
    $s = $n / 128.0
    $g.ScaleTransform($s, $s)
    $g.FillEllipse((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(37, 99, 235))), 1, 1, 126, 126)
    # Thin lines vanish at small sizes: keep the stroke at least 2 pixels wide.
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), ([Math]::Max(11.0, 2.0 / $s))
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'
    $g.DrawLine($pen, 64, 30, 64, 80)
    $g.DrawLines($pen, [System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF 42, 58),
        (New-Object System.Drawing.PointF 64, 80),
        (New-Object System.Drawing.PointF 86, 58)))
    $g.DrawLine($pen, 38, 96, 90, 96)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}

# The sizes Windows asks for at 100-200% display scaling.
$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$keep = 16, 32, 48, 128, 256
$images = @{}
foreach ($n in $sizes) {
    $images[$n] = Draw $n
    if ($keep -contains $n) { [IO.File]::WriteAllBytes((Join-Path $dir "slipstream-$n.png"), $images[$n]) }
}

# An .ico file: a 6-byte header, a 16-byte entry per size, then the PNG images.
$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ms
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($n in $sizes) {
    $w.Write([byte]($n % 256)); $w.Write([byte]($n % 256))   # 0 means 256
    $w.Write([byte]0); $w.Write([byte]0)                     # no palette, reserved
    $w.Write([UInt16]1); $w.Write([UInt16]32)                # planes, bits per pixel
    $w.Write([UInt32]$images[$n].Length); $w.Write([UInt32]$offset)
    $offset += $images[$n].Length
}
foreach ($n in $sizes) { $w.Write($images[$n]) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $root 'assets\slipstream.ico'), $ms.ToArray())
Get-ChildItem (Join-Path $root 'assets') -Recurse -File | Format-Table Name, Length
