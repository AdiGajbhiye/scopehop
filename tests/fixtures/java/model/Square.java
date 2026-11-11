package com.example.model;

public class Square extends Shape {
    public final double side;

    public Square(double side) {
        super("square");
        this.side = side;
    }

    @Override
    public double area() {
        return side * side;
    }
}
