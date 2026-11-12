using System;
using Shop.Models;
using Shop.Util;
using static Shop.Util.Mathx;
using Other = Shop.Other;
using Disk = Shop.Models.Circle;

namespace Shop.App
{
    public class Program
    {
        private int count = 10;

        public static void Main(string[] args)
        {
            var c = new Circle(2);
            Console.WriteLine(c.Area() + c.Radius + c.Describe().Length);
            var grown = c.Grow(1);
            Console.WriteLine(grown.Radius);
            var p = new Point(1, 2);
            Console.WriteLine(p.Moved(0.5).X);
            Console.WriteLine(Circle.Unit.Area() + Circle.Pi);
            Console.WriteLine(c.Center.Moved(1).Y);
            Circle made = Circle.Of(3);
            Console.WriteLine(made.Grow(by: 1));
            var named = new Circle(radius: 2, center: p);
            var built = new Circle.Builder { Size = 4 }.Build();
            Console.WriteLine(built.Radius + named.Radius);
            Console.WriteLine("hi".Shout() + c.Doubled() + StringExt.CountWords("a b"));
            Console.WriteLine(Square(2));
            var d = new Other.Circle { Radius = 1 };
            Console.WriteLine(d.Area());
            Disk disk = new(5);
            Console.WriteLine(disk.Radius);
            Console.WriteLine(Color.Red.ToString());
            var box = new Box<int>();
            box.Put(1);
            Box plain = new Box();
            Console.WriteLine(plain.Size + StringExt.Pad("a").Length + StringExt.Pad("a", 2).Length);
            Console.WriteLine(StringExt.Pad("a", 1, 2));
            var group = c.Doubled;
            Measure m = shape => shape.Area();
            Console.WriteLine(m(c));
        }

        public int Scopes(object o, IShape[] items)
        {
            int total = 0;
            for (int i = 0; i < items.Length; i++)
            {
                total += i + (int)items[i].Area();
            }
            if (o is Circle circle && circle.Radius > 0)
            {
                total += (int)circle.Area();
            }
            if (!int.TryParse("3", out var parsed))
            {
                return parsed;
            }
            switch (o)
            {
                case Point pt:
                    total += (int)pt.X;
                    break;
            }
            var kind = o switch
            {
                Circle k => k.Radius,
                _ => 0,
            };
            try
            {
                total += count;
            }
            catch (InvalidOperationException e)
            {
                Console.WriteLine(e.Message);
            }
            using (var reader = new System.IO.StringReader("x"))
            {
                reader.Read();
            }
            Func<int, int> twice = x => x * 2;
            int Local(int v) => v + total;
            total = twice(Local(total));
            {
                int count = 1;
                total += count;
            }
            return total;
        }

        public T First<T>(T[] list) where T : IShape
        {
            T item = list[0];
            return item;
        }
    }
}
