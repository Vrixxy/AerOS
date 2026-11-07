using System;
using System.IO;
using System.IO.Compression;
using System.Text;

namespace AerOSSetup
{
    /// The AerOS files ride at the end of the .exe as a zip archive, followed
    /// by a 16-byte trailer: "AEROSPKG" and the archive's start offset.
    static class Payload
    {
        static readonly byte[] Magic = Encoding.ASCII.GetBytes("AEROSPKG");

        public static string ExePath => System.Reflection.Assembly.GetExecutingAssembly().Location;

        /// Start of the archive, or -1 when this copy carries none (the
        /// installed AerOS.exe).
        public static long Find(out long archiveLength)
        {
            archiveLength = 0;
            using var file = File.OpenRead(ExePath);
            if (file.Length < 16)
                return -1;
            file.Seek(-16, SeekOrigin.End);
            var trailer = new byte[16];
            file.Read(trailer, 0, 16);
            for (int i = 0; i < 8; i++)
                if (trailer[i] != Magic[i])
                    return -1;
            long start = BitConverter.ToInt64(trailer, 8);
            if (start <= 0 || start >= file.Length - 16)
                return -1;
            archiveLength = file.Length - 16 - start;
            return start;
        }

        public static bool Present => Find(out _) > 0;

        /// Bytes of the program itself, without the archive.
        public static long ProgramLength()
        {
            long start = Find(out _);
            return start > 0 ? start : new FileInfo(ExePath).Length;
        }

        public static long UnpackedBytes()
        {
            long total = 0;
            using var archive = Open(out var stream);
            foreach (var entry in archive.Entries)
                total += entry.Length;
            stream.Dispose();
            return total;
        }

        public static ZipArchive Open(out Stream stream)
        {
            long start = Find(out long length);
            if (start < 0)
                throw new InvalidOperationException("This copy of AerOS Setup has no AerOS files inside it.");
            stream = new Window(File.OpenRead(ExePath), start, length);
            return new ZipArchive(stream, ZipArchiveMode.Read);
        }

        /// Writes every file to `root`, reporting progress in bytes.
        public static void Extract(string root, Action<long, long, string> progress)
        {
            using var archive = Open(out var stream);
            long total = 0, done = 0;
            foreach (var entry in archive.Entries)
                total += entry.Length;
            var buffer = new byte[1 << 20];
            foreach (var entry in archive.Entries)
            {
                if (entry.FullName.EndsWith("/"))
                    continue;
                string target = Path.Combine(root, entry.FullName.Replace('/', '\\'));
                Directory.CreateDirectory(Path.GetDirectoryName(target));
                using (var input = entry.Open())
                using (var output = new FileStream(target, FileMode.Create, FileAccess.Write, FileShare.None, 1 << 20, FileOptions.WriteThrough))
                {
                    int read;
                    while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
                    {
                        output.Write(buffer, 0, read);
                        done += read;
                        progress(done, total, entry.FullName);
                    }
                }
            }
            stream.Dispose();
        }

        /// A read-only view of part of another stream.
        sealed class Window : Stream
        {
            readonly Stream inner;
            readonly long start, length;
            long position;

            public Window(Stream inner, long start, long length)
            {
                this.inner = inner;
                this.start = start;
                this.length = length;
            }

            public override bool CanRead => true;
            public override bool CanSeek => true;
            public override bool CanWrite => false;
            public override long Length => length;

            public override long Position
            {
                get => position;
                set => position = value;
            }

            public override int Read(byte[] buffer, int offset, int count)
            {
                long left = length - position;
                if (left <= 0)
                    return 0;
                if (count > left)
                    count = (int)left;
                inner.Seek(start + position, SeekOrigin.Begin);
                int read = inner.Read(buffer, offset, count);
                position += read;
                return read;
            }

            public override long Seek(long offset, SeekOrigin origin)
            {
                position = origin switch
                {
                    SeekOrigin.Begin => offset,
                    SeekOrigin.Current => position + offset,
                    _ => length + offset,
                };
                return position;
            }

            public override void Flush() { }
            public override void SetLength(long value) => throw new NotSupportedException();
            public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();

            protected override void Dispose(bool disposing)
            {
                if (disposing)
                    inner.Dispose();
                base.Dispose(disposing);
            }
        }
    }
}
