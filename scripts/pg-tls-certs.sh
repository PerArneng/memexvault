#!/bin/sh
# Writes throwaway certificates for compose's postgres-tls service to target/pg-tls, which git ignores: a CA, a server certificate
# it signed, and an unrelated CA for testing refusal. Run it again for new ones, then restart the service.
set -eu
cd "$(dirname "$0")/.."
DIR=target/pg-tls
mkdir -p "$DIR"
cd "$DIR"
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=$2" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" -keyout "$1.key" -out "$1.crt" 2>/dev/null
}
ca ca "memexvault test CA"
ca wrong-ca "memexvault unrelated test CA"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=memexvault test server" -keyout server.key -out server.csr 2>/dev/null
cat > server.ext <<'EXT'
subjectAltName=DNS:localhost,DNS:db.memex.test,IP:127.0.0.1
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
EXT
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 30 -extfile server.ext -out server.crt 2>/dev/null
rm server.csr server.ext ca.srl wrong-ca.key ca.key
chmod 600 server.key
echo "certificates are in $DIR"
