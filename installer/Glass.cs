using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;
using Microsoft.Win32;

namespace AerOSSetup
{
    /// Frosted glass the way the AerOS desktop draws it: the wallpaper behind
    /// the window, heavily blurred, slightly magnified (refraction), tinted,
    /// lit from the top left with a coloured rim.
    static class Glass
    {
        const int Shrink = 10;
        static Bitmap blurred;
        static Rectangle blurredScreen;
        const float Zoom = 1.08f;
        static Bitmap backdrop;

        /// The blurred wallpaper of the monitor `screen`, enlarged once to
        /// screen size (times the refraction zoom) so a frame only copies it.
        static Bitmap Prepared(Rectangle screen)
        {
            if (backdrop != null && blurredScreen == screen)
                return backdrop;
            backdrop?.Dispose();
            blurred?.Dispose();
            blurredScreen = screen;
            int w = Math.Max(1, screen.Width / Shrink), h = Math.Max(1, screen.Height / Shrink);
            blurred = new Bitmap(w, h, PixelFormat.Format32bppArgb);
            using (var g = Graphics.FromImage(blurred))
            {
                g.InterpolationMode = InterpolationMode.HighQualityBilinear;
                g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                using var wallpaper = LoadWallpaper();
                if (wallpaper == null)
                {
                    using var fill = new LinearGradientBrush(new Rectangle(0, 0, w, h), Color.FromArgb(58, 44, 110), Color.FromArgb(24, 52, 112), 60f);
                    g.FillRectangle(fill, 0, 0, w, h);
                }
                else
                {
                    float cover = Math.Max(w / (float)wallpaper.Width, h / (float)wallpaper.Height);
                    float dw = wallpaper.Width * cover, dh = wallpaper.Height * cover;
                    using var attributes = new ImageAttributes();
                    attributes.SetWrapMode(WrapMode.TileFlipXY);
                    g.DrawImage(wallpaper, new Rectangle((int)((w - dw) / 2), (int)((h - dh) / 2), (int)Math.Ceiling(dw), (int)Math.Ceiling(dh)),
                        0, 0, wallpaper.Width, wallpaper.Height, GraphicsUnit.Pixel, attributes);
                }
            }
            for (int pass = 0; pass < 3; pass++)
                BoxBlur(blurred, 2);
            backdrop = new Bitmap((int)(w * Shrink * Zoom), (int)(h * Shrink * Zoom), PixelFormat.Format32bppPArgb);
            using (var g = Graphics.FromImage(backdrop))
            using (var attributes = new ImageAttributes())
            {
                attributes.SetWrapMode(WrapMode.TileFlipXY);
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                g.DrawImage(blurred, new Rectangle(0, 0, backdrop.Width, backdrop.Height), 0, 0, w, h, GraphicsUnit.Pixel, attributes);
            }
            return backdrop;
        }

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        static extern bool SystemParametersInfo(int action, int size, StringBuilder value, int ini);

        static Image LoadWallpaper()
        {
            var path = new StringBuilder(520);
            string[] candidates =
            {
                SystemParametersInfo(0x73, path.Capacity, path, 0) ? path.ToString() : null,
                Registry.GetValue(@"HKEY_CURRENT_USER\Control Panel\Desktop", "WallPaper", null) as string,
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), @"Microsoft\Windows\Themes\TranscodedWallpaper"),
            };
            foreach (var candidate in candidates)
            {
                if (string.IsNullOrEmpty(candidate) || !File.Exists(candidate))
                    continue;
                try
                {
                    using var file = File.OpenRead(candidate);
                    using var image = Image.FromStream(file);
                    return new Bitmap(image);
                }
                catch { }
            }
            return null;
        }

        /// One horizontal and one vertical running-average pass.
        static void BoxBlur(Bitmap bitmap, int radius)
        {
            var area = new Rectangle(0, 0, bitmap.Width, bitmap.Height);
            var data = bitmap.LockBits(area, ImageLockMode.ReadWrite, PixelFormat.Format32bppArgb);
            int w = bitmap.Width, h = bitmap.Height, stride = data.Stride;
            var pixels = new byte[stride * h];
            Marshal.Copy(data.Scan0, pixels, 0, pixels.Length);
            var copy = new byte[pixels.Length];
            for (int axis = 0; axis < 2; axis++)
            {
                Buffer.BlockCopy(pixels, 0, copy, 0, pixels.Length);
                int lines = axis == 0 ? h : w, length = axis == 0 ? w : h;
                for (int line = 0; line < lines; line++)
                    for (int c = 0; c < 3; c++)
                        for (int i = 0; i < length; i++)
                        {
                            int sum = 0, count = 0;
                            for (int k = Math.Max(0, i - radius); k <= Math.Min(length - 1, i + radius); k++)
                            {
                                sum += copy[axis == 0 ? line * stride + k * 4 + c : k * stride + line * 4 + c];
                                count++;
                            }
                            pixels[axis == 0 ? line * stride + i * 4 + c : i * stride + line * 4 + c] = (byte)(sum / count);
                        }
            }
            Marshal.Copy(pixels, 0, data.Scan0, pixels.Length);
            bitmap.UnlockBits(data);
        }

        public static GraphicsPath Rounded(RectangleF r, float radius)
        {
            var path = new GraphicsPath();
            float d = Math.Min(radius * 2, Math.Min(r.Width, r.Height));
            path.AddArc(r.X, r.Y, d, d, 180, 90);
            path.AddArc(r.Right - d, r.Y, d, d, 270, 90);
            path.AddArc(r.Right - d, r.Bottom - d, d, d, 0, 90);
            path.AddArc(r.X, r.Bottom - d, d, d, 90, 90);
            path.CloseFigure();
            return path;
        }

        /// Copies the blurred wallpaper behind `panel` into the surface and
        /// rounds its corners. `screenOrigin` is the desktop position of the
        /// panel's top-left corner.
        public static void Backdrop(Surface surface, Rectangle panel, int radius, Point screenOrigin)
        {
            var screen = Screen.FromPoint(new Point(screenOrigin.X + panel.Width / 2, screenOrigin.Y + panel.Height / 2)).Bounds;
            var source = Prepared(screen);
            // Refraction: the middle of the slab shows what's behind its middle, enlarged.
            float cx = (screenOrigin.X - screen.X + panel.Width / 2f) * Zoom, cy = (screenOrigin.Y - screen.Y + panel.Height / 2f) * Zoom;
            int sx = Math.Max(0, Math.Min(source.Width - panel.Width, (int)(cx - panel.Width / 2f)));
            int sy = Math.Max(0, Math.Min(source.Height - panel.Height, (int)(cy - panel.Height / 2f)));
            using (var g = Graphics.FromImage(surface.Bitmap))
            {
                g.CompositingMode = CompositingMode.SourceCopy;
                g.InterpolationMode = InterpolationMode.NearestNeighbor;
                g.DrawImage(source, panel, sx, sy, panel.Width, panel.Height, GraphicsUnit.Pixel);
            }
            surface.RoundCorners(panel, radius);
        }

        /// Everything about the slab that doesn't depend on where it is:
        /// shadow, tint, light, depth and rim. Drawn once, laid over the backdrop.
        public static void Overlay(Graphics g, RectangleF panel, float radius, float scale)
        {
            using var shape = Rounded(panel, radius);
            var outside = g.Save();
            using (var cut = new Region(shape))
                g.ExcludeClip(cut);
            for (int i = 0; i < 10; i++)
            {
                float grow = (10 - i) * 2.2f * scale;
                using var ring = Rounded(new RectangleF(panel.X - grow, panel.Y - grow + 6 * scale, panel.Width + grow * 2, panel.Height + grow * 2), radius + grow);
                using var shade = new SolidBrush(Color.FromArgb(4, 0, 0, 0));
                g.FillPath(shade, ring);
            }
            g.Restore(outside);

            using (var tint = new SolidBrush(Color.FromArgb(96, 16, 16, 26)))
                g.FillPath(tint, shape);
            // Light at -45 degrees: a broad sheen falling from the top-left.
            using (var sheen = new LinearGradientBrush(panel, Color.White, Color.White, 45f))
            {
                sheen.InterpolationColors = new ColorBlend
                {
                    Colors = new[] { Color.FromArgb(34, 255, 255, 255), Color.FromArgb(8, 255, 255, 255), Color.FromArgb(0, 255, 255, 255), Color.FromArgb(10, 255, 255, 255) },
                    Positions = new[] { 0f, 0.35f, 0.7f, 1f },
                };
                g.FillPath(sheen, shape);
            }
            // Depth: the glass edge is thicker and brighter than the middle.
            var inside = g.Save();
            g.SetClip(shape);
            float depth = 20 * scale;
            for (int i = 0; i < 8; i++)
            {
                float inset = depth * i / 8f;
                using var band = Rounded(RectangleF.Inflate(panel, -inset, -inset), Math.Max(1, radius - inset));
                using var pen = new Pen(Color.FromArgb((int)(9 * (1 - i / 8f)), 255, 255, 255), depth / 8f + 0.5f);
                g.DrawPath(pen, band);
            }
            g.Restore(inside);

            // Rim: bright where the light hits (top-left, and its reflection at
            // the bottom-right), with a faint dispersion fringe either side.
            DrawRim(g, panel, radius, -0.8f * scale, Color.FromArgb(40, 255, 120, 150), Color.FromArgb(24, 120, 160, 255));
            DrawRim(g, panel, radius, 0.6f * scale, Color.FromArgb(34, 120, 200, 255), Color.FromArgb(22, 255, 140, 170));
            DrawRim(g, panel, radius, 0, Color.FromArgb(130, 255, 255, 255), Color.FromArgb(70, 255, 255, 255));
        }

        static void DrawRim(Graphics g, RectangleF panel, float radius, float offset, Color lit, Color reflected)
        {
            var r = RectangleF.Inflate(panel, -0.75f + offset, -0.75f + offset);
            using var path = Rounded(r, radius + offset);
            using var brush = new LinearGradientBrush(r, lit, reflected, 45f);
            brush.InterpolationColors = new ColorBlend
            {
                Colors = new[] { lit, Color.FromArgb(lit.A / 6, lit), Color.FromArgb(0, 255, 255, 255), Color.FromArgb(reflected.A / 4, reflected), reflected },
                Positions = new[] { 0f, 0.3f, 0.5f, 0.75f, 1f },
            };
            using var pen = new Pen(brush, 1.3f);
            g.DrawPath(pen, path);
        }

        // ---- Per-pixel-alpha window ------------------------------------------

        [StructLayout(LayoutKind.Sequential)]
        struct BlendFunction { public byte Op, Flags, Alpha, Format; }

        [StructLayout(LayoutKind.Sequential)]
        struct BitmapInfoHeader
        {
            public int Size, Width, Height;
            public short Planes, BitCount;
            public int Compression, SizeImage, XPelsPerMeter, YPelsPerMeter, ClrUsed, ClrImportant;
        }

        [DllImport("user32.dll", SetLastError = true)]
        static extern bool UpdateLayeredWindow(IntPtr window, IntPtr screenDc, ref Point position, ref Size size, IntPtr sourceDc, ref Point sourceOrigin, int key, ref BlendFunction blend, int flags);
        [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr window);
        [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr window, IntPtr dc);
        [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
        [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
        [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr obj);
        [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr obj);
        [DllImport("gdi32.dll")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BitmapInfoHeader info, int usage, out IntPtr bits, IntPtr section, int offset);

        /// The window's pixels: a premultiplied DIB that GDI+ draws into and
        /// UpdateLayeredWindow reads directly (no copy per frame).
        public sealed class Surface : IDisposable
        {
            readonly IntPtr dc, dib, old;
            public readonly Bitmap Bitmap;
            readonly IntPtr bits;
            byte[] corner;
            int cornerRadius;
            public Size Size => Bitmap.Size;

            public Surface(Size size)
            {
                var header = new BitmapInfoHeader
                {
                    Size = Marshal.SizeOf(typeof(BitmapInfoHeader)),
                    Width = size.Width,
                    Height = -size.Height,
                    Planes = 1,
                    BitCount = 32,
                };
                IntPtr screen = GetDC(IntPtr.Zero);
                dc = CreateCompatibleDC(screen);
                ReleaseDC(IntPtr.Zero, screen);
                dib = CreateDIBSection(dc, ref header, 0, out bits, IntPtr.Zero, 0);
                old = SelectObject(dc, dib);
                Bitmap = new Bitmap(size.Width, size.Height, size.Width * 4, PixelFormat.Format32bppPArgb, bits);
            }

            /// Fades the pixels outside the rounded corners of `panel` (anti-aliased).
            public void RoundCorners(Rectangle panel, int radius)
            {
                if (corner == null || cornerRadius != radius)
                {
                    cornerRadius = radius;
                    corner = new byte[radius * radius];
                    for (int y = 0; y < radius; y++)
                        for (int x = 0; x < radius; x++)
                        {
                            double dx = radius - (x + 0.5), dy = radius - (y + 0.5);
                            double cover = radius - Math.Sqrt(dx * dx + dy * dy) + 0.5;
                            corner[y * radius + x] = (byte)(255 * Math.Max(0, Math.Min(1, cover)));
                        }
                }
                int stride = Size.Width * 4;
                var row = new byte[radius * 4];
                for (int y = 0; y < radius; y++)
                    for (int side = 0; side < 4; side++)
                    {
                        bool right = (side & 1) != 0, bottom = (side & 2) != 0;
                        int py = bottom ? panel.Bottom - 1 - y : panel.Y + y;
                        int px = right ? panel.Right - radius : panel.X;
                        IntPtr at = bits + py * stride + px * 4;
                        Marshal.Copy(at, row, 0, row.Length);
                        for (int i = 0; i < radius; i++)
                        {
                            int a = corner[y * radius + (right ? radius - 1 - i : i)];
                            if (a == 255)
                                continue;
                            for (int c = 0; c < 4; c++)
                                row[i * 4 + c] = (byte)(row[i * 4 + c] * a / 255);
                        }
                        Marshal.Copy(row, 0, at, row.Length);
                    }
            }

            /// Shows the surface; with `contents` false only the window moves.
            public void Present(IntPtr window, Point position, bool contents = true)
            {
                IntPtr screen = GetDC(IntPtr.Zero);
                var size = Size;
                var origin = Point.Empty;
                var blend = new BlendFunction { Op = 0, Flags = 0, Alpha = 255, Format = 1 };
                UpdateLayeredWindow(window, screen, ref position, ref size, contents ? dc : IntPtr.Zero, ref origin, 0, ref blend, 2);
                ReleaseDC(IntPtr.Zero, screen);
            }

            public void Dispose()
            {
                Bitmap.Dispose();
                SelectObject(dc, old);
                DeleteObject(dib);
                DeleteDC(dc);
            }
        }
    }
}
