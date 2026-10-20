# S1-01: hotkeys in the Russian layout. Starts postman.exe with POSTMAN_KEYLOG=1, switches its window
# to the given layout, presses keys and prints what the app received ("key <keystroke> (<char>) -> <action>").
# Keys are sent only while the Postman window is in the foreground; otherwise the script stops.
# The keystroke log is written to %TEMP%\postman-keys-<pid>.txt and removed afterwards (-Keep leaves it).
# Usage: powershell -File scripts\keys-ru.ps1 [-Exe <path to postman.exe>] [-Layout 00000419] [-Keep]
param(
  [string]$Exe = "",
  [string]$Layout = "00000419",
  [switch]$Keep
)
# Build directory from cargo (honours CARGO_TARGET_DIR) - never a hard-coded target\ (H-01).
function Release-Exe([string]$name) {
  $meta = cargo metadata --format-version 1 --no-deps --manifest-path "$PSScriptRoot\..\Cargo.toml" | ConvertFrom-Json
  Join-Path $meta.target_directory "release\$name"
}
if (-not $Exe) { $Exe = Release-Exe "postman.exe" }

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class K {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr LoadKeyboardLayoutW(string id, uint flags);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@

$out = Join-Path $env:TEMP "postman-keys-$PID.txt"
trap { if (-not $Keep) { Remove-Item $out -ErrorAction SilentlyContinue }; break }
$env:POSTMAN_KEYLOG = "1"
$p = Start-Process -FilePath $Exe -ArgumentList "--demo 50000" -PassThru -RedirectStandardOutput $out
Start-Sleep -Seconds 5
$p.Refresh()
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Stop-Process -Id $p.Id -Force; throw "no window" }

[K]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [K]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
[void][K]::SetForegroundWindow($h)
Start-Sleep -Milliseconds 300
# Switch the window's layout the way a user does: the language hotkey from HKCU\Keyboard Layout\Toggle
# (1 = Alt+Shift, 2 = Ctrl+Shift), up to 4 times, until the window thread has the wanted layout.
$hkl = [K]::LoadKeyboardLayoutW($Layout, 0)
$tid = [K]::GetWindowThreadProcessId($h, [IntPtr]::Zero)
$toggle = (Get-ItemProperty "HKCU:\Keyboard Layout\Toggle" -ErrorAction SilentlyContinue)."Language Hotkey"
$first = if ($toggle -eq "1") { 0x12 } else { 0x11 }
for ($i = 0; $i -lt 4 -and [K]::GetKeyboardLayout($tid) -ne $hkl; $i++) {
  if ([K]::GetForegroundWindow() -ne $h) { Stop-Process -Id $p.Id -Force; throw "Postman lost foreground while switching layout" }
  [K]::keybd_event($first, 0, 0, [UIntPtr]::Zero)
  [K]::keybd_event(0x10, 0, 0, [UIntPtr]::Zero)
  [K]::keybd_event(0x10, 0, 2, [UIntPtr]::Zero)
  [K]::keybd_event($first, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 400
}
$now = [K]::GetKeyboardLayout($tid)
"layout of window thread: 0x{0:X8}" -f $now.ToInt64()
if ($now -ne $hkl) { Stop-Process -Id $p.Id -Force; throw "could not switch the window to $Layout" }
# The language hotkey itself reaches the app as modifier events; drop them from the log below.

function Press([byte[]]$mods, [byte]$vk, [string]$label) {
  if ([K]::GetForegroundWindow() -ne $h) { Stop-Process -Id $p.Id -Force; throw "Postman lost foreground before $label" }
  foreach ($m in $mods) { [K]::keybd_event($m, 0, 0, [UIntPtr]::Zero) }
  [K]::keybd_event($vk, 0, 0, [UIntPtr]::Zero)
  [K]::keybd_event($vk, 0, 2, [UIntPtr]::Zero)
  [array]::Reverse($mods)
  foreach ($m in $mods) { [K]::keybd_event($m, 0, 2, [UIntPtr]::Zero) }
  Start-Sleep -Milliseconds 250
}

$CTRL = 0x11; $SHIFT = 0x10
Press @() 0x4A "j"
Press @() 0x4A "j"
Press @() 0x4B "k"
Press @($CTRL) 0x51 "ctrl-q"
Press @($CTRL) 0x55 "ctrl-u"
Press @($CTRL) 0x32 "ctrl-2"
Press @($CTRL, $SHIFT) 0x51 "ctrl-shift-q"
Press @($CTRL) 0x31 "ctrl-1"
Press @($CTRL) 0x45 "ctrl-e"
# In the search field letters are text, not list navigation: expect no action for these two.
Press @() 0x4A "j in search"
Press @() 0x4B "k in search"
Press @() 0x1B "escape"
Press @() 0x2D "insert"

Start-Sleep -Milliseconds 500
Stop-Process -Id $p.Id -Force
Start-Sleep -Milliseconds 300
Get-Content -Encoding UTF8 $out | Where-Object { $_ -like "key *" -or $_ -like "search *" }
if (-not $Keep) { Remove-Item $out -ErrorAction SilentlyContinue }
