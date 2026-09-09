package com.example.scope;

import java.util.ArrayList;

public class Shadowing {
    private String name = "field";

    public String rename(String name) {
        String upper = name.trim();
        var list = new ArrayList<String>();
        list.add(upper);
        return list.get(0);
    }

    public String fieldAccess() {
        Shadowing self = new Shadowing();
        return self.name;
    }

    static class Item { }
}
