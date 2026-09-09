package com.example.rec;

public class Recursor {
    public int twice(int n) {
        int a = step(n);
        int b = step(n);
        return a + b;
    }

    private int step(int n) {
        return n + 1;
    }

    public int fact(int n) {
        if (n <= 1) {
            return 1;
        }
        return n * fact(n - 1);
    }

    public boolean isEven(int n) {
        if (n == 0) return true;
        return isOdd(n - 1);
    }

    private boolean isOdd(int n) {
        if (n == 0) return false;
        return isEven(n - 1);
    }
}
