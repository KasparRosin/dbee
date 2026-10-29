# Tools image for the dbee test servers (`cargo xtask testdb`, compose.yml): the `init` service
# writes the test secrets with it, and the `sshd` service runs the SSH server. Test-only.
FROM alpine:3.22

RUN apk add --no-cache openssh-server openssl \
 && ssh-keygen -q -t ed25519 -N '' -C dbee-testdb-host -f /etc/ssh/ssh_host_ed25519_key \
 && adduser -D -s /bin/sh dbee \
 && adduser -D -s /bin/sh dbeepw \
 && echo 'dbee:*' | chpasswd -e

# `dbee` logs in with the keys in /secrets/authorized_keys; `dbeepw` only with the password the
# `sshd` service sets at start. Forwarding reaches the Postgres, MySQL and MariaDB servers and
# nothing else.
COPY <<'EOF_CONFIG' /etc/ssh/sshd_config
Port 22
HostKey /etc/ssh/ssh_host_ed25519_key
PidFile /run/sshd.pid
PermitRootLogin no
AllowUsers dbee dbeepw
AuthorizedKeysFile /secrets/authorized_keys
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
AllowTcpForwarding yes
PermitOpen pg14:5432 pgnew:5432 mysql57:3306 mysql80:3306 mysqlnew:3306 mariadb1011:3306 mariadbnew:3306
AllowAgentForwarding no
AllowStreamLocalForwarding no
GatewayPorts no
X11Forwarding no
PermitTunnel no
# The SSH tests fail logins and leave before logging in on purpose (wrong password, unknown host
# key), all from the Docker gateway's one address, and run in parallel: no per-source penalties
# (OpenSSH 9.8+ would soon refuse every connection) and no early drops of unauthenticated ones.
PerSourcePenalties no
MaxStartups 100

Match User dbeepw
    PubkeyAuthentication no
    PasswordAuthentication yes
EOF_CONFIG

COPY --chmod=0755 tools-init.sh /usr/local/bin/tools-init.sh
