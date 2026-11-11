package com.example.model;

public class Circle extends Shape {
    public final double radius;

    public Circle(double radius) {
        super("circle");
        this.radius = radius;
    }

    @Override
    public double area() {
        return 3.14 * radius * radius;
    }

    public static Circle unit() {
        return new Circle(1.0);
    }
}
