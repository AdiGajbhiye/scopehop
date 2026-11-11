package com.example.lib;

import com.example.app.Badge;

public class Greeter {
    public static final String HELLO = "hi";

    private final String name;

    public Greeter(String name) {
        this.name = name;
    }

    public String greet() {
        return HELLO + " " + name;
    }

    public static Greeter of(String name) {
        return new Greeter(name);
    }

    // Java using a Kotlin class.
    public String badged() {
        return new Badge(name).label();
    }
}
