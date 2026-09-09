package com.example.overload;

public class Caller {
    public String callAll() {
        Overloads o = new Overloads();
        String a = o.f(1);
        String b = o.f("x");
        String c = o.f(2L, 3, 4);
        String d = o.f(null);
        String e = o.g(5);
        return a + b + c + d + e;
    }
}
