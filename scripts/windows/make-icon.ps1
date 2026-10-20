# S4-01: draws the Postman icon (white envelope on a blue rounded square) and writes app-gpui\res\postman.ico.
# Every size is drawn on its own (not scaled down), PNG frames inside the .ico (Windows Vista and later).
# The .ico is committed; rerun only to change the picture.
# Usage: powershell -File scripts\windows\make-icon.ps1 [-Out <path>]
param(
  [string]$Out = "$PSScriptRoot\..\..\app-gpui\res\postman.ico"
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$blue = [System.Drawing.Color]::FromArgb(255, 15, 108, 189)   # Fluent accent #0F6CBD

function Frame([int]$s) {
  $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g.Clear([System.Drawing.Color]::Transparent)

  # Background: rounded square over the whole frame.
  $r = [single]($s * 0.22)
  $bg = New-Object System.Drawing.Drawing2D.GraphicsPath
  $e = [single]($s - 0.5)
  $bg.AddArc(0, 0, 2 * $r, 2 * $r, 180, 90)
  $bg.AddArc($e - 2 * $r, 0, 2 * $r, 2 * $r, 270, 90)
  $bg.AddArc($e - 2 * $r, $e - 2 * $r, 2 * $r, 2 * $r, 0, 90)
  $bg.AddArc(0, $e - 2 * $r, 2 * $r, 2 * $r, 90, 90)
  $bg.CloseFigure()
  $g.FillPath((New-Object System.Drawing.SolidBrush $blue), $bg)

  # Envelope: white body, the flap as a blue "V" from the top corners to the middle.
  $x0 = [single]($s * 0.17); $x1 = [single]($s * 0.83)
  $y0 = [single]($s * 0.29); $y1 = [single]($s * 0.71)
  $g.FillRectangle([System.Drawing.Brushes]::White, $x0, $y0, $x1 - $x0, $y1 - $y0)
  $w = [single][Math]::Max(1.0, $s / 14.0)
  $pen = New-Object System.Drawing.Pen $blue, $w
  $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
  $inset = $w / 2
  $pts = @(
    (New-Object System.Drawing.PointF ($x0 + $inset), ($y0 + $inset)),
    (New-Object System.Drawing.PointF ($s / 2), ([single]($s * 0.53))),
    (New-Object System.Drawing.PointF ($x1 - $inset), ($y0 + $inset))
  )
  $g.DrawLines($pen, [System.Drawing.PointF[]]$pts)
  $g.Dispose()

  $ms = New-Object System.IO.MemoryStream
  $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  , $ms.ToArray()
}

$frames = @($sizes | ForEach-Object { , (Frame $_) })
$ico = New-Object System.IO.MemoryStream
$wr = New-Object System.IO.BinaryWriter $ico
# ICONDIR: reserved, type 1 (icon), count.
$wr.Write([uint16]0); $wr.Write([uint16]1); $wr.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $s = $sizes[$i]; $len = $frames[$i].Length
  $b = if ($s -ge 256) { 0 } else { $s }   # 0 means 256
  $wr.Write([byte]$b); $wr.Write([byte]$b); $wr.Write([byte]0); $wr.Write([byte]0)
  $wr.Write([uint16]1); $wr.Write([uint16]32); $wr.Write([uint32]$len); $wr.Write([uint32]$offset)
  $offset += $len
}
foreach ($f in $frames) { $wr.Write([byte[]]$f) }
$wr.Flush()
$dir = Split-Path -Parent $Out
if ($dir) { New-Item -ItemType Directory -Force $dir | Out-Null }
[System.IO.File]::WriteAllBytes($Out, $ico.ToArray())
"icon: $Out ($($sizes -join ', ') px, $($ico.Length) bytes)"
