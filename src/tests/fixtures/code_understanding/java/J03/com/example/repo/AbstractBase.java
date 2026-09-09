package com.example.repo;

public abstract class AbstractBase {
    public abstract void saveAll();

    public final void lock() { }

    private void audit() { }
}
