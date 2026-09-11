package com.example.portal.auth;

/**
 * 外部 SSO 认证边界：接口在本样例中没有实现类。
 *
 * 真实部署中由公司统一认证中心提供实现；样例不推断其内部行为，
 * 也不把 SSO token 交换过程当作本地数据库访问。
 */
public interface SsoAuthPort {

    AuthResult exchangeToken(String ssoTicket, String rawPassword);
}
