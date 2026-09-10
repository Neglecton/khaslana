package com.example.login;

public final class LoginRejectedException extends RuntimeException {
    public LoginRejectedException(String message) {
        super(message);
    }
}
