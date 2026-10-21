#!/bin/sh
# A throw-away CA and a server certificate (ECDSA P-256: what iotkit's TLS verifies) for the test
# broker (8883) and update server (8443). Usage: certs/make.sh <this machine's IP address>
# Devices get the CA in DER: `IOTKIT_TLS_CA = "<path>/ca.der"` at build time.
set -eu
cd "$(dirname "$0")"
ip=${1:?"usage: $0 <this machine's IP address>"}
openssl ecparam -name prime256v1 -genkey -noout -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days 3650 -subj "/CN=iotkit test CA" -out ca.crt
openssl x509 -in ca.crt -outform der -out ca.der
openssl ecparam -name prime256v1 -genkey -noout -out server.key
openssl req -new -key server.key -subj "/CN=$ip" -out server.csr
printf 'subjectAltName=IP:%s,DNS:localhost\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=serverAuth\n' "$ip" >server.ext
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 825 -sha256 -extfile server.ext -out server.crt
rm -f server.csr server.ext ca.srl
chmod 644 server.key   # read by the containers' users
echo "certs for $ip: ca.der (devices), server.crt + server.key (broker, update server)"
