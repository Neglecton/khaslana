package com.example.repo;

public class UserRepository extends AbstractBase implements Repository {
    @Override
    public String findById(int id) {
        return "user-" + id;
    }

    @Override
    public void saveAll() { }
}
