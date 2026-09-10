package com.example.shop.service;

import com.example.shop.entity.UserAccount;
import com.example.shop.exception.AuthFailedException;
import com.example.shop.mapper.LoginLogMapper;
import com.example.shop.mapper.UserMapper;
import com.example.shop.session.SessionService;
import org.springframework.security.crypto.password.PasswordEncoder;
import org.springframework.stereotype.Service;

@Service
public class AuthService {
    private final UserMapper userMapper;
    private final LoginLogMapper loginLogMapper;
    private final PasswordEncoder passwordEncoder;
    private final SessionService sessionService;

    public AuthService(
            UserMapper userMapper,
            LoginLogMapper loginLogMapper,
            PasswordEncoder passwordEncoder,
            SessionService sessionService) {
        this.userMapper = userMapper;
        this.loginLogMapper = loginLogMapper;
        this.passwordEncoder = passwordEncoder;
        this.sessionService = sessionService;
    }

    public String login(String username, String rawPassword) {
        UserAccount user = userMapper.findByUsername(username);
        if (user == null) {
            loginLogMapper.insertLog(username, "USER_NOT_FOUND");
            throw new AuthFailedException("用户名或密码错误");
        }
        if (user.status() != 1) {
            loginLogMapper.insertLog(username, "DISABLED");
            throw new AuthFailedException("账号已停用");
        }
        if (!passwordEncoder.matches(rawPassword, user.passwordHash())) {
            userMapper.incrementFailedAttempts(user.id());
            loginLogMapper.insertLog(username, "BAD_PASSWORD");
            throw new AuthFailedException("用户名或密码错误");
        }
        userMapper.markLoginSuccess(user.id());
        loginLogMapper.insertLog(username, "SUCCESS");
        return sessionService.createSession(user.id());
    }
}
