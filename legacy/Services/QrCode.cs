using System;
using System.Text;

namespace DynamicIsland.Services
{
    /// <summary>
    /// Small QR encoder: byte mode, error correction level L, versions 1-5
    /// (up to 106 bytes, single Reed-Solomon block). Enough for a local share URL.
    /// </summary>
    public static class QrCode
    {
        // Index = version. Total codewords and EC codewords for level L.
        private static readonly int[] TotalCodewords = { 0, 26, 44, 70, 100, 134 };
        private static readonly int[] EccCodewords = { 0, 7, 10, 15, 20, 26 };
        private const int MaxVersion = 5;
        private const int Mask = 0; // (x + y) % 2 == 0

        /// <summary>Returns the module matrix [y, x] (true = dark), or null if the text is too long.</summary>
        public static bool[,] Encode(string text)
        {
            byte[] payload = Encoding.UTF8.GetBytes(text ?? "");

            int version = 0;
            for (int v = 1; v <= MaxVersion; v++)
            {
                if (payload.Length <= TotalCodewords[v] - EccCodewords[v] - 2)
                {
                    version = v;
                    break;
                }
            }
            if (version == 0) return null;

            int dataLen = TotalCodewords[version] - EccCodewords[version];
            byte[] data = BuildDataCodewords(payload, dataLen);
            byte[] ecc = ReedSolomonRemainder(data, ReedSolomonDivisor(EccCodewords[version]));

            var all = new byte[data.Length + ecc.Length];
            Buffer.BlockCopy(data, 0, all, 0, data.Length);
            Buffer.BlockCopy(ecc, 0, all, data.Length, ecc.Length);

            int size = version * 4 + 17;
            var modules = new bool[size, size];
            var isFunction = new bool[size, size];

            DrawFunctionPatterns(modules, isFunction, size, version);
            DrawCodewords(modules, isFunction, size, all);

            for (int y = 0; y < size; y++)
            {
                for (int x = 0; x < size; x++)
                {
                    if (!isFunction[y, x] && (x + y) % 2 == 0) modules[y, x] = !modules[y, x];
                }
            }

            DrawFormatBits(modules, isFunction, size);
            return modules;
        }

        private static byte[] BuildDataCodewords(byte[] payload, int dataLen)
        {
            var result = new byte[dataLen];
            int bitPos = 0;

            AppendBits(result, ref bitPos, 0x4, 4);            // byte mode
            AppendBits(result, ref bitPos, payload.Length, 8); // character count (versions 1-9)
            foreach (byte b in payload) AppendBits(result, ref bitPos, b, 8);

            int capacityBits = dataLen * 8;
            AppendBits(result, ref bitPos, 0, Math.Min(4, capacityBits - bitPos)); // terminator
            AppendBits(result, ref bitPos, 0, (8 - bitPos % 8) % 8);               // byte align

            for (int pad = 0xEC; bitPos < capacityBits; pad ^= 0xEC ^ 0x11)
            {
                AppendBits(result, ref bitPos, pad, 8);
            }
            return result;
        }

        private static void AppendBits(byte[] target, ref int bitPos, int value, int count)
        {
            for (int i = count - 1; i >= 0; i--)
            {
                if (((value >> i) & 1) != 0) target[bitPos >> 3] |= (byte)(0x80 >> (bitPos & 7));
                bitPos++;
            }
        }

        private static void DrawFunctionPatterns(bool[,] modules, bool[,] isFunction, int size, int version)
        {
            // Timing patterns
            for (int i = 0; i < size; i++)
            {
                SetFunction(modules, isFunction, 6, i, i % 2 == 0);
                SetFunction(modules, isFunction, i, 6, i % 2 == 0);
            }

            // Finder patterns (with separators)
            DrawFinder(modules, isFunction, size, 3, 3);
            DrawFinder(modules, isFunction, size, size - 4, 3);
            DrawFinder(modules, isFunction, size, 3, size - 4);

            // Versions 2-6 have exactly one alignment pattern, bottom right.
            if (version >= 2)
            {
                int c = size - 7;
                for (int dy = -2; dy <= 2; dy++)
                {
                    for (int dx = -2; dx <= 2; dx++)
                    {
                        SetFunction(modules, isFunction, c + dx, c + dy, Math.Max(Math.Abs(dx), Math.Abs(dy)) != 1);
                    }
                }
            }

            // Reserve the format areas; real bits are written after masking.
            DrawFormatBits(modules, isFunction, size);
        }

        private static void DrawFinder(bool[,] modules, bool[,] isFunction, int size, int cx, int cy)
        {
            for (int dy = -4; dy <= 4; dy++)
            {
                for (int dx = -4; dx <= 4; dx++)
                {
                    int x = cx + dx, y = cy + dy;
                    if (x < 0 || x >= size || y < 0 || y >= size) continue;
                    int dist = Math.Max(Math.Abs(dx), Math.Abs(dy));
                    SetFunction(modules, isFunction, x, y, dist != 2 && dist != 4);
                }
            }
        }

        private static void DrawFormatBits(bool[,] modules, bool[,] isFunction, int size)
        {
            int data = (1 << 3) | Mask; // level L = 01
            int rem = data;
            for (int i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >> 9) * 0x537);
            int bits = ((data << 10) | rem) ^ 0x5412;

            for (int i = 0; i <= 5; i++) SetFunction(modules, isFunction, 8, i, Bit(bits, i));
            SetFunction(modules, isFunction, 8, 7, Bit(bits, 6));
            SetFunction(modules, isFunction, 8, 8, Bit(bits, 7));
            SetFunction(modules, isFunction, 7, 8, Bit(bits, 8));
            for (int i = 9; i < 15; i++) SetFunction(modules, isFunction, 14 - i, 8, Bit(bits, i));

            for (int i = 0; i < 8; i++) SetFunction(modules, isFunction, size - 1 - i, 8, Bit(bits, i));
            for (int i = 8; i < 15; i++) SetFunction(modules, isFunction, 8, size - 15 + i, Bit(bits, i));
            SetFunction(modules, isFunction, 8, size - 8, true); // always dark
        }

        private static void DrawCodewords(bool[,] modules, bool[,] isFunction, int size, byte[] data)
        {
            int i = 0;
            for (int right = size - 1; right >= 1; right -= 2)
            {
                if (right == 6) right = 5;
                for (int vert = 0; vert < size; vert++)
                {
                    for (int j = 0; j < 2; j++)
                    {
                        int x = right - j;
                        bool upward = ((right + 1) & 2) == 0;
                        int y = upward ? size - 1 - vert : vert;
                        if (!isFunction[y, x] && i < data.Length * 8)
                        {
                            modules[y, x] = ((data[i >> 3] >> (7 - (i & 7))) & 1) != 0;
                            i++;
                        }
                    }
                }
            }
        }

        private static void SetFunction(bool[,] modules, bool[,] isFunction, int x, int y, bool dark)
        {
            modules[y, x] = dark;
            isFunction[y, x] = true;
        }

        private static bool Bit(int value, int index)
        {
            return ((value >> index) & 1) != 0;
        }

        private static byte[] ReedSolomonDivisor(int degree)
        {
            var result = new byte[degree];
            result[degree - 1] = 1;
            int root = 1;
            for (int i = 0; i < degree; i++)
            {
                for (int j = 0; j < degree; j++)
                {
                    result[j] = (byte)Multiply(result[j], root);
                    if (j + 1 < degree) result[j] ^= result[j + 1];
                }
                root = Multiply(root, 2);
            }
            return result;
        }

        private static byte[] ReedSolomonRemainder(byte[] data, byte[] divisor)
        {
            var result = new byte[divisor.Length];
            foreach (byte b in data)
            {
                int factor = b ^ result[0];
                Array.Copy(result, 1, result, 0, result.Length - 1);
                result[result.Length - 1] = 0;
                for (int i = 0; i < result.Length; i++)
                {
                    result[i] ^= (byte)Multiply(divisor[i], factor);
                }
            }
            return result;
        }

        private static int Multiply(int x, int y)
        {
            int z = 0;
            for (int i = 7; i >= 0; i--)
            {
                z = (z << 1) ^ ((z >> 7) * 0x11D);
                z ^= ((y >> i) & 1) * x;
            }
            return z & 0xFF;
        }
    }
}
