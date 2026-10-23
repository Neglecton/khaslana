//! SSH 身份和主机信任的界面资源；不包含私钥或口令。
use khaslana::ui_i18n::UiMessage;

pub(crate) const COPY_FINGERPRINT: UiMessage = UiMessage::new("ssh_ui.copy_fingerprint", "复制指纹", "Copy fingerprint");
pub(crate) const FINGERPRINT_COPIED: UiMessage = UiMessage::new("ssh_ui.fingerprint_copied", "指纹已复制", "Fingerprint copied");
pub(crate) const DIRECTORY: UiMessage = UiMessage::new("ssh_ui.directory", "实际保存目录：{path}", "Actual key directory: {path}");
pub(crate) const PRIVATE_PATH: UiMessage = UiMessage::new("ssh_ui.private_path", "私钥路径：{path}", "Private key path: {path}");
pub(crate) const PUBLIC_PATH: UiMessage = UiMessage::new("ssh_ui.public_path", "公钥路径：{path}", "Public key path: {path}");
pub(crate) const KEYS: UiMessage = UiMessage::new("ssh_ui.keys", "托管 SSH 密钥", "Managed SSH keys");
pub(crate) const KEY_HELP: UiMessage = UiMessage::new("ssh_ui.key_help", "密钥仅保存在应用数据目录，不会写入 ~/.ssh。取消凭据表单不会删除已生成的密钥。", "Keys stay in the application data directory, never in ~/.ssh. Cancelling a credential form does not delete generated keys.");
pub(crate) const GENERATE: UiMessage = UiMessage::new("ssh_ui.generate", "生成 Ed25519 密钥", "Generate Ed25519 key");
pub(crate) const NAME: UiMessage = UiMessage::new("ssh_ui.name", "密钥名称", "Key name");
pub(crate) const COMMENT: UiMessage = UiMessage::new("ssh_ui.comment", "公钥注释（可选）", "Public key comment (optional)");
pub(crate) const PASSPHRASE: UiMessage = UiMessage::new("ssh_ui.passphrase", "密钥口令", "Key passphrase");
pub(crate) const CONFIRM_PASSPHRASE: UiMessage = UiMessage::new("ssh_ui.confirm_passphrase", "再次输入口令", "Confirm passphrase");
pub(crate) const NO_PASSPHRASE: UiMessage = UiMessage::new("ssh_ui.no_passphrase", "明确使用无口令密钥（不推荐）", "Explicitly use an unencrypted key (not recommended)");
pub(crate) const INVALID_FORM: UiMessage = UiMessage::new("ssh_ui.invalid_form", "请填写名称，并输入一致的非空口令，或明确选择无口令。", "Enter a name and matching non-empty passphrases, or explicitly choose no passphrase.");
pub(crate) const GENERATING: UiMessage = UiMessage::new("ssh_ui.generating", "正在生成密钥…", "Generating key…");
pub(crate) const GENERATED: UiMessage = UiMessage::new("ssh_ui.generated", "密钥已保存。请将公钥添加到 Git 托管平台。", "Key saved. Add the public key to your Git hosting service.");
pub(crate) const COPY_PUBLIC: UiMessage = UiMessage::new("ssh_ui.copy_public", "复制公钥", "Copy public key");
pub(crate) const PUBLIC_COPIED: UiMessage = UiMessage::new("ssh_ui.public_copied", "公钥已复制", "Public key copied");
pub(crate) const USE_CREDENTIAL: UiMessage = UiMessage::new("ssh_ui.use_credential", "用于凭据", "Use for credential");
pub(crate) const DELETE: UiMessage = UiMessage::new("ssh_ui.delete", "删除", "Delete");
pub(crate) const DELETE_KEY: UiMessage = UiMessage::new("ssh_ui.delete_key", "删除托管密钥", "Delete managed key");
pub(crate) const DELETE_HELP: UiMessage = UiMessage::new("ssh_ui.delete_help", "永久删除 {name}？正在被凭据引用的密钥不能删除。", "Permanently delete {name}? Keys referenced by credentials cannot be deleted.");
pub(crate) const EMPTY_KEYS: UiMessage = UiMessage::new("ssh_ui.empty_keys", "尚无托管密钥", "No managed keys");
pub(crate) const HOSTS: UiMessage = UiMessage::new("ssh_ui.hosts", "SSH 主机信任", "SSH host trust");
pub(crate) const HOST_HELP: UiMessage = UiMessage::new("ssh_ui.host_help", "首次连接须核对主机指纹。忘记后，下次连接会重新确认；不匹配的主机密钥不会被静默接受。", "Verify the host fingerprint on first connection. Forgetting a host requires confirmation next time; mismatched host keys are never silently accepted.");
pub(crate) const EMPTY_HOSTS: UiMessage = UiMessage::new("ssh_ui.empty_hosts", "尚无已信任主机", "No trusted hosts");
pub(crate) const FORGET: UiMessage = UiMessage::new("ssh_ui.forget", "忘记", "Forget");
pub(crate) const FORGET_HOST: UiMessage = UiMessage::new("ssh_ui.forget_host", "忘记主机信任", "Forget host trust");
pub(crate) const FORGET_HELP: UiMessage = UiMessage::new("ssh_ui.forget_help", "忘记 {host} 的主机密钥？此操作不会自动信任替换后的密钥。", "Forget the host key for {host}? This does not automatically trust a replacement key.");
pub(crate) const UNKNOWN_HOST: UiMessage = UiMessage::new("ssh_ui.unknown_host", "确认未知 SSH 主机", "Verify unknown SSH host");
pub(crate) const VERIFY_HELP: UiMessage = UiMessage::new("ssh_ui.verify_help", "请通过可信渠道核对主机、端口、算法与 SHA256 指纹。默认不信任；Enter 不会接受，Esc 取消连接。", "Verify the host, port, algorithm and SHA256 fingerprint through a trusted channel. Not trusted by default; Enter does not accept, and Esc cancels the connection.");
pub(crate) const TRUST: UiMessage = UiMessage::new("ssh_ui.trust", "信任此主机并继续", "Trust this host and continue");
pub(crate) const CANCEL: UiMessage = UiMessage::new("ssh_ui.cancel", "取消", "Cancel");
pub(crate) const CLOSE: UiMessage = UiMessage::new("ssh_ui.close", "关闭", "Close");
pub(crate) const REFRESH: UiMessage = UiMessage::new("ssh_ui.refresh", "刷新", "Refresh");
pub(crate) const FAILED: UiMessage = UiMessage::new("ssh_ui.failed", "SSH 操作失败", "SSH operation failed");

#[cfg(test)]
const ALL: &[UiMessage] = &[
    COPY_FINGERPRINT, FINGERPRINT_COPIED, DIRECTORY, PRIVATE_PATH, PUBLIC_PATH,
    KEYS,
    KEY_HELP,
    GENERATE,
    NAME,
    COMMENT,
    PASSPHRASE,
    CONFIRM_PASSPHRASE,
    NO_PASSPHRASE,
    INVALID_FORM,
    GENERATING,
    GENERATED,
    COPY_PUBLIC,
    PUBLIC_COPIED,
    USE_CREDENTIAL,
    DELETE,
    DELETE_KEY,
    DELETE_HELP,
    EMPTY_KEYS,
    HOSTS,
    HOST_HELP,
    EMPTY_HOSTS,
    FORGET,
    FORGET_HOST,
    FORGET_HELP,
    UNKNOWN_HOST,
    VERIFY_HELP,
    TRUST,
    CANCEL,
    CLOSE,
    REFRESH,
    FAILED,
];

#[cfg(test)]
#[test]
fn resource_templates_are_complete() {
    khaslana::ui_i18n::validate(ALL).expect("SSH UI resources must have matching bilingual parameters");
    for message in ALL {
        for language in [khaslana::language::Language::ZhCn, khaslana::language::Language::En] {
            let args: &[(&str, &str)] = &[("name", "用户 {host}"), ("host", "git.example:2222")];
            let encoded = khaslana::ui_i18n::encode(*message, args);
            assert_eq!(khaslana::ui_i18n::resolve_in(language, &encoded), khaslana::ui_i18n::format_in(language, *message, args));
        }
    }
}
