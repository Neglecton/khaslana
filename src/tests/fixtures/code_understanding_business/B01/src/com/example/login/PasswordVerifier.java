package com.example.login;

public final class PasswordVerifier {
    public boolean matches(String password, String passwordHash) {
        return PasswordHashLibrary.verify(password, passwordHash);
    }
}
