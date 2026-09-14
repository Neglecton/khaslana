package com.example.core;

import com.example.api.AuthProvider;
import com.example.api.LoginResult;

public final class LoginApplicationService {
    private final AuthProvider provider;

    public LoginApplicationService(AuthProvider provider) {
        this.provider = provider;
    }

    public LoginResult login(String username) { // JLS: overload target
        return provider.login(username, new char[0]); // JLS: outgoing interface call
    }

    public LoginResult login(String username, char[] secret) {
        return provider.login(username, secret);
    }
}
