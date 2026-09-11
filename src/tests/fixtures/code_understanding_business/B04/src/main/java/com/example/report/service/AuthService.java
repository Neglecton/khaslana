package com.example.report.service;

import com.example.report.entity.UserAccount;
import com.example.report.mapper.UserMapper;
import com.example.report.mapper.LoginLogMapper;
import com.example.report.security.PasswordVerifier;
import org.springframework.stereotype.Service;

@Service
public class AuthService {

    private final UserMapper userMapper;
    private final LoginLogMapper loginLogMapper;
    private final PasswordVerifier passwordVerifier;

    public AuthService(UserMapper userMapper, LoginLogMapper loginLogMapper, PasswordVerifier passwordVerifier) {
        this.userMapper = userMapper;
        this.loginLogMapper = loginLogMapper;
        this.passwordVerifier = passwordVerifier;
    }

    public String authenticate(String username, String rawPassword) {
        UserAccount user = userMapper.findByUsername(username);
        if (user == null) {
            loginLogMapper.insertLog(username, "USER_NOT_FOUND");
            throw new IllegalArgumentException("用户名或密码错误");
        }
        if (!passwordVerifier.matches(rawPassword, user.passwordHash())) {
            loginLogMapper.insertLog(username, "BAD_PASSWORD");
            throw new IllegalArgumentException("用户名或密码错误");
        }
        loginLogMapper.insertLog(username, "SUCCESS");
        return "session-" + user.id();
    }
}
