package com.example.svc;

import com.example.dto.UserDto;
import org.springframework.stereotype.Service;

@Service
public class UserRepo {
    public UserDto load(long id) {
        return new UserDto();
    }
}
