package com.example.java8;

public final class LegacyApp {
    public static String run(String[] args) {
        LegacyGreeting greeting = new LegacyGreeting();
        return greeting.message(args[0]);
    }
}
