package com.example.login;

public final class AuthService {
    private final UserRepository users;
    private final LoginAttemptRepository attempts;
    private final PasswordVerifier passwordVerifier;
    private final SessionService sessions;

    public AuthService(
            UserRepository users,
            LoginAttemptRepository attempts,
            PasswordVerifier passwordVerifier,
            SessionService sessions) {
        this.users = users;
        this.attempts = attempts;
        this.passwordVerifier = passwordVerifier;
        this.sessions = sessions;
    }

    public String authenticate(String username, String password) {
        UserAccount user = users.findActiveByUsername(username);
        if (user == null) {
            attempts.record(username, false, "USER_NOT_FOUND");
            throw new LoginRejectedException("账号或密码错误");
        }
        if (!passwordVerifier.matches(password, user.passwordHash())) {
            users.incrementFailedAttempts(user.id());
            attempts.record(username, false, "BAD_PASSWORD");
            throw new LoginRejectedException("账号或密码错误");
        }

        String sessionId = sessions.create(user.id());
        users.markLoginSuccess(user.id());
        attempts.record(username, true, "SUCCESS");
        return sessionId;
    }
}
