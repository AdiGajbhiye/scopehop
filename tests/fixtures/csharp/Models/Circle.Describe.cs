namespace Shop.Models;

public partial class Circle
{
    public string Describe() => $"{Name} {Radius} at {Center.X}";

    public class Builder
    {
        public double Size { get; set; }

        public Circle Build() => new Circle(Size);
    }
}
