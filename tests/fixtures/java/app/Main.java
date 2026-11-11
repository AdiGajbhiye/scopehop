package com.example.app;

import com.example.model.*;
import com.example.model.Outer.Inner;
import java.util.List;
import static com.example.util.Strings.shout;
import static com.example.util.Strings.VERSION;

public class Main {
    private int count = 0;
    private final Circle start;

    public Main(Circle start) {
        this.start = start;
    }

    public int run(List<Object> items) {
        var c = new Circle(2.0);
        Circle typed = Circle.unit();
        double r = c.radius;
        double a = start.area();
        String n = c.getName();
        String l = c.label();
        String d = Named.DEFAULT;
        Circle made = Other.make(3.0);
        Outer.Inner nested = new Outer.Inner();
        Inner inner = new Inner();
        nested.ping();
        inner.ping();
        Color.RED.shout();
        shout(VERSION);
        Other.helper();
        this.count += 1;
        Point p = new Point(1, 2);
        int px = p.x() + p.sum();
        items.forEach(item -> System.out.println(item));
        java.util.function.IntUnaryOperator inc = x -> x + count;
        java.util.function.IntBinaryOperator add = (u, v) -> u + v;
        for (int i = 0; i < 3; i++) {
            count += i;
        }
        for (Object o : items) {
            if (o instanceof Circle circle && circle.radius > 0) {
                circle.area();
            }
        }
        try (var reader = new java.io.StringReader("x")) {
            reader.read();
        } catch (java.io.IOException e) {
            e.getMessage();
        }
        Object any = made;
        double cast = ((Circle) any).radius;
        Runnable task = new Runnable() {
            int ticks = 0;

            public void run() {
                ticks++;
            }
        };
        java.util.function.IntSupplier ref = this::shadow;
        // An array of Circles is not a Circle: no Shape.length here.
        Circle[] all = {c, typed};
        int howMany = all.length;
        // A method of this class, called without a receiver.
        double picked = pick().area();
        return count + px + typed.hashCode();
    }

    Circle pick() {
        return start;
    }

    int shadow() {
        int count = 10;
        return count;
    }

    static <T> T first(List<T> list) {
        T item = list.get(0);
        return item;
    }

    static class Shout extends Shape {
        Shout() {
            super("shout");
        }

        @Override
        public double area() {
            return super.area() + count();
        }

        // Shadows Main.count inside Shout.
        double count() {
            return getName().length();
        }
    }
}
