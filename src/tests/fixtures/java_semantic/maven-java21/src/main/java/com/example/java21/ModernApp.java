package com.example.java21;

public final class ModernApp {
    public static String run(String[] args) {
        ModernGreeting greeting = new ModernGreeting("Hello ");
        return greeting.message(args[0]);
    }
}
