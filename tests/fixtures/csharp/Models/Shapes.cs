using System;

namespace Shop.Models
{
    public interface IShape
    {
        double Area();
        string Name => "shape";
    }

    public record Point(double X, double Y)
    {
        public Point Moved(double by) => new Point(X + by, Y + by);
    }

    public partial class Circle : IShape
    {
        public const double Pi = 3.14;
        public static readonly Circle Unit = new Circle(1);
        private double radius;

        public Circle(double radius, Point? center = null)
        {
            this.radius = radius;
            Center = center ?? new Point(0, 0);
        }

        public double Radius
        {
            get => radius;
            set => radius = value;
        }

        public Point Center { get; init; }

        public double Area() => Pi * radius * radius;

        public Circle Grow(double by)
        {
            return new Circle(Radius + by);
        }

        public static Circle Of(double r) => new(r);

        public string Name => "circle";
    }

    public enum Color
    {
        Red,
        Green,
    }

    public struct Box<T>
    {
        public T Value;

        public void Put(T item)
        {
            Value = item;
        }
    }

    public class Box
    {
        public int Size;
    }

    public interface IRepository<T>
    {
        T Find(System.Func<T, bool> match);
    }

    public delegate double Measure(IShape shape);
}
