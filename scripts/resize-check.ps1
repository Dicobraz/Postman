# S1-04: black frames and flicker on maximize / restore / resize, without touching mouse or keyboard.
# Starts the app, then in parallel: one thread captures the window as fast as it can and measures the share
# of near-black pixels in the client area; the other maximizes/restores, minimizes/restores and resizes the
# window with SetWindowPos. A frame with more than 25 % near-black pixels counts as a black frame
# (the app background is light).
# Capture: PrintWindow (the window surface) by default; -Screen captures what is on screen instead (the window
# is made topmost for the run) - this is what the user sees. PrintWindow also catches surfaces DWM never
# shows, e.g. while the window is being minimized.
# Usage: powershell -File scripts\resize-check.ps1 [-Exe target\release\postman-gpui.exe] [-ExeArgs "--demo 50000"] [-Screen]
param(
  [string]$Exe = "$PSScriptRoot\..\target\release\postman-gpui.exe",
  [string]$ExeArgs = "--demo 50000",
  [switch]$Screen
)

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Threading;

public static class Rc {
  [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  public struct RECT { public int L, T, R, B; }
  public struct POINT { public int X, Y; }

  public static bool FromScreen;
  static volatile bool run;
  static int frames, black;
  static double maxDark;
  static string phase = "";
  static System.Collections.Generic.List<string> events = new System.Collections.Generic.List<string>();

  public static void DpiAware() { SetProcessDpiAwarenessContext(new IntPtr(-4)); }

  static double Dark(IntPtr h) {
    RECT r; if (!GetClientRect(h, out r)) return 0;
    int w = r.R - r.L, hh = r.B - r.T; if (w < 50 || hh < 50) return 0;
    using (var bmp = new Bitmap(w, hh, PixelFormat.Format32bppArgb)) {
      using (var g = Graphics.FromImage(bmp)) {
        if (FromScreen) { var p = new POINT(); ClientToScreen(h, ref p); g.CopyFromScreen(p.X, p.Y, 0, 0, new Size(w, hh)); }
        else { IntPtr dc = g.GetHdc(); PrintWindow(h, dc, 3); g.ReleaseHdc(dc); }
      }
      var d = bmp.LockBits(new Rectangle(0, 0, w, hh), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
      int dark = 0, n = 0;
      for (int y = 0; y < 24; y++) for (int x = 0; x < 32; x++) {
        int px = (x * 2 + 1) * w / 64, py = (y * 2 + 1) * hh / 48;
        int v = Marshal.ReadInt32(d.Scan0, py * d.Stride + px * 4);
        int b = v & 255, gg = (v >> 8) & 255, rr = (v >> 16) & 255;
        n++; if (rr < 24 && gg < 24 && b < 24) dark++;
      }
      bmp.UnlockBits(d);
      return (double)dark / n;
    }
  }

  static void Capture(object o) {
    IntPtr h = (IntPtr)o;
    while (run) {
      if (IsIconic(h)) { Thread.Sleep(5); continue; }
      double f = Dark(h);
      frames++;
      if (f > maxDark) maxDark = f;
      if (f > 0.25) { black++; lock (events) events.Add(string.Format("{0} dark={1:P0}", phase, f)); }
    }
  }

  public static string Exercise(IntPtr h) {
    run = true; frames = 0; black = 0; maxDark = 0;
    if (FromScreen) SetWindowPos(h, new IntPtr(-1), 0, 0, 0, 0, 0x13); // HWND_TOPMOST, no move/size/activate
    var t = new Thread(Capture); t.Start(h);
    Thread.Sleep(500);
    for (int i = 0; i < 5; i++) {
      phase = "maximize"; ShowWindow(h, 3); Thread.Sleep(500);
      phase = "restore"; ShowWindow(h, 9); Thread.Sleep(500);
    }
    for (int i = 0; i < 2; i++) {
      phase = "minimize"; ShowWindow(h, 6); Thread.Sleep(400);
      phase = "unminimize"; ShowWindow(h, 9); Thread.Sleep(600);
    }
    phase = "resize";
    for (int k = 0; k < 2; k++) {
      for (int w = 1000; w <= 1600; w += 20) { SetWindowPos(h, IntPtr.Zero, 0, 0, w, 900, 0x16); Thread.Sleep(16); }
      for (int w = 1600; w >= 1000; w -= 20) { SetWindowPos(h, IntPtr.Zero, 0, 0, w, 700 + (w - 1000) / 3, 0x16); Thread.Sleep(16); }
    }
    phase = "settle"; Thread.Sleep(500);
    run = false; t.Join();
    var s = string.Format("frames={0} black_frames={1} max_dark={2:P0}", frames, black, maxDark);
    foreach (var e in events) s += "\n  " + e;
    return s;
  }
}
"@

[Rc]::DpiAware()
$p = Start-Process -FilePath $Exe -ArgumentList $ExeArgs -PassThru
Start-Sleep -Seconds 5
$p.Refresh()
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Stop-Process -Id $p.Id -Force; throw "no window" }
[Rc]::FromScreen = [bool]$Screen
$result = [Rc]::Exercise($h)
Stop-Process -Id $p.Id -Force
$mode = if ($Screen) { "screen" } else { "printwindow" }
"$([IO.Path]::GetFileName($Exe)) $ExeArgs ($mode)"
$result
