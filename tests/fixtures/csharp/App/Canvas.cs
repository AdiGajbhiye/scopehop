using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;

namespace Shop.App;

public class Canvas
{
    private readonly List<IShape> shapes = new();
    private Circle[] circles = new Circle[0];
    private Dictionary<string, Circle> byName = new();

    public int Count { get; private set; }

    public void Add(IShape shape)
    {
        shapes.Add(shape);
        Count++;
    }

    public double Total()
    {
        double sum = 0;
        foreach (var s in shapes)
        {
            sum += s.Area();
        }
        foreach (Circle c in circles)
        {
            sum += c.Radius;
        }
        return sum + circles[0].Radius + byName["a"].Radius;
    }

    public async Task<Circle> LoadAsync()
    {
        await Task.Delay(1);
        return Circle.Unit;
    }

    public async Task<double> UseAsync()
    {
        var loaded = await LoadAsync();
        return loaded.Radius + (await LoadAsync()).Area();
    }
}

public class Shapes : Canvas
{
    public double Twice() => Total() * 2 + Count;

    public double Queries(IRepository<Circle> repo, List<Circle> list, IServiceProvider services)
    {
        var big = list.Where(c => c.Radius > 1).First();
        var found = repo.Find(f => f.Radius > 2);
        var maybe = list.FirstOrDefault(m => m.Radius > 0)?.Grow(1);
        var canvas = services.GetRequiredService<Canvas>();
        return big.Area() + found.Radius + maybe.Radius + canvas.Total() + Square(3);
    }
}
