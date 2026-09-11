package com.example.portal.service;

import com.example.portal.auth.AuthResult;
import com.example.portal.auth.CredentialChecker;
import com.example.portal.auth.SsoAuthPort;
import com.example.portal.notification.LoginNotifier;
import com.example.portal.repo.UserRepository;
import org.springframework.stereotype.Service;

/**
 * 门户认证：本地口令与 SSO 双通道。
 *
 * 条件多实现：CredentialChecker 有两个实现（local / legacy），
 * 运行时按配置 portal.auth.mode 选择注入，源码无法确定实际用哪个。
 *
 * 按月分表：用户表与登录事件表的物理表名带月份后缀（如 portal_user_202609），
 * 运行时按当前月份拼接，源码无法确定唯一物理表名。
 */
@Service
public class AuthService {

    private final UserRepository userRepository;
    private final SsoAuthPort ssoAuthPort;
    private final CredentialChecker credentialChecker;
    private final LoginNotifier loginNotifier;

    public AuthService(
            UserRepository userRepository,
            SsoAuthPort ssoAuthPort,
            CredentialChecker credentialChecker,
            LoginNotifier loginNotifier) {
        this.userRepository = userRepository;
        this.ssoAuthPort = ssoAuthPort;
        this.credentialChecker = credentialChecker;
        this.loginNotifier = loginNotifier;
    }

    public AuthResult authenticate(String username, String password) {
        if (username != null && username.startsWith("sso:")) {
            return ssoAuthPort.exchangeToken(username.substring(4), password);
        }
        String month = java.time.LocalDate.now().toString().substring(0, 7).replace("-", "");
        UserRecord user = userRepository.findActive(month, username);
        if (user == null) {
            throw new IllegalArgumentException("用户不存在或已停用");
        }
        if (!user.passwordHash().equals(credentialChecker.hash(password))) {
            userRepository.insertLoginEvent(month, username, "BAD_PASSWORD");
            loginNotifier.notifyFailedLogin(username);
            throw new IllegalArgumentException("用户名或密码错误");
        }
        userRepository.insertLoginEvent(month, username, "SUCCESS");
        loginNotifier.notifyLoginSucceeded(username);
        return new AuthResult("token-" + user.id(), false);
    }
}
