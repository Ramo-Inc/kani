# E2E test: generate relative mouse movements via SendInput to trigger border crossing
# Usage: powershell -ExecutionPolicy Bypass -File tests\e2e_mouse_inject.ps1

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public class MouseSendInput {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT lpPoint);

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X;
        public int Y;
    }

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUT {
        [FieldOffset(0)] public uint type;
        [FieldOffset(8)] public MOUSEINPUT mi;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    public static void MoveRelative(int dx, int dy) {
        INPUT[] inputs = new INPUT[1];
        inputs[0].type = 0; // INPUT_MOUSE
        inputs[0].mi.dx = dx;
        inputs[0].mi.dy = dy;
        inputs[0].mi.dwFlags = 0x0001; // MOUSEEVENTF_MOVE (relative)
        SendInput(1, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    public static POINT GetPos() {
        POINT p;
        GetCursorPos(out p);
        return p;
    }
}
"@

$pos = [MouseSendInput]::GetPos()
Write-Host "=== E2E Mouse Border Test ==="
Write-Host "Start cursor position: X=$($pos.X) Y=$($pos.Y)"
Write-Host "Moving cursor LEFT toward edge (200 steps x -30px = -6000px)..."
Write-Host ""

for ($i = 0; $i -lt 200; $i++) {
    [MouseSendInput]::MoveRelative(-30, 0)
    Start-Sleep -Milliseconds 10
}

$pos = [MouseSendInput]::GetPos()
Write-Host "End cursor position: X=$($pos.X) Y=$($pos.Y)"
Write-Host ""
Write-Host "If kani detected edge crossing, you should see 'Enter sent' in the kani log."
