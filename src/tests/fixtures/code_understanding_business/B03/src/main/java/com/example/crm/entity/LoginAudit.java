package com.example.crm.entity;

import jakarta.persistence.Entity;
import jakarta.persistence.GeneratedValue;
import jakarta.persistence.GenerationType;
import jakarta.persistence.Id;
import java.time.Instant;

@Entity
public class LoginAudit {
    @Id
    @GeneratedValue(strategy = GenerationType.IDENTITY)
    private Long id;

    private String username;

    private String reason;

    private Instant createdAt;

    public LoginAudit(String username, String reason) {
        this.username = username;
        this.reason = reason;
        this.createdAt = Instant.now();
    }
}
