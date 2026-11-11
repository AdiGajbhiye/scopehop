package com.example.app;

import com.example.model.Circle;

// Same package as Main.java, so visible there without an import.
public class Other {
    public static int helper() {
        return 1;
    }

    public static Circle make(double radius) {
        return new Circle(radius);
    }
}
