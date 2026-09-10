package com.example.crm.repository;

import com.example.crm.entity.UserEntity;
import java.util.Optional;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.query.Param;

public interface UserRepository extends JpaRepository<UserEntity, Long> {
    Optional<UserEntity> findByUsernameAndEnabledTrue(String username);

    @Query("SELECT COUNT(u) FROM UserEntity u WHERE u.username = :username AND u.failedAttempts >= 5")
    long countLocked(@Param("username") String username);
}
