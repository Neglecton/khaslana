package com.example.inherit;

public class Child extends Base {
    public Child(int seed) {
        super(seed);
    }

    @Override
    public void greet() {
        super.greet();
        this.log("hello");
    }
}
