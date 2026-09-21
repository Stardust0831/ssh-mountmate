//! Explain explicit SSH failures without guessing that every mount error is a
//! bad password. Classification only uses diagnostics from the current attempt.
use crate::i18n::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshFailure {
    Authentication,
    PrivateKey,
    HostKey,
    Hostname,
    Refused,
    Timeout,
    Unreachable,
    Negotiation,
    Disconnected,
}

pub fn diagnose(cause: &str) -> Option<SshFailure> {
    // Scan before the UI clips the log, including OpenSSH/Plink stderr which
    // can precede rclone's final, less informative "unexpected EOF" message.
    let cause = cause.to_ascii_lowercase();
    let has = |patterns: &[&str]| patterns.iter().any(|pattern| cause.contains(pattern));
    if has(&[
        "knownhosts: key mismatch",
        "host key verification failed",
        "remote host identification has changed",
        "host key does not match",
    ]) {
        return Some(SshFailure::HostKey);
    }
    if has(&[
        "ssh: this private key is passphrase protected",
        "ssh: no key found",
        "ssh: cannot decode encrypted private keys",
        "ssh: failed to parse private key",
        "unable to load key file",
        "unable to parse private key",
        "unprotected private key file",
    ]) || (cause.contains("load key ")
        && has(&[
            "incorrect passphrase",
            "invalid format",
            "permission denied",
            "no such file",
        ]))
    {
        return Some(SshFailure::PrivateKey);
    }
    if has(&[
        "unable to authenticate",
        "authentication failed",
        "no supported authentication methods available",
        "no supported methods remain",
        "permission denied (publickey",
        "permission denied (password",
        "permission denied (keyboard-interactive",
        "too many authentication failures",
        "server refused our key",
        "access denied for 'password'",
    ]) && has(&[
        "ssh",
        "sftp",
        "publickey",
        "plink",
        "server refused our key",
        "permission denied (password",
        "permission denied (keyboard-interactive",
        "no supported authentication methods available",
        "too many authentication failures",
    ]) {
        return Some(SshFailure::Authentication);
    }
    if has(&[
        "could not resolve hostname",
        "no such host",
        "name or service not known",
        "nodename nor servname provided",
        "temporary failure in name resolution",
        "getaddrinfo: no address associated with hostname",
    ]) && has(&["ssh", "sftp", "dial tcp", "hostname"])
    {
        return Some(SshFailure::Hostname);
    }

    // Require a connection/handshake diagnostic, not a bare timeout or EOF:
    // SFTP can fail after login. Explicit OpenSSH stderr still takes priority
    // over rclone's generic "couldn't initialise SFTP" wrapper on the next line.
    let connecting = has(&[
        "couldn't connect ssh",
        "ssh: connect to host",
        "ssh: handshake failed",
        "dial tcp",
        "kex_exchange_identification",
        "ssh_exchange_identification",
        "could not obtain the server host key",
        "network error:", // Plink
    ]);
    if connecting && has(&["connection refused", "actively refused"]) {
        return Some(SshFailure::Refused);
    }
    if connecting
        && has(&[
            "connection timed out",
            "i/o timeout",
            "operation timed out",
            "connection timeout",
        ])
    {
        return Some(SshFailure::Timeout);
    }
    if connecting
        && has(&[
            "no route to host",
            "network is unreachable",
            "network is down",
        ])
    {
        return Some(SshFailure::Unreachable);
    }
    if has(&[
        "no matching key exchange method",
        "no matching host key type",
        "no matching cipher",
        "no matching mac found",
        "ssh: no common algorithm",
    ]) {
        return Some(SshFailure::Negotiation);
    }
    if connecting
        && has(&[
            "connection reset",
            "connection closed",
            "remote host has closed",
            "unexpected eof",
            "handshake failed: eof",
            "handshake failed: ssh: disconnect",
        ])
    {
        return Some(SshFailure::Disconnected);
    }
    None
}

impl SshFailure {
    pub fn title(self, locale: Locale) -> &'static str {
        match (self, locale) {
            (Self::Authentication, Locale::Chinese) => "SSH 登录失败：身份验证未通过",
            (Self::Authentication, Locale::English) => "SSH login failed: authentication rejected",
            (Self::PrivateKey, Locale::Chinese) => "SSH 登录失败：无法使用私钥",
            (Self::PrivateKey, Locale::English) => "SSH login failed: private key unavailable",
            (Self::HostKey, Locale::Chinese) => "SSH 连接中止：服务器指纹校验失败",
            (Self::HostKey, Locale::English) => {
                "SSH connection stopped: server fingerprint verification failed"
            }
            (Self::Hostname, Locale::Chinese) => "SSH 连接失败：无法解析服务器地址",
            (Self::Hostname, Locale::English) => {
                "SSH connection failed: server address could not be resolved"
            }
            (Self::Refused, Locale::Chinese) => "SSH 连接失败：目标端口拒绝连接",
            (Self::Refused, Locale::English) => {
                "SSH connection failed: target port refused the connection"
            }
            (Self::Timeout, Locale::Chinese) => "SSH 连接超时：服务器未及时响应",
            (Self::Timeout, Locale::English) => {
                "SSH connection timed out: server did not respond in time"
            }
            (Self::Unreachable, Locale::Chinese) => "SSH 连接失败：无法到达服务器网络",
            (Self::Unreachable, Locale::English) => {
                "SSH connection failed: server network is unreachable"
            }
            (Self::Negotiation, Locale::Chinese) => "SSH 连接失败：加密算法协商失败",
            (Self::Negotiation, Locale::English) => {
                "SSH connection failed: no compatible encryption algorithm"
            }
            (Self::Disconnected, Locale::Chinese) => "SSH 登录未完成：连接已断开",
            (Self::Disconnected, Locale::English) => {
                "SSH login did not complete: connection closed"
            }
        }
    }

    pub fn summary(self, locale: Locale) -> String {
        let hint = match (self, locale) {
            (Self::Authentication, Locale::Chinese) => {
                "请用相同地址、端口和账号测试 SSH，并核对认证方式、密码或密钥及账号状态。"
            }
            (Self::Authentication, Locale::English) => {
                "Test SSH with the same host, port and account; check login method, credentials and account status."
            }
            (Self::PrivateKey, Locale::Chinese) => {
                "请检查私钥文件、读取权限和私钥口令，再重试登录。"
            }
            (Self::PrivateKey, Locale::English) => {
                "Check the private key file, its permissions and passphrase, then retry login."
            }
            (Self::HostKey, Locale::Chinese) => "请先向管理员核实服务器指纹，再重新确认连接。",
            (Self::HostKey, Locale::English) => {
                "Verify the server fingerprint with its administrator before confirming the connection again."
            }
            (Self::Hostname, Locale::Chinese) => {
                "请检查地址、SSH 别名及 DNS/VPN；用相同配置测试 SSH 登录。"
            }
            (Self::Hostname, Locale::English) => {
                "Check the address, SSH alias and DNS/VPN; test SSH with the same configuration."
            }
            (Self::Refused, Locale::Chinese) => {
                "请检查地址、端口和服务器 SSH 服务；若直接 SSH 也失败，请联系管理员。"
            }
            (Self::Refused, Locale::English) => {
                "Check the host, port and SSH service; contact the administrator if direct SSH also fails."
            }
            (Self::Timeout | Self::Unreachable, Locale::Chinese) => {
                "请检查网络、VPN、地址和端口；先用相同配置测试 SSH，恢复后再挂载。"
            }
            (Self::Timeout | Self::Unreachable, Locale::English) => {
                "Check network/VPN, host and port; test SSH with the same configuration before mounting again."
            }
            (Self::Negotiation, Locale::Chinese) => {
                "请用相同配置测试 SSH，并向管理员核对客户端与服务器支持的算法。"
            }
            (Self::Negotiation, Locale::English) => {
                "Test SSH with the same configuration; ask the administrator to check supported algorithms."
            }
            (Self::Disconnected, Locale::Chinese) => {
                "请测试相同配置的 SSH 登录，并检查网络或服务器访问限制；此错误无法确定凭据是否有误。"
            }
            (Self::Disconnected, Locale::English) => {
                "Test the same SSH login; check network and server access limits. This does not establish a credential problem."
            }
        };
        format!("{}\n{hint}", self.title(locale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_native_openssh_and_plink_failures() {
        for (cause, expected) in [
            (
                "couldn't connect SSH: ssh: handshake failed: ssh: unable to authenticate, attempted methods [none password], no supported methods remain",
                SshFailure::Authentication,
            ),
            (
                "user@cluster: Permission denied (publickey,password).\nCRITICAL: couldn't initialise SFTP: unexpected EOF",
                SshFailure::Authentication,
            ),
            (
                "user@cluster: Permission denied (keyboard-interactive).",
                SshFailure::Authentication,
            ),
            (
                "plink: FATAL ERROR: No supported authentication methods available (server sent: publickey)",
                SshFailure::Authentication,
            ),
            (
                "ssh: Load key \"id_ed25519\": incorrect passphrase supplied to decrypt private key",
                SshFailure::PrivateKey,
            ),
            (
                "ssh: handshake failed: knownhosts: key mismatch",
                SshFailure::HostKey,
            ),
            (
                "ssh: Could not resolve hostname cluster: Name or service not known",
                SshFailure::Hostname,
            ),
            (
                "couldn't connect SSH: dial tcp: lookup cluster: no such host",
                SshFailure::Hostname,
            ),
            (
                "ssh: connect to host cluster port 22: Connection refused",
                SshFailure::Refused,
            ),
            (
                "ssh: connect to host cluster port 22: Connection refused\nCRITICAL: couldn't initialise SFTP: error receiving version packet: unexpected EOF",
                SshFailure::Refused,
            ),
            ("Network error: Connection refused", SshFailure::Refused),
            (
                "couldn't connect SSH: dial tcp 192.0.2.1:22: i/o timeout",
                SshFailure::Timeout,
            ),
            (
                "could not obtain the server host key: ssh: connect to host cluster port 22: Operation timed out",
                SshFailure::Timeout,
            ),
            (
                "ssh: connect to host cluster port 22: No route to host",
                SshFailure::Unreachable,
            ),
            (
                "Unable to negotiate with 192.0.2.1 port 22: no matching host key type found",
                SshFailure::Negotiation,
            ),
            (
                "kex_exchange_identification: read: Connection reset by peer",
                SshFailure::Disconnected,
            ),
            (
                "couldn't connect SSH: ssh: handshake failed: EOF",
                SshFailure::Disconnected,
            ),
        ] {
            assert_eq!(diagnose(cause), Some(expected), "{cause}");
        }
    }

    #[test]
    fn does_not_blame_ssh_login_for_sftp_or_local_failures() {
        for cause in [
            "invalid mountpoint: Permission denied (os error 13)",
            "SFTP permission denied opening /restricted",
            "could not prepare private rclone RC authentication: authentication failed",
            "couldn't initialise SFTP: error receiving version packet: unexpected EOF",
            "couldn't initialise SFTP: error receiving version packet: packet too long",
            "couldn't initialise SFTP: read tcp 192.0.2.1:22: i/o timeout",
            "SSH transfer failed: i/o timeout",
            "RC connection refused: http://127.0.0.1:5572",
            "mount did not become ready; log: /tmp/ssh-mountmate/mount.log",
        ] {
            assert_eq!(diagnose(cause), None, "{cause}");
        }
    }

    #[test]
    fn every_explanation_fits_the_connection_card() {
        for failure in [
            SshFailure::Authentication,
            SshFailure::PrivateKey,
            SshFailure::HostKey,
            SshFailure::Hostname,
            SshFailure::Refused,
            SshFailure::Timeout,
            SshFailure::Unreachable,
            SshFailure::Negotiation,
            SshFailure::Disconnected,
        ] {
            for locale in [Locale::English, Locale::Chinese] {
                let summary = failure.summary(locale);
                assert_eq!(
                    summary.lines().count(),
                    crate::MOUNT_ERROR_SUMMARY_MAX_LINES
                );
                assert!(summary.chars().count() <= crate::MOUNT_ERROR_SUMMARY_MAX_CHARS);
                assert!(
                    summary
                        .lines()
                        .all(|line| line.chars().count()
                            <= crate::MOUNT_ERROR_SUMMARY_LINE_MAX_CHARS)
                );
            }
        }
    }
}
