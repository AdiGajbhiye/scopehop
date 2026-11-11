package com.example.model;

public abstract class Shape implements Named {
    protected final String name;

    protected Shape(String name) {
        this.name = name;
    }

    public String getName() {
        return name;
    }

    public double area() {
        return 0.0;
    }

    public int length() {
        return name.length();
    }

    @Override
    public String label() {
        return name;
    }
}
