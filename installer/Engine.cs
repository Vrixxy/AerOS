using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
using System.Text.RegularExpressions;
using Microsoft.Win32;

namespace AerOSSetup
{
    /// What the installer found out about this PC.
    sealed class SystemInfo
    {
        public bool Elevated;
        public bool Uefi;
        public bool? SecureBoot;
        public bool? BitLocker;
        public bool Gpt;
        public int DiskNumber = -1;
        public int PartitionNumber = -1;
        public long PartitionOffset;
        public long PartitionSize;
        public long ShrinkMinimum = -1;
        public long FreeBytes;
        public string DiskName = "";
        public string BusType = "";
        public InstallRecord Installed;
        public string Problem;

        /// Largest AerOS partition this PC can give, in bytes (Windows keeps
        /// at least `Engine.WindowsReserve` free and FAT32 caps it at 32 GB).
        public long MaxAerOSBytes
        {
            get
            {
                long byFree = FreeBytes - Engine.WindowsReserve;
                long byShrink = ShrinkMinimum > 0 ? PartitionSize - ShrinkMinimum - Engine.WindowsReserve : byFree;
                return Math.Max(0, Math.Min(Engine.MaxBytes, Math.Min(byFree, byShrink)));
            }
        }
    }

    sealed class InstallRecord
    {
        public string BootEntry;
        public string PartitionGuid;
        public int DiskNumber;
        public long Size;
        public string Version;
    }

    static class Engine
    {
        public const long GiB = 1L << 30;
        public const long MiB = 1L << 20;
        public const long MaxBytes = 32 * GiB;
        public const long WindowsReserve = 10 * GiB;
        const string BasicDataGuid = "{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}";
        const string RegistryKey = @"SOFTWARE\AerOS";
        const string UninstallKey = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\AerOS";
        public static readonly string InstallDir =
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "AerOS");
        public static readonly string LogPath = Path.Combine(Path.GetTempPath(), "AerOS-Setup.log");

        public static bool DryRun;
        public static readonly List<string> Plan = new List<string>();

        [DllImport("kernel32.dll")]
        static extern bool GetFirmwareType(out int type);

        public static void Log(string line)
        {
            try { File.AppendAllText(LogPath, DateTime.Now.ToString("HH:mm:ss ") + line + Environment.NewLine); }
            catch { }
        }

        public static bool IsElevated()
        {
            using var identity = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
        }

        // ---- Running tools ----------------------------------------------

        public static string Run(string file, string arguments, bool changes = true)
        {
            if (changes && DryRun)
            {
                Plan.Add(file + " " + arguments);
                return "";
            }
            Log("> " + file + " " + arguments);
            var start = new ProcessStartInfo(file, arguments)
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                StandardOutputEncoding = Encoding.UTF8,
            };
            using var process = Process.Start(start);
            string output = process.StandardOutput.ReadToEnd();
            string error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            Log(output.Trim());
            if (process.ExitCode != 0)
            {
                Log("exit " + process.ExitCode + ": " + error.Trim());
                throw new SetupException(Path.GetFileName(file) + " failed: " + FirstLine(error.Length > 0 ? error : output));
            }
            return output;
        }

        /// Runs a PowerShell script (the Storage cmdlets do the disk work).
        public static string PowerShell(string script, bool changes = true)
        {
            if (changes && DryRun)
            {
                Plan.Add("PowerShell: " + script);
                return "";
            }
            string encoded = Convert.ToBase64String(Encoding.Unicode.GetBytes(
                "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; " + script));
            return Run("powershell.exe", "-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand " + encoded, false);
        }

        static string BcdEdit(string arguments, bool changes = true) =>
            Run(Path.Combine(Environment.SystemDirectory, "bcdedit.exe"), arguments, changes);

        static string FirstLine(string text) =>
            text.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault()?.Trim() ?? "unknown error";

        static Dictionary<string, string> Fields(string output)
        {
            var fields = new Dictionary<string, string>();
            foreach (var line in output.Split('\n'))
            {
                int equals = line.IndexOf('=');
                if (equals > 0)
                    fields[line.Substring(0, equals).Trim()] = line.Substring(equals + 1).Trim();
            }
            return fields;
        }

        // ---- Checks -----------------------------------------------------

        public static SystemInfo Inspect()
        {
            var info = new SystemInfo { Elevated = IsElevated() };
            info.Uefi = GetFirmwareType(out int type) && type == 2;
            info.SecureBoot = ReadSecureBoot();
            info.Installed = ReadRecord();
            try
            {
                info.FreeBytes = new DriveInfo("C").AvailableFreeSpace;
                var fields = Fields(PowerShell(
                    "$p = Get-Partition -DriveLetter C; $d = Get-Disk -Number $p.DiskNumber; " +
                    "'disk=' + $p.DiskNumber; 'number=' + $p.PartitionNumber; 'offset=' + $p.Offset; 'size=' + $p.Size; " +
                    "'style=' + $d.PartitionStyle; 'name=' + $d.FriendlyName; 'bus=' + $d.BusType; " +
                    "try { 'min=' + (Get-PartitionSupportedSize -DiskNumber $p.DiskNumber -PartitionNumber $p.PartitionNumber).SizeMin } catch { 'min=-1' }",
                    changes: false));
                info.DiskNumber = int.Parse(fields["disk"]);
                info.PartitionNumber = int.Parse(fields["number"]);
                info.PartitionOffset = long.Parse(fields["offset"]);
                info.PartitionSize = long.Parse(fields["size"]);
                info.ShrinkMinimum = long.Parse(fields["min"]);
                info.Gpt = fields["style"] == "GPT";
                info.DiskName = fields["name"];
                info.BusType = fields["bus"];
            }
            catch (Exception error)
            {
                info.Problem = "Couldn't read the disk layout: " + error.Message;
            }
            info.BitLocker = ReadBitLocker();
            return info;
        }

        static bool? ReadSecureBoot()
        {
            using var key = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Control\SecureBoot\State");
            return key?.GetValue("UEFISecureBootEnabled") is int value ? value != 0 : (bool?)null;
        }

        static bool? ReadBitLocker()
        {
            try
            {
                var fields = Fields(PowerShell(
                    "$v = Get-CimInstance -Namespace root/CIMV2/Security/MicrosoftVolumeEncryption -ClassName Win32_EncryptableVolume -Filter \"DriveLetter='C:'\" -ErrorAction SilentlyContinue; " +
                    "if ($v) { 'status=' + $v.ProtectionStatus } else { $s = (New-Object -ComObject Shell.Application).NameSpace('C:').Self.ExtendedProperty('System.Volume.BitLockerProtection'); 'shell=' + $s }",
                    changes: false));
                if (fields.TryGetValue("status", out var status))
                    return status == "1";
                if (fields.TryGetValue("shell", out var shell))
                    return shell == "1" || shell == "3" || shell == "5";
            }
            catch { }
            return null;
        }

        public static InstallRecord ReadRecord()
        {
            using var key = Registry.LocalMachine.OpenSubKey(RegistryKey);
            if (key?.GetValue("BootEntry") is not string entry)
                return null;
            return new InstallRecord
            {
                BootEntry = entry,
                PartitionGuid = key.GetValue("PartitionGuid") as string,
                DiskNumber = key.GetValue("DiskNumber") is int disk ? disk : -1,
                Size = key.GetValue("Size") is long size ? size : 0,
                Version = key.GetValue("Version") as string,
            };
        }

        /// Reasons setup can't go ahead (empty = fine).
        public static List<string> Blockers(SystemInfo info, long wanted)
        {
            var blockers = new List<string>();
            if (!info.Uefi)
                blockers.Add("This PC starts in legacy BIOS mode. AerOS needs UEFI.");
            if (!Environment.Is64BitOperatingSystem)
                blockers.Add("AerOS needs a 64-bit PC.");
            if (info.Problem != null)
                blockers.Add(info.Problem);
            else if (!info.Gpt)
                blockers.Add("Windows's disk uses the old MBR layout. AerOS needs GPT.");
            if (info.Installed != null)
                blockers.Add("AerOS is already installed. Uninstall it first.");
            if (info.Problem == null && wanted > info.MaxAerOSBytes)
                blockers.Add("Not enough free space on C: (AerOS needs " + Gb(wanted) + ", and Windows keeps " + Gb(WindowsReserve) + ").");
            return blockers;
        }

        public static string Gb(long bytes) => (bytes / (double)GiB).ToString("0.#") + " GB";

        // ---- Install ----------------------------------------------------

        public static void Install(SystemInfo info, long wanted, Action<int, string> progress)
        {
            Log("install " + wanted + " bytes, disk " + info.DiskNumber + " partition " + info.PartitionNumber);
            long newWindowsSize = (info.PartitionSize - wanted) / MiB * MiB;
            long aerosSize = info.PartitionSize - newWindowsSize;
            long aerosOffset = info.PartitionOffset + newWindowsSize;
            string bootEntry = null, partitionGuid = null, letter = null;
            bool shrunk = false;
            try
            {
                progress(2, "Making room on drive C:");
                PowerShell("Resize-Partition -DiskNumber " + info.DiskNumber + " -PartitionNumber " + info.PartitionNumber + " -Size " + newWindowsSize);
                shrunk = true;

                progress(12, "Creating the AerOS partition");
                var fields = Fields(PowerShell(
                    "$p = New-Partition -DiskNumber " + info.DiskNumber + " -Offset " + aerosOffset + " -Size " + aerosSize + " -GptType '" + BasicDataGuid + "'; " +
                    "$p | Add-PartitionAccessPath -AssignDriveLetter; $p = Get-Partition -DiskNumber " + info.DiskNumber + " -PartitionNumber $p.PartitionNumber; " +
                    "Format-Volume -Partition $p -FileSystem FAT32 -NewFileSystemLabel AEROS -Confirm:$false -Force | Out-Null; " +
                    "'guid=' + $p.Guid; 'letter=' + $p.DriveLetter"));
                partitionGuid = DryRun ? "{partition}" : fields["guid"];
                letter = DryRun ? "A" : fields["letter"];
                if (string.IsNullOrEmpty(letter))
                    throw new SetupException("Windows didn't give the new partition a drive letter.");

                progress(18, "Copying AerOS");
                if (DryRun)
                    Plan.Add("extract the AerOS files to " + letter + ":\\");
                else
                    Payload.Extract(letter + ":\\", (done, total, name) =>
                        progress(18 + (int)(56 * done / Math.Max(1, total)), "Copying AerOS (" + Gb(done) + " of " + Gb(total) + ")"));

                progress(75, "Making room for your files");
                if (DryRun)
                    Plan.Add("create " + letter + ":\\HOME, AerOS's home folder");
                else
                    CreateHome(letter + ":\\");

                progress(80, "Adding AerOS to the boot menu");
                string copied = BcdEdit("/copy {bootmgr} /d \"AerOS\"");
                bootEntry = DryRun ? "{new-entry}" : Regex.Match(copied, @"\{[0-9a-fA-F-]{36}\}").Value;
                if (string.IsNullOrEmpty(bootEntry))
                    throw new SetupException("bcdedit didn't report the new boot entry.");
                BcdEdit("/set " + bootEntry + " device partition=" + letter + ":");
                BcdEdit("/set " + bootEntry + " path \\EFI\\BOOT\\BOOTX64.EFI");
                BcdEdit("/set {fwbootmgr} displayorder " + bootEntry + " /addlast");

                progress(88, "Hiding the AerOS partition from File Explorer");
                PowerShell("$p = Get-Partition | Where-Object Guid -eq '" + partitionGuid + "'; " +
                    "$p | Remove-PartitionAccessPath -AccessPath '" + letter + ":\\'; $p | Set-Partition -NoDefaultDriveLetter $true");

                progress(92, "Adding AerOS to the Start menu");
                InstallTools(new InstallRecord
                {
                    BootEntry = bootEntry,
                    PartitionGuid = partitionGuid,
                    DiskNumber = info.DiskNumber,
                    Size = aerosSize,
                    Version = Program.Version,
                });
                progress(100, "AerOS is installed");
            }
            catch (Exception error)
            {
                Log("install failed: " + error);
                progress(-1, "Undoing the changes");
                try
                {
                    if (bootEntry != null && !DryRun)
                        BcdEdit("/delete " + bootEntry + " /cleanup");
                    if (partitionGuid != null)
                        PowerShell("Get-Partition | Where-Object Guid -eq '" + partitionGuid + "' | Remove-Partition -Confirm:$false");
                    else if (shrunk)
                        PowerShell("Get-Partition -DiskNumber " + info.DiskNumber + " -Offset " + aerosOffset + " -ErrorAction SilentlyContinue | Remove-Partition -Confirm:$false");
                    if (shrunk)
                        GrowWindows(info.DiskNumber, info.PartitionNumber);
                }
                catch (Exception undo)
                {
                    Log("undo failed: " + undo);
                }
                throw;
            }
        }

        /// The user's files live in HOME, one file AerOS formats and mounts as
        /// /home on its first start (a FAT32 file stops just short of 4 GiB).
        static void CreateHome(string root)
        {
            long free = new DriveInfo(root).AvailableFreeSpace;
            long size = Math.Min(free - 64 * MiB, 4 * GiB - MiB) / MiB * MiB;
            if (size < 64 * MiB)
            {
                Log("no room for HOME (" + free + " bytes free)");
                return;
            }
            using (var file = new FileStream(Path.Combine(root, "HOME"), FileMode.CreateNew, FileAccess.Write))
            {
                file.SetLength(size);
                var marker = Encoding.ASCII.GetBytes("AEROS-DATA-BLANK");
                file.Write(marker, 0, marker.Length);
            }
            Log("HOME " + size + " bytes");
        }

        static void GrowWindows(int disk, int partition) =>
            PowerShell("$s = Get-PartitionSupportedSize -DiskNumber " + disk + " -PartitionNumber " + partition + "; " +
                "Resize-Partition -DiskNumber " + disk + " -PartitionNumber " + partition + " -Size $s.SizeMax");

        /// AerOS.exe (this program without the AerOS files), shortcuts, the
        /// uninstall entry and the record of what was installed.
        static void InstallTools(InstallRecord record)
        {
            string exe = Path.Combine(InstallDir, "AerOS.exe");
            if (DryRun)
            {
                Plan.Add("copy the program (without the AerOS files) to " + exe);
                Plan.Add("Start menu + desktop shortcuts, uninstall entry, HKLM\\" + RegistryKey);
                return;
            }
            Directory.CreateDirectory(InstallDir);
            using (var input = File.OpenRead(Payload.ExePath))
            using (var output = File.Create(exe))
            {
                long left = Payload.ProgramLength();
                var buffer = new byte[1 << 16];
                while (left > 0)
                {
                    int read = input.Read(buffer, 0, (int)Math.Min(buffer.Length, left));
                    if (read <= 0)
                        break;
                    output.Write(buffer, 0, read);
                    left -= read;
                }
            }
            using (var key = Registry.LocalMachine.CreateSubKey(RegistryKey))
            {
                key.SetValue("BootEntry", record.BootEntry);
                key.SetValue("PartitionGuid", record.PartitionGuid);
                key.SetValue("DiskNumber", record.DiskNumber, RegistryValueKind.DWord);
                key.SetValue("Size", record.Size, RegistryValueKind.QWord);
                key.SetValue("Version", record.Version);
            }
            using (var key = Registry.LocalMachine.CreateSubKey(UninstallKey))
            {
                key.SetValue("DisplayName", "AerOS");
                key.SetValue("DisplayVersion", record.Version);
                key.SetValue("Publisher", "AerOS");
                key.SetValue("DisplayIcon", exe);
                key.SetValue("InstallLocation", InstallDir);
                key.SetValue("UninstallString", "\"" + exe + "\" --uninstall");
                key.SetValue("EstimatedSize", (int)(record.Size / 1024), RegistryValueKind.DWord);
                key.SetValue("NoModify", 1, RegistryValueKind.DWord);
                key.SetValue("NoRepair", 1, RegistryValueKind.DWord);
            }
            string programs = Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms);
            string desktop = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
            Shortcut(Path.Combine(desktop, "AerOS.lnk"), exe, "--restart", "Restart into AerOS");
            Shortcut(Path.Combine(programs, "AerOS.lnk"), exe, "--restart", "Restart into AerOS");
            Shortcut(Path.Combine(programs, "Uninstall AerOS.lnk"), exe, "--uninstall", "Remove AerOS from this PC");
        }

        static void Shortcut(string path, string target, string arguments, string description)
        {
            var shellType = Type.GetTypeFromProgID("WScript.Shell");
            dynamic shell = Activator.CreateInstance(shellType);
            dynamic link = shell.CreateShortcut(path);
            link.TargetPath = target;
            link.Arguments = arguments;
            link.Description = description;
            link.IconLocation = target + ",0";
            link.WorkingDirectory = Path.GetDirectoryName(target);
            link.Save();
        }

        // ---- Restart and uninstall ------------------------------------------

        /// Boots AerOS once at the next restart (Windows stays the default).
        public static void RestartIntoAerOS(InstallRecord record)
        {
            BcdEdit("/set {fwbootmgr} bootsequence " + record.BootEntry);
            Run(Path.Combine(Environment.SystemDirectory, "shutdown.exe"), "/r /t 0");
        }

        public static void Uninstall(InstallRecord record, Action<int, string> progress)
        {
            Log("uninstall " + record.BootEntry + " " + record.PartitionGuid);
            progress(5, "Removing AerOS from the boot menu");
            try { BcdEdit("/delete " + record.BootEntry + " /cleanup"); }
            catch (SetupException error) { Log("boot entry already gone? " + error.Message); }

            progress(30, "Deleting the AerOS partition");
            var fields = Fields(PowerShell(
                "$p = Get-Partition | Where-Object Guid -eq '" + record.PartitionGuid + "'; " +
                "if ($p) { $p | Remove-Partition -Confirm:$false; 'removed=1' } else { 'removed=0' }; " +
                "$c = Get-Partition -DriveLetter C; 'disk=' + $c.DiskNumber; 'number=' + $c.PartitionNumber"));

            progress(60, "Giving the space back to drive C:");
            if (fields.TryGetValue("disk", out var disk) && fields.TryGetValue("number", out var number))
            {
                try { GrowWindows(int.Parse(disk), int.Parse(number)); }
                catch (SetupException error) { Log("couldn't extend C: " + error.Message); }
            }

            progress(85, "Removing shortcuts");
            if (!DryRun)
            {
                string programs = Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms);
                string desktop = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
                foreach (var link in new[] { Path.Combine(desktop, "AerOS.lnk"), Path.Combine(programs, "AerOS.lnk"), Path.Combine(programs, "Uninstall AerOS.lnk") })
                    try { File.Delete(link); } catch { }
                Registry.LocalMachine.DeleteSubKeyTree(UninstallKey, false);
                Registry.LocalMachine.DeleteSubKeyTree(RegistryKey, false);
                // The running AerOS.exe can't delete itself: a short-lived cmd does.
                Process.Start(new ProcessStartInfo("cmd.exe",
                    "/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"" + InstallDir + "\"")
                { CreateNoWindow = true, UseShellExecute = false });
            }
            progress(100, "AerOS has been removed");
        }
    }

    sealed class SetupException : Exception
    {
        public SetupException(string message) : base(message) { }
    }
}
