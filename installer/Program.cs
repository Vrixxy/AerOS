using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Windows.Forms;

namespace AerOSSetup
{
    static class Program
    {
        public const string Version = "0.1.0";

        [STAThread]
        static int Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            try
            {
                if (args.Length >= 2 && args[0] == "--plan")
                    return WritePlan(args[1], args.Skip(2).ToArray());
                if (args.Length >= 3 && args[0] == "--silent")
                    return Silent(args[1], long.Parse(args[2]) * Engine.GiB, args.Contains("--reboot"), args.Contains("--uninstall"));
                bool restart = args.Contains("--restart");
                bool uninstall = args.Contains("--uninstall");
                if (!Engine.IsElevated())
                    return Elevate(args);
                if (restart)
                    return Restart();
                var mode = uninstall ? SetupForm.Mode.Uninstall
                    : Payload.Present ? SetupForm.Mode.Install
                    : SetupForm.Mode.Tools;
                Application.Run(new SetupForm(mode));
                return 0;
            }
            catch (Exception error)
            {
                Engine.Log("fatal: " + error);
                MessageBox.Show(error.Message, "AerOS", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 1;
            }
        }

        /// Setup changes the disk and the boot menu, so it runs as administrator.
        static int Elevate(string[] args)
        {
            try
            {
                Process.Start(new ProcessStartInfo(Payload.ExePath, string.Join(" ", args)) { UseShellExecute = true, Verb = "runas" });
                return 0;
            }
            catch (System.ComponentModel.Win32Exception)
            {
                MessageBox.Show("AerOS needs permission to change the disk and the boot menu. Nothing was changed.",
                    "AerOS", MessageBoxButtons.OK, MessageBoxIcon.Information);
                return 1;
            }
        }

        static int Restart()
        {
            var record = Engine.ReadRecord();
            if (record == null)
            {
                MessageBox.Show("AerOS isn't installed on this PC.", "AerOS", MessageBoxButtons.OK, MessageBoxIcon.Information);
                return 1;
            }
            if (MessageBox.Show("Your PC will restart into AerOS. Save your work first.\n\nThe next time you restart from AerOS, the PC goes back to Windows.",
                    "Restart into AerOS", MessageBoxButtons.OKCancel, MessageBoxIcon.Information) != DialogResult.OK)
                return 0;
            Engine.RestartIntoAerOS(record);
            return 0;
        }

        /// `--silent <result file> <GB> [--reboot] [--uninstall]`: no screens, for
        /// automated tests and scripted installs. Must already run elevated.
        static int Silent(string resultPath, long wanted, bool reboot, bool uninstall)
        {
            using var output = new StreamWriter(resultPath) { AutoFlush = true };
            int lastPercent = int.MinValue;
            void Report(int percent, string text)
            {
                if (percent == lastPercent)
                    return;
                lastPercent = percent;
                output.WriteLine("[" + percent + "%] " + text);
            }
            if (!Engine.IsElevated())
            {
                output.WriteLine("RESULT=not-elevated");
                return 2;
            }
            try
            {
                if (uninstall)
                {
                    var record = Engine.ReadRecord();
                    if (record == null)
                    {
                        output.WriteLine("RESULT=not-installed");
                        return 3;
                    }
                    Engine.Uninstall(record, Report);
                    output.WriteLine("RESULT=uninstalled");
                    return 0;
                }
                var info = Engine.Inspect();
                output.WriteLine("disk=" + info.DiskNumber + " " + info.DiskName + " " + info.BusType + " gpt=" + info.Gpt +
                    " c=" + info.PartitionNumber + " size=" + info.PartitionSize + " shrink_min=" + info.ShrinkMinimum +
                    " free=" + info.FreeBytes + " secure_boot=" + info.SecureBoot + " bitlocker=" + info.BitLocker);
                var blockers = Engine.Blockers(info, wanted);
                foreach (var blocker in blockers)
                    output.WriteLine("BLOCKER: " + blocker);
                if (blockers.Count > 0)
                {
                    output.WriteLine("RESULT=blocked");
                    return 4;
                }
                Engine.Install(info, wanted, Report);
                var installed = Engine.ReadRecord();
                output.WriteLine("boot_entry=" + installed?.BootEntry + " partition=" + installed?.PartitionGuid);
                output.WriteLine("RESULT=installed");
                if (reboot && installed != null)
                    Engine.RestartIntoAerOS(installed);
                return 0;
            }
            catch (Exception error)
            {
                output.WriteLine("ERROR: " + error);
                output.WriteLine("RESULT=failed");
                return 1;
            }
        }

        /// `--plan <file> [GB]`: the checks and every command an install would
        /// run, without changing anything (no administrator rights needed).
        static int WritePlan(string path, string[] rest)
        {
            Engine.DryRun = true;
            var info = Engine.Inspect();
            long wanted = (rest.Length > 0 ? long.Parse(rest[0]) : 8) * Engine.GiB;
            using var output = new StreamWriter(path);
            output.WriteLine("elevated=" + info.Elevated);
            output.WriteLine("uefi=" + info.Uefi);
            output.WriteLine("secure_boot=" + info.SecureBoot);
            output.WriteLine("bitlocker=" + info.BitLocker);
            output.WriteLine("gpt=" + info.Gpt + " disk=" + info.DiskNumber + " (" + info.DiskName + ", " + info.BusType + ")");
            output.WriteLine("c_partition=" + info.PartitionNumber + " offset=" + info.PartitionOffset + " size=" + info.PartitionSize + " shrink_min=" + info.ShrinkMinimum);
            output.WriteLine("free=" + Engine.Gb(info.FreeBytes) + " max_aeros=" + Engine.Gb(info.MaxAerOSBytes));
            output.WriteLine("payload=" + (Payload.Present ? Engine.Gb(Payload.UnpackedBytes()) : "none"));
            output.WriteLine("installed=" + (info.Installed?.BootEntry ?? "no"));
            output.WriteLine("problem=" + info.Problem);
            foreach (var blocker in Engine.Blockers(info, wanted))
                output.WriteLine("BLOCKER: " + blocker);
            try
            {
                Engine.Install(info, wanted, (percent, text) => output.WriteLine("[" + percent + "%] " + text));
            }
            catch (Exception error)
            {
                output.WriteLine("plan error: " + error.Message);
            }
            foreach (var step in Engine.Plan)
                output.WriteLine("  would run: " + step);
            return 0;
        }
    }
}
