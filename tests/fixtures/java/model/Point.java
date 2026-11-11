package com.example.model;

public record Point(int x, int y) {
    public int sum() {
        return x + y;
    }
}
