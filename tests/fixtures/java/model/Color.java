package com.example.model;

public enum Color {
    RED("#f00"),
    GREEN("#0f0");

    private final String hex;

    Color(String hex) {
        this.hex = hex;
    }

    public String shout() {
        return hex.toUpperCase();
    }
}
