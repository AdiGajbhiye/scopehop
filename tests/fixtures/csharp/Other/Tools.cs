namespace Shop.Other.Tools
{
    public class Measurer
    {
        // Shop.Other.Circle (a parent namespace); the Shop project's
        // `global using Shop.Models;` does not reach this project.
        public double Of(Circle c) => c.Area();

        public object Unknown(Point p) => p;
    }
}
