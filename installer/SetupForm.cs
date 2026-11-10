using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Linq;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace AerOSSetup
{
    /// One glass panel floating over the desktop. Everything inside it is
    /// drawn here (the pages, the option pills, buttons and progress), at
    /// 96-dpi coordinates scaled to the monitor.
    sealed class SetupForm : Form
    {
        public enum Mode { Install, Uninstall, Tools }

        const int W = 666, H = 627, Margin_ = 34, Radius = 36;
        static readonly Color Blue = Color.FromArgb(52, 120, 246);
        static readonly Color Ink = Color.FromArgb(28, 28, 32);
        static readonly Color Muted = Color.FromArgb(96, 98, 108);

        readonly Mode mode;
        readonly string family;
        readonly Timer ticker = new Timer { Interval = 33 };
        readonly List<(RectangleF area, string id, Action click)> hits = new List<(RectangleF, string, Action)>();
        Action<Graphics> page;
        string hover, pressed;
        float scale = 1, spin;
        bool busy, dragging, sliderHeld;
        string picked;

        SystemInfo info;
        long wanted = 8 * Engine.GiB;
        bool usb, recoveryKey;
        List<UsbDrive> drives;
        UsbDrive drive;
        int percent;
        string stepText = "";

        public SetupForm(Mode mode, string preview = null)
        {
            this.mode = mode;
            Text = mode == Mode.Uninstall ? "Remove AerOS" : "AerOS Installer";
            FormBorderStyle = FormBorderStyle.None;
            StartPosition = FormStartPosition.Manual;
            ShowInTaskbar = true;
            KeyPreview = true;
            try { Icon = Icon.ExtractAssociatedIcon(Payload.ExePath); } catch { }
            var installed = new InstalledFontCollection().Families.Select(f => f.Name).ToList();
            family = new[] { "Segoe UI Variable Display", "Segoe UI" }.FirstOrDefault(installed.Contains) ?? FontFamily.GenericSansSerif.Name;

            ticker.Tick += (s, e) =>
            {
                spin = (spin + 11) % 360;
                Render();
            };
            FormClosing += (s, e) => e.Cancel = busy;

            switch (mode)
            {
                case Mode.Install: Preview(preview); break;
                case Mode.Uninstall: ConfirmUninstall(); break;
                default: ToolsPage(); break;
            }
        }

        protected override CreateParams CreateParams
        {
            get
            {
                var cp = base.CreateParams;
                cp.ExStyle |= 0x00080000; // WS_EX_LAYERED
                return cp;
            }
        }

        protected override void OnHandleCreated(EventArgs e)
        {
            base.OnHandleCreated(e);
            scale = GetDpiForWindow(Handle) / 96f;
            var work = Screen.FromPoint(Cursor.Position).WorkingArea;
            var size = Outer();
            Location = new Point(work.X + (work.Width - size.Width) / 2, work.Y + (work.Height - size.Height) / 2);
            Size = size;
            Render();
        }

        protected override void OnDpiChanged(DpiChangedEventArgs e)
        {
            scale = e.DeviceDpiNew / 96f;
            Size = Outer();
            Location = e.SuggestedRectangle.Location;
            Render();
        }

        protected override void OnMove(EventArgs e)
        {
            base.OnMove(e);
            if (IsHandleCreated)
                Compose();
        }

        Size Outer() => new Size((int)Math.Ceiling((W + Margin_ * 2) * scale), (int)Math.Ceiling((H + Margin_ * 2) * scale));

        // ---- Drawing -----------------------------------------------------------

        Glass.Surface surface;
        Bitmap glassLayer, overlay;
        bool contentStale = true;

        /// The page changed: redraw it, then show it.
        void Render()
        {
            contentStale = true;
            Compose();
        }

        Rectangle PanelPixels()
        {
            int m = (int)Math.Round(Margin_ * scale);
            return new Rectangle(m, m, (int)Math.Round(W * scale), (int)Math.Round(H * scale));
        }

        /// Puts the frame together: the wallpaper behind the panel, then one
        /// cached layer with the glass and the page. Moving the window only
        /// redoes the first part, which is a plain copy.
        void Compose()
        {
            if (!IsHandleCreated)
                return;
            var size = Outer();
            var panel = PanelPixels();
            if (surface == null || surface.Size != size)
            {
                surface?.Dispose();
                glassLayer?.Dispose();
                overlay?.Dispose();
                surface = new Glass.Surface(size);
                glassLayer = new Bitmap(size.Width, size.Height, PixelFormat.Format32bppPArgb);
                using (var g = Graphics.FromImage(glassLayer))
                {
                    g.SmoothingMode = SmoothingMode.AntiAlias;
                    Glass.Overlay(g, panel, Radius * scale, scale);
                }
                overlay = new Bitmap(size.Width, size.Height, PixelFormat.Format32bppPArgb);
                contentStale = true;
            }
            var whole = new Rectangle(Point.Empty, size);
            if (contentStale)
            {
                contentStale = false;
                using var g = Graphics.FromImage(overlay);
                g.CompositingMode = CompositingMode.SourceCopy;
                g.DrawImage(glassLayer, whole, 0, 0, size.Width, size.Height, GraphicsUnit.Pixel);
                g.CompositingMode = CompositingMode.SourceOver;
                g.SmoothingMode = SmoothingMode.AntiAlias;
                g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
                g.TranslateTransform(panel.X, panel.Y);
                g.ScaleTransform(scale, scale);
                hits.Clear();
                Chrome(g);
                page?.Invoke(g);
            }
            using (var g = Graphics.FromImage(surface.Bitmap))
            {
                g.CompositingMode = CompositingMode.SourceCopy;
                g.Clear(Color.Transparent);
            }
            Glass.Backdrop(surface, panel, (int)Math.Round(Radius * scale), new Point(Left + panel.X, Top + panel.Y));
            using (var g = Graphics.FromImage(surface.Bitmap))
            {
                g.CompositingQuality = CompositingQuality.HighSpeed;
                g.InterpolationMode = InterpolationMode.NearestNeighbor;
                g.DrawImage(overlay, whole, 0, 0, size.Width, size.Height, GraphicsUnit.Pixel);
            }
            surface.Present(Handle, Location);
        }

        Font F(float size, FontStyle style = FontStyle.Regular) => new Font(family, size, style, GraphicsUnit.Pixel);

        static readonly Brush Soft = new SolidBrush(Color.FromArgb(200, 255, 255, 255));

        void Chrome(Graphics g)
        {
            using (var font = F(16))
            using (var brush = new SolidBrush(Color.FromArgb(240, 255, 255, 255)))
                g.DrawString(mode == Mode.Uninstall ? "AerOS" : "AerOS Installer", font, brush, 62, 48);
            if (busy)
                return;
            var close = new RectangleF(W - 66, 44, 30, 30);
            Hit(close, "close", Close);
            if (hover == "close")
                using (var back = new SolidBrush(Color.FromArgb(40, 255, 255, 255)))
                    g.FillEllipse(back, close);
            using var pen = new Pen(Color.FromArgb(hover == "close" ? 230 : 150, 255, 255, 255), 1.6f);
            float c = close.X + 15, m = close.Y + 15;
            g.DrawLine(pen, c - 5, m - 5, c + 5, m + 5);
            g.DrawLine(pen, c + 5, m - 5, c - 5, m + 5);
        }

        void Hit(RectangleF area, string id, Action click) => hits.Add((area, id, click));

        void Heading(Graphics g, string text)
        {
            using var font = F(21);
            using var format = new StringFormat { Alignment = StringAlignment.Center };
            g.DrawString(text, font, Brushes.White, new RectangleF(36, 120, W - 72, 60), format);
        }

        float Paragraph(Graphics g, string text, float top, float size = 16, Brush brush = null, float left = Left_, float width = W - Left_ * 2)
        {
            using var font = F(size);
            var measured = g.MeasureString(text, font, (int)width);
            g.DrawString(text, font, brush ?? Brushes.White, new RectangleF(left, top, width, measured.Height + 2));
            return top + measured.Height;
        }

        const float Left_ = 38;

        /// The light-grey option with a blue icon on the left and a radio on the right.
        void Option(Graphics g, float top, string id, string title, string subtitle, Action<Graphics, PointF> icon, bool selected, Action click, bool enabled = true)
        {
            var area = new RectangleF((W - 528) / 2f, top, 528, 70);
            if (enabled)
                Hit(area, id, click);
            using (var path = Glass.Rounded(area, 16))
            {
                var fill = !enabled ? Color.FromArgb(120, 214, 214, 219) : hover == id ? Color.FromArgb(244, 236, 236, 240) : Color.FromArgb(236, 214, 214, 219);
                using var brush = new SolidBrush(fill);
                g.FillPath(brush, path);
            }
            icon?.Invoke(g, new PointF(area.X + 42, area.Y + area.Height / 2));
            using (var font = F(15))
            using (var brush = new SolidBrush(enabled ? Ink : Muted))
                g.DrawString(title, font, brush, area.X + 84, area.Y + 15);
            using (var font = F(12))
            using (var brush = new SolidBrush(Muted))
                g.DrawString(subtitle, font, brush, area.X + 84, area.Y + 37);
            Choice(g, new PointF(area.Right - 42, area.Y + area.Height / 2), selected);
        }

        /// Selected: a blue ring around a white dot. Otherwise a plain blue dot.
        static void Choice(Graphics g, PointF c, bool on)
        {
            using var blue = new SolidBrush(Blue);
            g.FillEllipse(blue, c.X - 16, c.Y - 16, 32, 32);
            if (on)
                g.FillEllipse(Brushes.White, c.X - 9, c.Y - 9, 18, 18);
        }

        /// A tick box drawn as a ring: hollow when off, blue ring and white dot when on.
        static void Toggle(Graphics g, PointF c, bool on)
        {
            if (on)
            {
                Choice(g, c, true);
                return;
            }
            using var ring = new Pen(Color.FromArgb(220, 255, 255, 255), 2.2f);
            g.DrawEllipse(ring, c.X - 15, c.Y - 15, 30, 30);
        }

        void Button(Graphics g, RectangleF area, string id, string text, Action click, bool primary = true, bool enabled = true)
        {
            if (enabled)
                Hit(area, id, click);
            using var path = Glass.Rounded(area, area.Height / 2);
            Color fill = !enabled ? Color.FromArgb(46, 255, 255, 255)
                : primary ? (hover == id ? Color.FromArgb(84, 148, 255) : Blue)
                : Color.FromArgb(hover == id ? 64 : 34, 255, 255, 255);
            using (var brush = new SolidBrush(fill))
                g.FillPath(brush, path);
            using var font = F(15);
            using var format = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center };
            using var ink = new SolidBrush(enabled ? Color.White : Color.FromArgb(130, 255, 255, 255));
            g.DrawString(text, font, ink, area, format);
        }

        /// Up to two buttons along the bottom: the main one on the right.
        void Buttons(Graphics g, string primary, Action next, bool enabled = true, string secondary = null, Action other = null)
        {
            float top = H - 86;
            if (primary != null)
                Button(g, new RectangleF(W - Left_ - 190, top, 190, 46), "next", primary, next, true, enabled);
            if (secondary != null)
                Button(g, new RectangleF(Left_, top, 130, 46), "back", secondary, other, false);
        }

        static readonly PointF SpinnerCentre = new PointF(W / 2f, 226);

        void Spinner(Graphics g, PointF c, float size = 74, float stroke = 6)
        {
            var r = new RectangleF(c.X - size / 2, c.Y - size / 2, size, size);
            using (var track = new Pen(Color.FromArgb(40, 255, 255, 255), stroke))
                g.DrawEllipse(track, r);
            using var arc = new Pen(Color.White, stroke) { StartCap = LineCap.Round, EndCap = LineCap.Round };
            g.DrawArc(arc, r, spin, 270);
        }

        /// The finished spinner: a full ring with a tick or a cross in it.
        static void Result(Graphics g, PointF c, bool ok)
        {
            using var ring = new Pen(ok ? Color.White : Color.FromArgb(255, 150, 140), 6);
            g.DrawEllipse(ring, c.X - 37, c.Y - 37, 74, 74);
            Mark(g, c, ok, 2.2f, ring.Color);
        }

        /// A thin tick or cross centred on `c`, at `size` times the row size.
        static void Mark(Graphics g, PointF c, bool ok, float size = 1, Color? color = null)
        {
            using var pen = new Pen(color ?? Color.White, 1.8f * Math.Min(size, 1.8f)) { StartCap = LineCap.Round, EndCap = LineCap.Round, LineJoin = LineJoin.Round };
            float s = 6 * size;
            if (ok)
                g.DrawLines(pen, new[] { new PointF(c.X - s, c.Y), new PointF(c.X - s / 3, c.Y + s * 0.7f), new PointF(c.X + s, c.Y - s * 0.75f) });
            else
            {
                g.DrawLine(pen, c.X - s * 0.75f, c.Y - s * 0.75f, c.X + s * 0.75f, c.Y + s * 0.75f);
                g.DrawLine(pen, c.X + s * 0.75f, c.Y - s * 0.75f, c.X - s * 0.75f, c.Y + s * 0.75f);
            }
        }

        /// A check line: the text, then a tick or a cross after it.
        void Check(Graphics g, float top, string text, bool? ok)
        {
            using var font = F(16);
            g.DrawString(text, font, Brushes.White, Left_, top);
            if (ok == null)
                return;
            float width = g.MeasureString(text, font).Width;
            Mark(g, new PointF(Left_ + width + 14, top + 11), ok.Value, 1, ok.Value ? Color.White : Color.FromArgb(255, 150, 140));
        }

        static Pen IconPen() => new Pen(Blue, 2.4f) { LineJoin = LineJoin.Round };

        static void DualbootIcon(Graphics g, PointF c)
        {
            using var pen = IconPen();
            using (var back = Glass.Rounded(new RectangleF(c.X - 15, c.Y - 15, 20, 20), 6))
                g.DrawPath(pen, back);
            using (var front = Glass.Rounded(new RectangleF(c.X - 5, c.Y - 5, 20, 20), 6))
                g.DrawPath(pen, front);
        }

        static void UsbIcon(Graphics g, PointF c)
        {
            using var blue = new SolidBrush(Blue);
            using (var body = Glass.Rounded(new RectangleF(c.X - 17, c.Y - 9, 28, 18), 5))
                g.FillPath(blue, body);
            using (var plug = Glass.Rounded(new RectangleF(c.X + 10, c.Y - 5.5f, 8, 11), 2))
                g.FillPath(blue, plug);
        }

        static void DriveIcon(Graphics g, PointF c)
        {
            using var pen = IconPen();
            using (var body = Glass.Rounded(new RectangleF(c.X - 16, c.Y - 8, 32, 16), 4))
                g.DrawPath(pen, body);
            using var dot = new SolidBrush(Blue);
            g.FillEllipse(dot, c.X + 7, c.Y - 2, 4, 4);
        }

        static void RemoveIcon(Graphics g, PointF c)
        {
            using var pen = IconPen();
            g.DrawLine(pen, c.X - 12, c.Y - 9, c.X + 12, c.Y - 9);
            g.DrawLine(pen, c.X - 4, c.Y - 13, c.X + 4, c.Y - 13);
            using var bin = Glass.Rounded(new RectangleF(c.X - 9, c.Y - 9, 18, 22), 3);
            g.DrawPath(pen, bin);
        }

        // ---- Input -------------------------------------------------------------

        string HitAt(Point p, out Action click)
        {
            var at = new PointF(p.X / scale - Margin_, p.Y / scale - Margin_);
            for (int i = hits.Count - 1; i >= 0; i--)
                if (hits[i].area.Contains(at))
                {
                    click = hits[i].click;
                    return hits[i].id;
                }
            click = null;
            return null;
        }

        [System.Runtime.InteropServices.DllImport("user32.dll")] static extern bool ReleaseCapture();
        [System.Runtime.InteropServices.DllImport("user32.dll")] static extern int GetDpiForWindow(IntPtr window);
        [System.Runtime.InteropServices.DllImport("user32.dll")] static extern IntPtr SendMessage(IntPtr window, int message, IntPtr w, IntPtr l);

        protected override void OnMouseMove(MouseEventArgs e)
        {
            if (sliderHeld)
            {
                sliding?.Invoke(e.X / scale - Margin_);
                return;
            }
            var id = HitAt(e.Location, out _);
            Cursor = id != null ? Cursors.Hand : Cursors.Default;
            if (id != hover)
            {
                hover = id;
                Render();
            }
        }

        protected override void OnMouseLeave(EventArgs e)
        {
            if (hover != null)
            {
                hover = null;
                Render();
            }
        }

        protected override void OnMouseDown(MouseEventArgs e)
        {
            if (e.Button != MouseButtons.Left)
                return;
            pressed = HitAt(e.Location, out _);
            if (pressed == "slider" && sliding != null)
            {
                sliderHeld = true;
                Capture = true;
                sliding(e.X / scale - Margin_);
                return;
            }
            if (pressed == null && !dragging)
            {
                dragging = true;
                ReleaseCapture();
                SendMessage(Handle, 0xA1, (IntPtr)2, IntPtr.Zero); // drag by the caption
                dragging = false;
            }
        }

        protected override void OnMouseUp(MouseEventArgs e)
        {
            if (sliderHeld)
            {
                sliderHeld = false;
                Capture = false;
                pressed = null;
                return;
            }
            var id = HitAt(e.Location, out var click);
            if (id != null && id == pressed)
                click?.Invoke();
            pressed = null;
        }

        protected override bool ProcessCmdKey(ref Message msg, Keys keys)
        {
            if (keys == Keys.Escape && !busy)
            {
                Close();
                return true;
            }
            if (keys == Keys.Enter)
            {
                var next = hits.FirstOrDefault(h => h.id == "next");
                next.click?.Invoke();
                return true;
            }
            return base.ProcessCmdKey(ref msg, keys);
        }

        void Show(Action<Graphics> view, bool spinner = false)
        {
            page = view;
            hover = null;
            ticker.Enabled = spinner;
            Render();
        }

        // ---- Install -----------------------------------------------------------

        /// `--preview <page>` opens straight at one screen (nothing is changed).
        void Preview(string start)
        {
            switch (start)
            {
                case "checks": Checks(); break;
                case "usb": UsbPick(); break;
                case "warnings":
                case "size":
                    info = Engine.Inspect();
                    info.SecureBoot = info.BitLocker = true;
                    if (start == "size") SizePage(); else Warnings();
                    break;
                case "work": Work("Installing AerOS", () => { Report(42, "Copying AerOS (0.8 GB of 1.9 GB)"); System.Threading.Thread.Sleep(600000); }, Choose); break;
                case "done":
                    info = new SystemInfo { SecureBoot = true };
                    Finished();
                    break;
                default: Choose(); break;
            }
        }

        void Choose() => Show(g =>
        {
            Heading(g, "Choose how you'd like to use AerOS.");
            Option(g, 258, "dual", "Dualboot", "Keep Windows and switch to AerOS anytime", DualbootIcon, picked != "usb", () => Pick(false));
            Option(g, 362, "usb", "Make a bootable USB", "Run AerOS from a USB stick", UsbIcon, picked == "usb", () => Pick(true));
        });

        void Pick(bool toUsb)
        {
            usb = toUsb;
            picked = toUsb ? "usb" : "dual";
            Render();
            var delay = new Timer { Interval = 220 };
            delay.Tick += (s, e) =>
            {
                delay.Dispose();
                picked = null;
                if (usb)
                    UsbPick();
                else
                    Checks();
            };
            delay.Start();
        }

        async void Checks()
        {
            Show(g =>
            {
                Heading(g, "Checking PC specifications");
                Spinner(g, SpinnerCentre);
                Check(g, 310, "Starts with UEFI", null);
                Check(g, 356, "Disk :", null);
                Check(g, 402, "Free space on C:", null);
            }, spinner: true);
            info ??= await Task.Run(Engine.Inspect);
            long minimum = MinimumBytes();
            if (wanted < minimum)
                wanted = minimum;
            var blockers = Engine.Blockers(info, minimum);
            bool warnings = info.SecureBoot == true || info.BitLocker == true;
            Show(g =>
            {
                Heading(g, "Checking PC specifications");
                Result(g, SpinnerCentre, blockers.Count == 0);
                Check(g, 310, "Starts with UEFI", info.Uefi);
                Check(g, 356, "Disk : " + (string.IsNullOrEmpty(info.DiskName) ? "unknown" : info.DiskName), info.Gpt && info.Problem == null);
                Check(g, 402, "Free space on C: " + Engine.Gb(info.FreeBytes), info.Problem == null && info.MaxAerOSBytes >= minimum);
                if (blockers.Count > 0)
                {
                    using var red = new SolidBrush(Color.FromArgb(255, 170, 160));
                    Paragraph(g, string.Join("\n", blockers), 452, 14, red);
                    Buttons(g, "Close", Close, true, "Back", Choose);
                }
                else
                    Buttons(g, "Continue", warnings ? Warnings : SizePage, true, "Back", Choose);
            });
        }

        void Warnings() => Show(g =>
        {
            Heading(g, "Warnings");
            float top = 204;
            if (info.SecureBoot == true)
                top = Paragraph(g, "Secure Boot is on. You'll turn it off once after the setup. We'll show you how.", top) + 28;
            if (info.BitLocker == true)
            {
                top = Paragraph(g, "BitLocker is on. Have your recovery key ready (aka.ms/myrecoverykey).", top) + 30;
                using var font = F(16);
                const string label = "I have my recovery key";
                float width = g.MeasureString(label, font).Width;
                Hit(new RectangleF(Left_ - 8, top - 10, width + 70, 44), "key", () =>
                {
                    recoveryKey = !recoveryKey;
                    Render();
                });
                g.DrawString(label, font, hover == "key" ? Brushes.White : Soft, Left_, top);
                Toggle(g, new PointF(Left_ + width + 30, top + 11), recoveryKey);
            }
            Buttons(g, "Continue", SizePage, info.BitLocker != true || recoveryKey, "Back", Checks);
        });

        long MinimumBytes()
        {
            long payload = Payload.Present ? Payload.UnpackedBytes() : 2 * Engine.GiB;
            return (payload + Engine.GiB + Engine.GiB - 1) / Engine.GiB * Engine.GiB;
        }

        void SizePage()
        {
            long minimum = MinimumBytes();
            long maximum = Math.Max(minimum, info.MaxAerOSBytes / Engine.GiB * Engine.GiB);
            wanted = Math.Max(minimum, Math.Min(maximum, wanted));
            var track = new RectangleF(Left_ + 10, 320, W - Left_ * 2 - 20, 8);
            sliding = x =>
            {
                float t = Math.Max(0, Math.Min(1, (x - track.X) / track.Width));
                wanted = (long)Math.Round((minimum + t * (maximum - minimum)) / Engine.GiB) * Engine.GiB;
                Render();
            };
            Show(g =>
            {
                Heading(g, "How much space for AerOS?");
                Paragraph(g, "AerOS keeps its apps and files here. You can remove AerOS later to get the space back.", 204, 16, Soft);
                float t = maximum == minimum ? 1 : (wanted - minimum) / (float)(maximum - minimum);
                using (var path = Glass.Rounded(track, 4))
                using (var fill = new SolidBrush(Color.FromArgb(50, 255, 255, 255)))
                    g.FillPath(fill, path);
                using (var path = Glass.Rounded(new RectangleF(track.X, track.Y, Math.Max(8, track.Width * t), track.Height), 4))
                using (var fill = new SolidBrush(Blue))
                    g.FillPath(fill, path);
                var knob = new RectangleF(track.X + track.Width * t - 13, track.Y - 9, 26, 26);
                Hit(RectangleF.Inflate(new RectangleF(track.X, track.Y - 14, track.Width, 36), 14, 0), "slider", null);
                using (var shadow = new SolidBrush(Color.FromArgb(50, 0, 0, 0)))
                    g.FillEllipse(shadow, knob.X, knob.Y + 1.5f, knob.Width, knob.Height);
                g.FillEllipse(Brushes.White, knob);
                Paragraph(g, "AerOS: " + Engine.Gb(wanted), 370);
                Paragraph(g, "Windows keeps: " + Engine.Gb(info.PartitionSize - wanted), 414);
                Buttons(g, "Continue", Ready, true, "Back", () =>
                {
                    sliding = null;
                    if (info.SecureBoot == true || info.BitLocker == true)
                        Warnings();
                    else
                        Checks();
                });
            });
        }

        Action<float> sliding;

        void Ready()
        {
            sliding = null;
            Show(g =>
            {
                Heading(g, "Ready to install");
                float top = Paragraph(g, "Setup shrinks drive C: by " + Engine.Gb(wanted) + " and adds AerOS to the boot menu. Windows and your files stay as they are.", 204);
                Paragraph(g, "Close your other apps first. Setup can't be paused once it starts.", top + 24, 16, Soft);
                Buttons(g, "Install", () => Work("Installing AerOS", () => Engine.Install(info, wanted, Report), Finished), true, "Back", SizePage);
            });
        }

        void Finished() => Show(g =>
        {
            Heading(g, "AerOS is installed");
            if (info.SecureBoot == true)
            {
                float top = Paragraph(g, "One more step: turn off Secure Boot. AerOS isn't signed by Microsoft yet, so your PC won't start it until you do. You only do this once:", 190, 15);
                string[] steps =
                {
                    "Restart and keep pressing F2 as the PC starts (other brands: F10, Del or Esc).",
                    "Open Advanced Mode (F7), then Security, then Secure Boot.",
                    "Set Secure Boot Control to Disabled. Press F10 to save and restart.",
                };
                top += 14;
                for (int i = 0; i < steps.Length; i++)
                {
                    using (var badge = new SolidBrush(Color.FromArgb(46, 255, 255, 255)))
                        g.FillEllipse(badge, Left_, top, 24, 24);
                    using (var font = F(13))
                    using (var format = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center })
                        g.DrawString((i + 1).ToString(), font, Brushes.White, new RectangleF(Left_, top, 24, 24), format);
                    top = Paragraph(g, steps[i], top + 2, 15, null, Left_ + 38, W - Left_ * 2 - 38) + 12;
                }
                Paragraph(g, "Windows keeps working with Secure Boot off.", top, 14, Soft);
            }
            else
                Paragraph(g, "Windows still starts by default. To use AerOS, open \"AerOS\" from the Start menu or the desktop and your PC restarts into it.", 204);
            Buttons(g, "Restart into AerOS", Restart, true, "Close", Close);
        });

        void Restart()
        {
            try { Engine.RestartIntoAerOS(info?.Installed ?? Engine.ReadRecord()); }
            catch (Exception error) { MessageBox.Show(this, error.Message, "AerOS", MessageBoxButtons.OK, MessageBoxIcon.Error); }
        }

        // ---- Bootable USB --------------------------------------------------------

        async void UsbPick()
        {
            drive = null;
            Show(g =>
            {
                Heading(g, "Choose a USB stick");
                Spinner(g, SpinnerCentre);
            }, spinner: true);
            try { drives = await Task.Run(Engine.UsbDrives); }
            catch (Exception error) { Engine.Log("usb list: " + error); drives = new List<UsbDrive>(); }
            long minimum = MinimumBytes();
            Show(g =>
            {
                Heading(g, "Choose a USB stick");
                if (drives.Count == 0)
                {
                    Paragraph(g, "Plug in a USB stick (" + Engine.Gb(minimum) + " or bigger), then press Look again.", 204);
                    Buttons(g, "Look again", UsbPick, true, "Back", Choose);
                    return;
                }
                float top = 200;
                foreach (var d in drives.Take(3))
                {
                    bool fits = d.Size >= minimum;
                    var current = d;
                    Option(g, top, "drive" + d.Number, d.Name, fits ? Engine.Gb(d.Size) : Engine.Gb(d.Size) + ", too small", DriveIcon, drive == d,
                        () => { drive = current; Render(); }, fits);
                    top += 88;
                }
                Paragraph(g, "Everything on the stick you pick is erased.", top + 2, 14, Soft);
                Buttons(g, "Make USB", UsbConfirm, drive != null, "Back", Choose);
            });
        }

        void UsbConfirm() => Show(g =>
        {
            Heading(g, "Erase " + drive.Name + "?");
            Paragraph(g, "Everything on this USB stick (" + Engine.Gb(drive.Size) + ") is deleted and replaced with AerOS. This can't be undone.", 204);
            Buttons(g, "Erase and write", () => Work("Making your AerOS USB", () => Engine.MakeUsb(drive, Report), UsbDone), true, "Back", UsbPick);
        });

        void UsbDone() => Show(g =>
        {
            Heading(g, "Your USB stick is ready");
            float top = Paragraph(g, "To start AerOS from it: plug it into a PC, restart, and pick the USB stick in the boot menu (usually F8, F12 or Esc as the PC starts).", 204);
            Paragraph(g, "Turn Secure Boot off first. AerOS isn't signed by Microsoft yet.", top + 24, 16, Soft);
            Buttons(g, "Done", Close);
        });

        // ---- Uninstall and tools -------------------------------------------------

        void ConfirmUninstall()
        {
            var record = Engine.ReadRecord();
            if (record == null)
            {
                Message("AerOS isn't installed", "There's nothing to remove.");
                return;
            }
            Show(g =>
            {
                Heading(g, "Remove AerOS?");
                Paragraph(g, "This deletes AerOS and everything saved inside it, removes it from the boot menu, and gives the space back to drive C:.", 204);
                Buttons(g, "Remove", () => Work("Removing AerOS", () => Engine.Uninstall(record, Report),
                    () => Message("AerOS has been removed", "Its space is back on drive C:.")), true, "Cancel", Close);
            });
        }

        void ToolsPage()
        {
            var record = Engine.ReadRecord();
            if (record == null)
            {
                Message("AerOS isn't installed", "Run AerOS Setup to install it.");
                return;
            }
            Show(g =>
            {
                Heading(g, "AerOS");
                Paragraph(g, "Your PC restarts into AerOS. Restarting from AerOS brings you back to Windows.", 204);
                Option(g, 300, "remove", "Remove AerOS", "Delete AerOS and give its space back to C:", RemoveIcon, false, ConfirmUninstall);
                Buttons(g, "Restart into AerOS", Restart, true, "Close", Close);
            });
        }

        void Message(string heading, string text) => Show(g =>
        {
            Heading(g, heading);
            Paragraph(g, text, 204);
            Buttons(g, "Close", Close);
        });

        // ---- Progress -------------------------------------------------------------

        void Report(int value, string text) => BeginInvoke((Action)(() =>
        {
            if (value >= 0)
                percent = Math.Min(100, value);
            stepText = text;
            Render();
        }));

        async void Work(string heading, Action work, Action done)
        {
            busy = true;
            percent = 0;
            stepText = "Starting";
            Show(g =>
            {
                Heading(g, heading);
                Spinner(g, SpinnerCentre);
                var track = new RectangleF(Left_, 330, W - Left_ * 2, 8);
                using (var path = Glass.Rounded(track, 4))
                using (var fill = new SolidBrush(Color.FromArgb(46, 255, 255, 255)))
                    g.FillPath(fill, path);
                if (percent > 0)
                    using (var path = Glass.Rounded(new RectangleF(track.X, track.Y, Math.Max(8, track.Width * percent / 100f), track.Height), 4))
                    using (var fill = new SolidBrush(Blue))
                        g.FillPath(fill, path);
                Paragraph(g, stepText, 356, 16, null, Left_, W - Left_ * 2 - 70);
                using (var font = F(16))
                using (var format = new StringFormat { Alignment = StringAlignment.Far })
                    g.DrawString(percent + "%", font, Brushes.White, new RectangleF(W - Left_ - 70, 356, 70, 24), format);
                Paragraph(g, "Don't turn off your PC.", 410, 14, Soft);
            }, spinner: true);
            try
            {
                await Task.Run(work);
                busy = false;
                done();
            }
            catch (Exception error)
            {
                busy = false;
                Show(g =>
                {
                    Heading(g, "Something went wrong");
                    float top = Paragraph(g, error.Message, 204);
                    Paragraph(g, "Setup undid its changes. Details: " + Engine.LogPath, top + 16, 14, Soft);
                    Buttons(g, "Close", Close);
                });
            }
        }
    }
}
