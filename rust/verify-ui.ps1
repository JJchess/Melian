param([string]$Mode='capture',[string]$Output='preview.png',[uint32]$Action=0)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class IslandTest {
 [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c,string n);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out RECT r);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
}
'@
[IslandTest]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
$hwnd=[IslandTest]::FindWindow('ClaudeGuard.Rust.Island',$null)
if($hwnd -eq [IntPtr]::Zero){throw 'No Rust island window'}
if($Mode -eq 'open'){[IslandTest]::PostMessage($hwnd,0x8001,[IntPtr]$Action,[IntPtr]::Zero) | Out-Null; Start-Sleep -Milliseconds 400}
if($Mode -eq 'action'){[IslandTest]::PostMessage($hwnd,0x8001,[IntPtr]$Action,[IntPtr]::Zero) | Out-Null; exit}
if($Mode -eq 'collapse'){[IslandTest]::PostMessage($hwnd,0x8003,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null; exit}
$bounds=New-Object IslandTest+RECT
[IslandTest]::GetWindowRect($hwnd,[ref]$bounds) | Out-Null
$width=$bounds.Right-$bounds.Left;$height=$bounds.Bottom-$bounds.Top
$bitmap=[Drawing.Bitmap]::new($width,$height)
$graphics=[Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Left,$bounds.Top,0,0,$bitmap.Size)
$bitmap.Save([IO.Path]::GetFullPath($Output),[Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose();$bitmap.Dispose()
[pscustomobject]@{Handle=$hwnd;Left=$bounds.Left;Top=$bounds.Top;Width=$width;Height=$height;Output=$Output}

