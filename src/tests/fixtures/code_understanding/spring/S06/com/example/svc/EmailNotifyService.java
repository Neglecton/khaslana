package com.example.svc;

import org.springframework.context.annotation.Profile;
import org.springframework.stereotype.Service;

@Service
@Profile("prod")
public class EmailNotifyService implements NotifyService {
    @Override
    public String channel() {
        return "email";
    }
}
