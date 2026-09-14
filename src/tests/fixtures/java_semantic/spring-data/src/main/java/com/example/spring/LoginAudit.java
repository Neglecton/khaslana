package com.example.spring;

import jakarta.persistence.Entity;
import jakarta.persistence.Id;

@Entity
public class LoginAudit {
    @Id public long id;
    public String username;

    public LoginAudit(String username) {
        this.username = username;
    }
}
