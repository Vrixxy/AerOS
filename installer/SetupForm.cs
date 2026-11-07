using System;
using System.Drawing;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace AerOSSetup
{
    /// Placeholder screens (the real design comes from Figma later): one
    /// window whose middle changes per step, Back / Next along the bottom.
    sealed class SetupForm : Form
    {
        public enum Mode { Install, Uninstall, Tools }

        readonly Mode mode;
        readonly Label title = new Label();
        readonly Label body = new Label();
        readonly Panel content = new Panel();
        readonly Button back = new Button();
        readonly Button next = new Button();
        readonly Button cancel = new Button();
        SystemInfo info;
        long wanted = 8 * Engine.GiB;
        Action onNext;
        bool busy;

        static readonly Font TitleFont = new Font("Segoe UI Semibold", 18f);
        static readonly Font BodyFont = new Font("Segoe UI", 10f);

        public SetupForm(Mode mode)
        {
            this.mode = mode;
            Text = mode == Mode.Uninstall ? "Remove AerOS" : "AerOS";
            ClientSize = new Size(720, 480);
            FormBorderStyle = FormBorderStyle.FixedSingle;
            MaximizeBox = false;
            StartPosition = FormStartPosition.CenterScreen;
            BackColor = Color.White;
            Font = BodyFont;
            AutoScaleMode = AutoScaleMode.Dpi;
            try { Icon = Icon.ExtractAssociatedIcon(Payload.ExePath); } catch { }

            title.SetBounds(40, 32, 640, 44);
            title.Font = TitleFont;
            body.SetBounds(40, 80, 640, 64);
            content.SetBounds(40, 150, 640, 250);
            foreach (var button in new[] { back, next, cancel })
            {
                button.Size = new Size(110, 34);
                button.FlatStyle = FlatStyle.System;
            }
            cancel.Location = new Point(40, 426);
            back.Location = new Point(450, 426);
            next.Location = new Point(570, 426);
            cancel.Text = "Cancel";
            back.Text = "Back";
            cancel.Click += (s, e) => Close();
            next.Click += (s, e) => onNext?.Invoke();
            Controls.AddRange(new Control[] { title, body, content, back, next, cancel });
            FormClosing += (s, e) => e.Cancel = busy;

            switch (mode)
            {
                case Mode.Install: Welcome(); break;
                case Mode.Uninstall: ConfirmUninstall(); break;
                default: ToolsPage(); break;
            }
        }

        void Page(string heading, string text, string nextText, Action nextAction, Action backAction = null)
        {
            title.Text = heading;
            body.Text = text;
            content.Controls.Clear();
            next.Text = nextText;
            next.Visible = nextAction != null;
            onNext = nextAction;
            back.Visible = backAction != null;
            back.Click -= BackClicked;
            backTarget = backAction;
            back.Click += BackClicked;
            next.Enabled = true;
        }

        Action backTarget;
        void BackClicked(object sender, EventArgs e) => backTarget?.Invoke();

        Label Text_(string text, int top, int height = 24, Color? color = null)
        {
            var label = new Label { Text = text, Location = new Point(0, top), Size = new Size(640, height), ForeColor = color ?? Color.Black };
            content.Controls.Add(label);
            return label;
        }

        // ---- Install --------------------------------------------------------

        void Welcome() => Page("Install AerOS next to Windows",
            "AerOS gets its own space on this PC. Windows and your files stay as they are, and Windows still starts by default. " +
            "To use AerOS, open \"AerOS\" from the Start menu or the desktop.",
            "Next", Checks);

        async void Checks()
        {
            Page("Checking this PC", "This takes a few seconds.", "Next", null, Welcome);
            next.Enabled = false;
            info ??= await Task.Run(Engine.Inspect);
            long minimum = MinimumBytes();
            if (wanted < minimum)
                wanted = minimum;
            var blockers = Engine.Blockers(info, minimum);
            int top = 0;
            Row(ref top, info.Uefi, "Starts with UEFI");
            Row(ref top, info.Gpt, "Disk: " + info.DiskName + " (" + info.BusType + ")");
            Row(ref top, info.Problem == null && info.MaxAerOSBytes >= minimum, "Free space on C: " + Engine.Gb(info.FreeBytes));
            if (info.SecureBoot == true)
                Row(ref top, null, "Secure Boot is on. You'll turn it off once, after setup (we'll show you how).");
            if (info.BitLocker == true)
                Row(ref top, null, "BitLocker is on. Have your recovery key ready (aka.ms/myrecoverykey).");
            CheckBox confirm = null;
            if (info.BitLocker == true && info.SecureBoot == true)
            {
                confirm = new CheckBox { Text = "I have my BitLocker recovery key", Location = new Point(0, top + 8), AutoSize = true };
                confirm.CheckedChanged += (s, e) => next.Enabled = blockers.Count == 0 && confirm.Checked;
                content.Controls.Add(confirm);
                top += 36;
            }
            foreach (var blocker in blockers)
            {
                Text_(blocker, top, 40, Color.Firebrick);
                top += 40;
            }
            title.Text = blockers.Count == 0 ? "This PC can run AerOS" : "AerOS can't be installed yet";
            body.Text = blockers.Count == 0 ? "" : "Fix the items in red, then run setup again.";
            next.Enabled = blockers.Count == 0 && (confirm == null || confirm.Checked);
            onNext = SizePage;
        }

        void Row(ref int top, bool? ok, string text)
        {
            string mark = ok == true ? "✔" : ok == false ? "✖" : "!";
            var color = ok == true ? Color.SeaGreen : ok == false ? Color.Firebrick : Color.DarkOrange;
            var markLabel = new Label { Text = mark, Location = new Point(0, top), Size = new Size(24, 24), ForeColor = color, Font = new Font("Segoe UI Symbol", 11f) };
            content.Controls.Add(markLabel);
            var label = Text_(text, top, 40);
            label.Left = 28;
            label.Width = 612;
            top += 32;
        }

        long MinimumBytes()
        {
            long payload = Payload.Present ? Payload.UnpackedBytes() : 2 * Engine.GiB;
            return (payload + Engine.GiB + Engine.GiB - 1) / Engine.GiB * Engine.GiB;
        }

        void SizePage()
        {
            long minimum = MinimumBytes();
            long maximum = Math.Max(minimum, info.MaxAerOSBytes / Engine.GiB * Engine.GiB);
            Page("How much space for AerOS?", "AerOS keeps its apps and files in this space. You can remove AerOS later to get it back.",
                "Install", Confirm, Checks);
            var slider = new TrackBar
            {
                Minimum = (int)(minimum / Engine.GiB),
                Maximum = (int)(maximum / Engine.GiB),
                Value = (int)Math.Max(minimum, Math.Min(maximum, wanted)) / (int)Engine.GiB,
                TickFrequency = 2,
                Location = new Point(0, 20),
                Width = 640,
            };
            var label = Text_("", 80);
            void Update()
            {
                wanted = slider.Value * Engine.GiB;
                label.Text = "AerOS: " + slider.Value + " GB        Windows keeps: " + Engine.Gb(info.PartitionSize - wanted);
            }
            slider.ValueChanged += (s, e) => Update();
            content.Controls.Add(slider);
            Update();
        }

        void Confirm()
        {
            if (MessageBox.Show("Setup will shrink drive C: by " + Engine.Gb(wanted) + " and add AerOS to the boot menu.\n\n" +
                    "Close your other apps first. Setup can't be paused once it starts.",
                    "Ready to install", MessageBoxButtons.OKCancel, MessageBoxIcon.Information) == DialogResult.OK)
                Work("Installing AerOS", () => Engine.Install(info, wanted, Report), Finished);
        }

        void Finished()
        {
            Page("AerOS is installed",
                "Windows still starts by default. To use AerOS, open \"AerOS\" from the Start menu or the desktop and your PC restarts into it.",
                "Restart into AerOS", () => Restart(), null);
            cancel.Text = "Close";
            if (info.SecureBoot == true)
            {
                int top = 0;
                Text_("One more step: turn off Secure Boot", top, 24).Font = new Font("Segoe UI Semibold", 11f);
                Text_("AerOS isn't signed by Microsoft yet, so the PC won't start it while Secure Boot is on. You only do this once:", top += 28, 40);
                string[] steps =
                {
                    "1.  Restart the PC and keep pressing F2 as it starts (ASUS; other brands use F2, F10, Del or Esc).",
                    "2.  Open Advanced Mode (F7), go to Security, then Secure Boot.",
                    "3.  Set Secure Boot Control to Disabled, then press F10 to save and restart.",
                };
                foreach (var step in steps)
                    Text_(step, top += 34, 40);
                Text_("Windows keeps working with Secure Boot off.", top += 44, 24, Color.DimGray);
            }
        }

        void Restart()
        {
            try { Engine.RestartIntoAerOS(info?.Installed ?? Engine.ReadRecord()); }
            catch (Exception error) { MessageBox.Show(error.Message, "AerOS", MessageBoxButtons.OK, MessageBoxIcon.Error); }
        }

        // ---- Uninstall and tools ---------------------------------------------

        void ConfirmUninstall()
        {
            var record = Engine.ReadRecord();
            if (record == null)
            {
                Page("AerOS isn't installed", "There's nothing to remove.", null, null);
                cancel.Text = "Close";
                return;
            }
            Page("Remove AerOS?", "This deletes AerOS and everything saved inside it, removes it from the boot menu, and gives the space back to drive C:.",
                "Remove", () => Work("Removing AerOS", () => Engine.Uninstall(record, Report), () =>
                {
                    Page("AerOS has been removed", "Its space is back on drive C:.", null, null);
                    cancel.Text = "Close";
                }));
        }

        void ToolsPage()
        {
            var record = Engine.ReadRecord();
            if (record == null)
            {
                Page("AerOS isn't installed", "Run AerOS Setup to install it.", null, null);
                cancel.Text = "Close";
                return;
            }
            Page("AerOS", "Your PC restarts into AerOS. Restarting from AerOS brings you back to Windows.",
                "Restart into AerOS", () => Restart(), null);
            cancel.Text = "Close";
            var remove = new LinkLabel { Text = "Remove AerOS from this PC", Location = new Point(0, 20), AutoSize = true };
            remove.Click += (s, e) => ConfirmUninstall();
            content.Controls.Add(remove);
        }

        // ---- Progress ---------------------------------------------------------

        ProgressBar bar;
        Label step;

        void Report(int percent, string text) => BeginInvoke((Action)(() =>
        {
            if (percent >= 0)
                bar.Value = Math.Min(100, percent);
            step.Text = text;
        }));

        async void Work(string heading, Action work, Action done)
        {
            Page(heading, "Don't turn off your PC.", null, null);
            busy = true;
            cancel.Enabled = false;
            bar = new ProgressBar { Location = new Point(0, 20), Size = new Size(640, 22), Maximum = 100 };
            step = Text_("Starting", 56);
            content.Controls.Add(bar);
            try
            {
                await Task.Run(work);
                busy = false;
                cancel.Enabled = true;
                done();
            }
            catch (Exception error)
            {
                busy = false;
                cancel.Enabled = true;
                Page("Something went wrong", error.Message + "\n\nSetup undid its changes. Details: " + Engine.LogPath, null, null);
                cancel.Text = "Close";
            }
        }
    }
}
