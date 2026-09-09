package com.example.flow;

public class Flow {
    public String process(int n) {
        if (n < 0) {
            return "negative";
        }
        for (int i = 0; i < n; i++) {
            if (i == 3) {
                break;
            }
        }
        try {
            n = n / 0;
        } catch (ArithmeticException e) {
            return "divide-by-zero";
        }
        while (n > 10) {
            n--;
        }
        if (n == 0) {
            return "zero";
        }
        return "done";
    }
}
