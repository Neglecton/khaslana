package com.example.gradle;

public final class GradleApp {
    public static String run(String[] args) {
        GradleGreeter greeter = new GradleGreeter();
        return greeter.greet(args[0]);
    }
}
