# Таблица поясов IANA ↔ Windows id для EWS (crates/ews/src/tz/zones.rs) — CLDR windowsZones из ICU Windows
# (icu.dll, Windows 10 1903+). Запуск — вручную, когда нужна новая версия CLDR; в сборке и в работе Postman
# ничего не запрашивается.
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-zones.ps1 > crates\ews\src\tz\zones.rs
# Вывод — UTF-8 без BOM, строки LF (как в репозитории). Сам скрипт — UTF-8 с BOM: иначе PowerShell 5.1 читает
# кириллицу строк как ANSI.

$src = @"
using System;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public static class Icu {
  [DllImport("icu.dll", CharSet=CharSet.Unicode)] static extern IntPtr ucal_openTimeZones(ref int status);
  [DllImport("icu.dll", CharSet=CharSet.Unicode)] static extern IntPtr uenum_unext(IntPtr e, out int len, ref int status);
  [DllImport("icu.dll")] static extern void uenum_close(IntPtr e);
  [DllImport("icu.dll", CharSet=CharSet.Unicode)] static extern int ucal_getWindowsTimeZoneID([MarshalAs(UnmanagedType.LPWStr)] string id, int len, [Out] char[] win, int cap, ref int status);
  [DllImport("icu.dll", CharSet=CharSet.Unicode)] static extern int ucal_getTimeZoneIDForWindowsID([MarshalAs(UnmanagedType.LPWStr)] string win, int len, [MarshalAs(UnmanagedType.LPStr)] string region, [Out] char[] id, int cap, ref int status);
  [DllImport("icu.dll")] static extern void ulocdata_getCLDRVersion([Out] byte[] v, ref int status);
  public static string Cldr() { int s = 0; var v = new byte[4]; ulocdata_getCLDRVersion(v, ref s); return v[0] + "." + v[1]; }
  public static List<string> Zones() {
    int s = 0; var e = ucal_openTimeZones(ref s); var list = new List<string>();
    while (true) { int len; var p = uenum_unext(e, out len, ref s); if (p == IntPtr.Zero) break; list.Add(Marshal.PtrToStringUni(p, len)); }
    uenum_close(e); return list;
  }
  public static string Win(string id) { int s = 0; var b = new char[128]; int n = ucal_getWindowsTimeZoneID(id, id.Length, b, 128, ref s); return (s > 0 || n <= 0) ? null : new string(b, 0, n); }
  public static string Iana(string win) { int s = 0; var b = new char[128]; int n = ucal_getTimeZoneIDForWindowsID(win, win.Length, "001", b, 128, ref s); return (s > 0 || n <= 0) ? null : new string(b, 0, n); }
}
"@
Add-Type -TypeDefinition $src

# Пояса базы IANA (с прежними именами); без устаревших трёхбуквенных id ICU (ACT, EST…) и SystemV/.
$pairs = @{}
foreach ($z in [Icu]::Zones()) {
    if (($z -like '*/*' -or $z -eq 'UTC') -and $z -notlike 'SystemV/*') {
        $w = [Icu]::Win($z)
        if ($w) { $pairs[$z] = $w }
    }
}
$main = @{}
foreach ($w in ($pairs.Values | Sort-Object -Unique)) {
    $i = [Icu]::Iana($w)
    if ($i) { $main[$w] = $i }
}

# Порядок — байтовый, как у `str::cmp` (двоичный поиск в tz.rs).
$ordinal = [System.StringComparer]::Ordinal
function Sorted($keys) { $a = [string[]]@($keys); [Array]::Sort($a, $ordinal); $a }

$out = New-Object System.Text.StringBuilder
[void]$out.Append("//! Таблица поясов IANA ↔ Windows id — CLDR ``windowsZones`` (CLDR $([Icu]::Cldr()), данные ICU Windows:`n")
[void]$out.Append("//! ``ucal_getWindowsTimeZoneID``, ``ucal_getTimeZoneIDForWindowsID`` с территорией ``001``). Создана скриптом`n")
[void]$out.Append("//! ``scripts/windows-zones.ps1``; в сборке и в работе ничего не запрашивается. Данные CLDR — Unicode License v3.`n`n")
[void]$out.Append("/// (IANA, Windows) по IANA — для двоичного поиска: все пояса базы IANA (с прежними именами), у которых есть Windows id.`n")
[void]$out.Append("pub(super) const IANA_TO_WINDOWS: &[(&str, &str)] = &[`n")
foreach ($k in (Sorted $pairs.Keys)) { [void]$out.Append("    (`"$k`", `"$($pairs[$k])`"),`n") }
[void]$out.Append("];`n`n")
[void]$out.Append("/// (Windows, IANA) по Windows id: основной пояс IANA для каждого Windows id (территория ``001``).`n")
[void]$out.Append("pub(super) const WINDOWS_TO_IANA: &[(&str, &str)] = &[`n")
foreach ($k in (Sorted $main.Keys)) { [void]$out.Append("    (`"$k`", `"$($main[$k])`"),`n") }
[void]$out.Append("];`n")

$stdout = [Console]::OpenStandardOutput()
$bytes = (New-Object System.Text.UTF8Encoding $false).GetBytes($out.ToString())
$stdout.Write($bytes, 0, $bytes.Length)
