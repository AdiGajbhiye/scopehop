namespace Shop.Other
{
    public class Circle
#if FANCY
        : System.IComparable
#endif
    {
        public double Radius { get; set; }

        public double Area() => Radius;
    }
}
