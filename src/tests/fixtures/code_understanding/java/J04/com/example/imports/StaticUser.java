package com.example.imports;

import static java.lang.Math.max;
import static java.util.Collections.emptyList;

import java.util.*;

public class StaticUser {
    public int pick(int a, int b) {
        return max(a, b);
    }

    public List<String> emptyNames() {
        List<String> names = new ArrayList<>();
        return emptyList();
    }
}
