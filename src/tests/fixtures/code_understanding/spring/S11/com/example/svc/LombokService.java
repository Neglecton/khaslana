package com.example.svc;

import com.example.dto.UserDto;
import lombok.RequiredArgsConstructor;
import org.springframework.stereotype.Service;

@Service
@RequiredArgsConstructor
public class LombokService {

    private final UserRepo userRepo;

    public String label(long id) {
        UserDto dto = userRepo.load(id);
        return dto.getName();
    }
}
