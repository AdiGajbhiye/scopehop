using Shop.Models;

namespace Shop.Util
{
    public static class StringExt
    {
        public static string Shout(this string s) => s.ToUpper() + "!";

        public static double Doubled(this Circle c) => c.Area() * 2;

        public static int CountWords(string s) => s.Split(' ').Length;

        public static string Pad(string s) => s + " ";

        public static string Pad(string s, int n) => s.PadRight(n);

        public static string Pad(string s, params object[] rest) => s;
    }

    public static class Mathx
    {
        public static double Square(double x) => x * x;
    }
}
