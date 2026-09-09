package com.example.alpha;

import com.example.beta.Util;

public class Runner {
    public String run(String raw) {
        String a = Util.format(raw);
        Util.print(a);
        return innerLabel();
    }

    public class Inner {
        public String label() {
            return "inner";
        }
    }

    private String innerLabel() {
        return new Inner().label();
    }
}
