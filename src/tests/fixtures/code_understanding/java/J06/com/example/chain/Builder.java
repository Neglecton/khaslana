package com.example.chain;

public class Builder {
    private String name;
    private int count;

    public Builder setName(String name) {
        this.name = name;
        return this;
    }

    public Builder setCount(int count) {
        this.count = count;
        return this;
    }

    public String build() {
        return name + ":" + count;
    }
}
