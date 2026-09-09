package com.example.overload;

public class Overloads {
    public String f(int amount) { return "int:" + amount; }

    public String f(String name) { return "string:" + name; }

    public String f(long base, int... extras) { return "varargs:" + base; }

    public String g(Integer boxed) { return "boxed"; }

    public String g(long raw) { return "raw"; }
}
