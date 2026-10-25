# WTS_CURRENT_SESSION; no administrative query of other users or token secrets.
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class SyncEngineSession {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    public struct Info {
        public int State;
        public uint SessionId, IncomingBytes, OutgoingBytes, IncomingFrames, OutgoingFrames,
            IncomingCompressedBytes, OutgoingCompressedBytes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Station;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=17)] public string Domain;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=21)] public string User;
        public long ConnectTime, DisconnectTime, LastInputTime, LogonTime, CurrentTime;
    }
    [DllImport("wtsapi32.dll", EntryPoint="WTSQuerySessionInformationW", SetLastError=true)]
    static extern bool Query(IntPtr server, int session, int kind, out IntPtr data, out uint size);
    [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr data);
    public static string Read() {
        IntPtr data; uint size;
        if (!Query(IntPtr.Zero, -1, 24, out data, out size))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        try {
            if (size < Marshal.SizeOf(typeof(Info))) throw new InvalidOperationException("WTS size");
            Info info=(Info)Marshal.PtrToStructure(data, typeof(Info));
            if (info.SessionId==0 || info.LogonTime<=0) throw new InvalidOperationException("No interactive logon");
            return info.SessionId.ToString(System.Globalization.CultureInfo.InvariantCulture)+":"+
                info.LogonTime.ToString(System.Globalization.CultureInfo.InvariantCulture);
        } finally { WTSFreeMemory(data); }
    }
}
"@
[Console]::Write([SyncEngineSession]::Read())
