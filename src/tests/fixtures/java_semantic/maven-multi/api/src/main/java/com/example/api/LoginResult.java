package com.example.api;

public final class LoginResult {
    private final String subject;

    public LoginResult(String subject) {
        this.subject = subject;
    }

    public String subject() {
        return subject;
    }
}
