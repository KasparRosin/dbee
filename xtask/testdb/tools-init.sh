#!/bin/sh
# The `init` service of compose.yml: writes the dbee test servers' secrets into /secrets, once.
#   - ca.crt, and server.crt / server.key for the Postgres servers (ECDSA P-256; SAN localhost,
#     127.0.0.1 and every database service's name); the key is mode 0600 and owned by uid 70, the
#     postgres user on Alpine.
#   - mysql/server.crt / mysql/server.key for the MySQL and MariaDB servers: RSA 2048, the same names
#     and CA. MySQL 5.7's image (OpenSSL 1.0.2) offers no cipher for an ECDSA key. The key is owned
#     by uid 999, the mysql user of the MySQL and MariaDB images.
#   - SSH client keys id_ed25519, id_ed25519_enc (passphrase $DBEE_TEST_PASSPHRASE) and id_rsa
#     (3072 bits), with their public halves in authorized_keys.
# Synthetic, test-only material for `cargo xtask testdb`; a second run changes nothing.
set -eu

out=/secrets
if [ -f "$out/.done" ]; then
    echo "tools-init: $out is complete, nothing to do"
    exit 0
fi
: "${DBEE_TEST_PASSPHRASE:?DBEE_TEST_PASSPHRASE must be set}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

# Certificate authority and server certificate: ECDSA P-256, valid for ten years.
openssl req -x509 -new -nodes -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -sha256 -days 3650 \
    -subj "/CN=dbee test CA" -keyout ca.key -out ca.crt \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -new -nodes -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -sha256 \
    -subj "/CN=localhost" -keyout server.key -out server.csr
cat > server.ext <<'EXT'
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, IP:127.0.0.1, DNS:pg14, DNS:pgnew, DNS:mysql57, DNS:mysql80, DNS:mysqlnew, DNS:mariadb1011, DNS:mariadbnew
EXT
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -sha256 -days 3650 \
    -extfile server.ext -out server.crt
# The MySQL servers' certificate; keyEncipherment too, for clients that pick RSA key exchange.
openssl req -new -nodes -newkey rsa:2048 -sha256 -subj "/CN=localhost" -keyout mysql-server.key -out mysql-server.csr
sed 's/^keyUsage = .*/keyUsage = critical, digitalSignature, keyEncipherment/' server.ext > mysql-server.ext
openssl x509 -req -in mysql-server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -sha256 -days 3650 \
    -extfile mysql-server.ext -out mysql-server.crt

# SSH client keys.
ssh-keygen -q -t ed25519 -N '' -C dbee-test-plain -f id_ed25519
ssh-keygen -q -t ed25519 -N "$DBEE_TEST_PASSPHRASE" -C dbee-test-encrypted -f id_ed25519_enc
ssh-keygen -q -t rsa -b 3072 -N '' -C dbee-test-rsa -f id_rsa
cat id_ed25519.pub id_ed25519_enc.pub id_rsa.pub > authorized_keys

install -m 0644 ca.crt server.crt authorized_keys id_ed25519.pub id_ed25519_enc.pub id_rsa.pub "$out/"
install -m 0600 -o 70 -g 70 server.key "$out/server.key"
install -d -m 0755 "$out/mysql"
install -m 0644 mysql-server.crt "$out/mysql/server.crt"
install -m 0600 -o 999 -g 999 mysql-server.key "$out/mysql/server.key"
install -m 0600 id_ed25519 id_ed25519_enc id_rsa "$out/"
touch "$out/.done"
echo "tools-init: wrote $out"
