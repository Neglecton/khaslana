package com.example.spring;

import jakarta.persistence.Entity;
import jakarta.persistence.Id;

@Entity
public class UserEntity {
    @Id public long id;
    public String username;
}
