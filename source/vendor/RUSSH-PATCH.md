# russh 0.63.3 local patch

Vendored from the published russh 0.63.3 crate. Original archive SHA-256:
036204edbd199552a5b3832f63c60dcdf395dc44c7f06b4af1c0e8139cc11bce.

Upstream: https://github.com/warp-tech/russh
The original licenses, tests and public test keys are retained.

Changes:
- Suspend client global keepalive requests during key exchange; always re-arm the timer.
- Bound rekeying separately with Config::key_exchange_timeout (default 60 seconds).
- Report Error::KeyExchangeTimeout separately from Error::KeepaliveTimeout.
- Gate the decompression-only constant with the existing compression feature.

The 0.11.6 controller updates rsa to 0.10.0, pkcs1 to 0.8.0 and ssh-key to 0.7.0-rc.12. The existing RSA format readers use the new private-key type names. Host signature and managed Ed25519 identity checks still follow the same application paths.
Regression coverage: source/controller-rust/tests/ssh_liveness.rs.

The crate archive omits a standalone license file. This repository supplies the standard Apache 2.0 text in russh/LICENSE and its third-party notices; original source copyright and license headers remain unchanged.
