# Draws B-Installer's icon, assets\bi.ico: a green rounded square with an arrow going down into
# a tray. Every size is drawn on its own (not scaled down from 256) so the small ones stay sharp.
#
#   powershell -ExecutionPolicy Bypass -File assets\make-icon.ps1
Add-Type -AssemblyName System.Drawing

function Draw([int] $size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $u = $size / 16.0

    # The tile.
    $inset = [math]::Max(0.5, 0.5 * $u)
    $r = 3.5 * $u
    $box = New-Object System.Drawing.RectangleF $inset, $inset, ($size - 2 * $inset), ($size - 2 * $inset)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($box.X, $box.Y, 2 * $r, 2 * $r, 180, 90)
    $path.AddArc($box.Right - 2 * $r, $box.Y, 2 * $r, 2 * $r, 270, 90)
    $path.AddArc($box.Right - 2 * $r, $box.Bottom - 2 * $r, 2 * $r, 2 * $r, 0, 90)
    $path.AddArc($box.X, $box.Bottom - 2 * $r, 2 * $r, 2 * $r, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 31, 122, 92))), $path)

    # The arrow and the tray, in white.
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), ([math]::Max(1.5, 1.6 * $u))
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'
    $g.DrawLine($pen, 8 * $u, 3.6 * $u, 8 * $u, 9.4 * $u)
    $g.DrawLines($pen, [System.Drawing.PointF[]] @(
        (New-Object System.Drawing.PointF (5.2 * $u), (6.8 * $u)),
        (New-Object System.Drawing.PointF (8 * $u), (9.6 * $u)),
        (New-Object System.Drawing.PointF (10.8 * $u), (6.8 * $u))))
    $g.DrawLines($pen, [System.Drawing.PointF[]] @(
        (New-Object System.Drawing.PointF (4 * $u), (10 * $u)),
        (New-Object System.Drawing.PointF (4 * $u), (12.2 * $u)),
        (New-Object System.Drawing.PointF (12 * $u), (12.2 * $u)),
        (New-Object System.Drawing.PointF (12 * $u), (10 * $u))))
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $ms.ToArray()
}

# An .ico of PNG images (fine for every size since Windows Vista).
$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$images = $sizes | ForEach-Object { , (Draw $_) }
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16] 0); $w.Write([uint16] 1); $w.Write([uint16] $sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i] % 256  # 0 means 256
    $w.Write([byte] $s); $w.Write([byte] $s); $w.Write([byte] 0); $w.Write([byte] 0)
    $w.Write([uint16] 1); $w.Write([uint16] 32)
    $w.Write([uint32] $images[$i].Length); $w.Write([uint32] $offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write($img) }
[System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot 'bi.ico'), $out.ToArray())
Set-Content -Path (Join-Path $PSScriptRoot 'bi-256.png') -Value $images[-1] -Encoding Byte
"Wrote $(Join-Path $PSScriptRoot 'bi.ico')"
