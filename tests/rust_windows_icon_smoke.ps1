param(
  [Parameter(Mandatory = $true)]
  [string] $Binary,
  [string] $Icon = 'assets/ssh-mountmate-logo.ico'
)

$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path $Binary).Path
$iconPath = (Resolve-Path $Icon).Path

Add-Type @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;

public static class SSHMountMateIconTest {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool FreeLibrary(IntPtr module);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LoadResource(IntPtr module, IntPtr resource);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint SizeofResource(IntPtr module, IntPtr resource);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LockResource(IntPtr resource);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadImage(IntPtr module, IntPtr name, uint type,
        int width, int height, uint flags);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool DestroyIcon(IntPtr icon);

    private static byte[] ReadResource(IntPtr module, int type, int id) {
        IntPtr resource = FindResource(module, new IntPtr(id), new IntPtr(type));
        if (resource == IntPtr.Zero) {
            throw new InvalidDataException("Missing PE icon resource: type=" + type + " id=" + id);
        }
        uint size = SizeofResource(module, resource);
        IntPtr loaded = LoadResource(module, resource);
        IntPtr data = loaded == IntPtr.Zero ? IntPtr.Zero : LockResource(loaded);
        if (size == 0 || data == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not load PE icon resource");
        }
        byte[] bytes = new byte[checked((int)size)];
        Marshal.Copy(data, bytes, 0, bytes.Length);
        return bytes;
    }

    public static void Verify(string binary, string icon) {
        byte[] expected = File.ReadAllBytes(icon);
        if (expected.Length < 6 || BitConverter.ToUInt16(expected, 0) != 0
            || BitConverter.ToUInt16(expected, 2) != 1) {
            throw new InvalidDataException("Expected a valid ICO file");
        }
        int count = BitConverter.ToUInt16(expected, 4);
        if (count == 0 || expected.Length < 6 + count * 16) {
            throw new InvalidDataException("ICO image directory is empty or truncated");
        }
        // Load resources without executing the application or resolving its imports.
        IntPtr module = LoadLibraryEx(binary, IntPtr.Zero, 0x00000002 | 0x00000020);
        if (module == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not load executable resources");
        }
        try {
            byte[] group = ReadResource(module, 14, 1);
            if (group.Length != 6 + count * 14) {
                throw new InvalidDataException("PE icon group has the wrong image count");
            }
            for (int index = 0; index < 6; index++) {
                if (group[index] != expected[index]) {
                    throw new InvalidDataException("PE icon group header differs from the tracked ICO");
                }
            }
            for (int index = 0; index < count; index++) {
                int sourceEntry = 6 + index * 16;
                int groupEntry = 6 + index * 14;
                for (int offset = 0; offset < 12; offset++) {
                    if (group[groupEntry + offset] != expected[sourceEntry + offset]) {
                        throw new InvalidDataException("PE icon image metadata differs from the tracked ICO");
                    }
                }
                int id = BitConverter.ToUInt16(group, groupEntry + 12);
                byte[] image = ReadResource(module, 3, id);
                int size = checked((int)BitConverter.ToUInt32(expected, sourceEntry + 8));
                int start = checked((int)BitConverter.ToUInt32(expected, sourceEntry + 12));
                if (image.Length != size || start < 0 || start > expected.Length - size) {
                    throw new InvalidDataException("PE icon image has the wrong size");
                }
                for (int offset = 0; offset < size; offset++) {
                    if (image[offset] != expected[start + offset]) {
                        throw new InvalidDataException("PE icon image differs from the tracked ICO");
                    }
                }
            }
            foreach (int size in new int[] { 16, 32, 256 }) {
                IntPtr loadedIcon = LoadImage(module, new IntPtr(1), 1, size, size, 0);
                if (loadedIcon == IntPtr.Zero) {
                    throw new Win32Exception(Marshal.GetLastWin32Error(),
                        "Windows could not decode icon group 1 at size " + size);
                }
                DestroyIcon(loadedIcon);
            }
            Console.WriteLine("Windows icon resources verified: {0} images in group 1", count);
        } finally {
            FreeLibrary(module);
        }
    }
}
'@

[SSHMountMateIconTest]::Verify($binaryPath, $iconPath)
