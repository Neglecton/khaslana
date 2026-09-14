package com.example.spring;

import org.springframework.stereotype.Service;

@Service
public final class AuthService {
    private final UserMapper mapper;
    private final LoginAuditRepository auditRepository;

    public AuthService(UserMapper mapper, LoginAuditRepository auditRepository) {
        this.mapper = mapper;
        this.auditRepository = auditRepository;
    }

    public UserEntity login(String username) {
        UserEntity user = mapper.findByUsername(username);
        auditRepository.save(new LoginAudit(username));
        return user;
    }
}
