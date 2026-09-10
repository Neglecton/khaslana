package com.example.crm.service;

import com.example.crm.entity.LoginAudit;
import com.example.crm.entity.UserEntity;
import com.example.crm.exception.LoginFailedException;
import com.example.crm.repository.LoginAuditRepository;
import com.example.crm.repository.UserRepository;
import com.example.crm.security.PasswordHasher;
import com.example.crm.security.TokenIssuer;
import java.time.Instant;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class AuthService {
    private final UserRepository users;
    private final LoginAuditRepository audits;
    private final PasswordHasher passwordHasher;
    private final TokenIssuer tokenIssuer;

    public AuthService(
            UserRepository users,
            LoginAuditRepository audits,
            PasswordHasher passwordHasher,
            TokenIssuer tokenIssuer) {
        this.users = users;
        this.audits = audits;
        this.passwordHasher = passwordHasher;
        this.tokenIssuer = tokenIssuer;
    }

    @Transactional
    public String login(String username, String rawPassword) {
        UserEntity user = users.findByUsernameAndEnabledTrue(username).orElse(null);
        if (user == null) {
            audits.save(new LoginAudit(username, "USER_NOT_FOUND"));
            throw new LoginFailedException("用户名或密码错误");
        }
        if (!passwordHasher.matches(rawPassword, user.getPasswordHash())) {
            user.setFailedAttempts(user.getFailedAttempts() + 1);
            users.save(user);
            audits.save(new LoginAudit(username, "BAD_PASSWORD"));
            throw new LoginFailedException("用户名或密码错误");
        }
        user.setLastLoginAt(Instant.now());
        user.setFailedAttempts(0);
        users.save(user);
        audits.save(new LoginAudit(username, "SUCCESS"));
        return tokenIssuer.issue(user.getId());
    }
}
