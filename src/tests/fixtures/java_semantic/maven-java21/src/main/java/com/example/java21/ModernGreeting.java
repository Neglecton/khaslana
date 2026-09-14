package com.example.java21;

public record ModernGreeting(String prefix) {
    public String message(String name) {
        return prefix + name;
    }
}
