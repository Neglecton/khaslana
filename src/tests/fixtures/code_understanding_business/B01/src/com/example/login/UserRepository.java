package com.example.login;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;

public final class UserRepository {
    private static final String FIND_ACTIVE =
            "SELECT id, password_hash FROM app_user WHERE username = ? AND enabled = TRUE";
    private static final String INCREMENT_FAILURE =
            "UPDATE app_user SET failed_attempts = failed_attempts + 1 WHERE id = ?";
    private static final String MARK_SUCCESS =
            "UPDATE app_user SET last_login_at = CURRENT_TIMESTAMP, failed_attempts = 0 WHERE id = ?";

    private final Connection connection;

    public UserRepository(Connection connection) {
        this.connection = connection;
    }

    public UserAccount findActiveByUsername(String username) {
        try (PreparedStatement statement = connection.prepareStatement(FIND_ACTIVE)) {
            statement.setString(1, username);
            try (ResultSet rows = statement.executeQuery()) {
                return rows.next() ? new UserAccount(rows.getLong(1), rows.getString(2)) : null;
            }
        } catch (SQLException error) {
            throw new IllegalStateException("读取用户失败", error);
        }
    }

    public void incrementFailedAttempts(long userId) {
        executeUpdate(INCREMENT_FAILURE, userId);
    }

    public void markLoginSuccess(long userId) {
        executeUpdate(MARK_SUCCESS, userId);
    }

    private void executeUpdate(String sql, long userId) {
        try (PreparedStatement statement = connection.prepareStatement(sql)) {
            statement.setLong(1, userId);
            statement.executeUpdate();
        } catch (SQLException error) {
            throw new IllegalStateException("更新用户失败", error);
        }
    }
}
