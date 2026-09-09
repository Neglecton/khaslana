package com.example.app;

import com.example.repo.Repository;

public class Service {
    private final Repository repo;

    public Service(Repository repo) {
        this.repo = repo;
    }

    public String load(int id) {
        return repo.findById(id);
    }
}
