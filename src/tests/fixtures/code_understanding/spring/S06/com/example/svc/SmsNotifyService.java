package com.example.svc;

import org.springframework.context.annotation.Conditional;
import org.springframework.stereotype.Service;

@Service
@Conditional(SmsEnabledCondition.class)
public class SmsNotifyService implements NotifyService {
    @Override
    public String channel() {
        return "sms";
    }
}
