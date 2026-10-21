#!/bin/bash
# Образцы S/MIME от независимой реализации (OpenSSL) для тестов postman-smime. Ключи — только тестовые.
set -e
cd "$(dirname "$0")"
export MSYS_NO_PATHCONV=1
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out ca.key
openssl req -x509 -new -key ca.key -subj "/CN=Postman Test OpenSSL CA/O=Postman Test" -not_before 20260101000000Z -not_after 20460101000000Z -extensions ca -config ext.cnf -out ca.crt
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out alice.key
openssl req -new -key alice.key -subj "/CN=Alice Test/O=Postman Test" -config ext.cnf -out alice.csr
openssl x509 -req -in alice.csr -CA ca.crt -CAkey ca.key -set_serial 0x2001 -not_before 20260101000000Z -not_after 20360101000000Z -extfile ext.cnf -extensions alice -out alice.crt
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out bob.key
openssl req -new -key bob.key -subj "/CN=Bob Test/O=Postman Test" -config ext.cnf -out bob.csr
openssl x509 -req -in bob.csr -CA ca.crt -CAkey ca.key -set_serial 0x2002 -not_before 20260101000000Z -not_after 20360101000000Z -extfile ext.cnf -extensions bob -out bob.crt
printf 'Content-Type: text/plain; charset=utf-8\n\nHello from OpenSSL.\nSecond line.\n' > part.txt
openssl cms -sign -in part.txt -signer alice.crt -inkey alice.key -certfile ca.crt -md sha256 -from alice@itt.test -to bob@itt.test -subject "OpenSSL RSA SHA-256" -out rsa-sha256.eml
openssl cms -sign -in part.txt -signer alice.crt -inkey alice.key -md sha256 -keyopt rsa_padding_mode:pss -from alice@itt.test -to bob@itt.test -subject "OpenSSL RSA-PSS" -out rsa-pss.eml
openssl cms -sign -in part.txt -signer alice.crt -inkey alice.key -md sha1 -from alice@itt.test -to bob@itt.test -subject "OpenSSL RSA SHA-1" -out rsa-sha1.eml
openssl cms -sign -in part.txt -signer bob.crt -inkey bob.key -md sha384 -from bob@itt.test -to alice@itt.test -subject "OpenSSL ECDSA P-256" -out ec-p256.eml
openssl cms -sign -in part.txt -signer alice.crt -inkey alice.key -nodetach -md sha512 -from alice@itt.test -to bob@itt.test -subject "OpenSSL opaque" -out rsa-opaque.eml
openssl cms -sign -in part.txt -signer alice.crt -inkey alice.key -md sha256 -keyid -crlfeol -from alice@itt.test -to bob@itt.test -subject "OpenSSL SKI CRLF" -out rsa-ski.eml
ls -la
