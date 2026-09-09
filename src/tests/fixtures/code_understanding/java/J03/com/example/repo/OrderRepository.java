package com.example.repo;

public class OrderRepository implements Repository {
    @Override
    public String findById(int id) {
        return "order-" + id;
    }
}
