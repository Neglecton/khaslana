package com.example.login;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.SQLException;

public final class LoginAttemptRepository {
    private static final String INSERT_ATTEMPT =
            "INSERT INTO login_attempt(username, succeeded, reason) VALUES (?, ?, ?)";

    private final Connection connection;

    public LoginAttemptRepository(Connection connection) {
        this.connection = connection;
    }

    public void record(String username, boolean succeeded, String reason) {
        try (PreparedStatement statement = connection.prepareStatement(INSERT_ATTEMPT)) {
            statement.setString(1, username);
            statement.setBoolean(2, succeeded);
            statement.setString(3, reason);
            statement.executeUpdate();
        } catch (SQLException error) {
            throw new IllegalStateException("记录登录尝试失败", error);
        }
    }
}
