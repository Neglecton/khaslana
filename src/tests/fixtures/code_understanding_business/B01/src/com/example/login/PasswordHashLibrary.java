package com.example.login;

public final class PasswordHashLibrary {
    private PasswordHashLibrary() {}

    public static boolean verify(String password, String passwordHash) {
        return passwordHash.equals("hash:" + password);
    }
}
