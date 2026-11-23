//! Release smoke test against a disposable loopback OpenSSH server with an RSA host key.
use russh::{
    client,
    keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate},
};
use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

struct Pinned(PublicKey);
impl client::Handler for Pinned {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(key.certificate().is_none() && key.public_key().key_data() == self.0.key_data())
    }
}

#[tokio::test]
#[ignore = "requires an isolated OpenSSH SFTP fixture"]
async fn rsa_host_ed25519_user_sftp_read() {
    let dir = PathBuf::from(env::var_os("YJ_TEST_SFTP_DIR").unwrap());
    let port = env::var("YJ_TEST_SFTP_PORT").unwrap().parse().unwrap();
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let host = PublicKey::from_openssh(&fs::read_to_string(dir.join("host.pub")).unwrap()).unwrap();
    assert!(matches!(
        host.algorithm(),
        russh::keys::Algorithm::Rsa { .. }
    ));
    let user = PrivateKey::from_openssh(fs::read(dir.join("identity")).unwrap()).unwrap();
    assert_eq!(user.algorithm(), russh::keys::Algorithm::Ed25519);
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut client = client::connect(Arc::new(client::Config::default()), addr, Pinned(host))
            .await
            .unwrap();
        assert!(
            client
                .authenticate_publickey("root", PrivateKeyWithHashAlg::new(Arc::new(user), None))
                .await
                .unwrap()
                .success()
        );
        let channel = client.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .unwrap();
        assert_eq!(sftp.read("/marker").await.unwrap(), b"release fixture\n");
        sftp.close().await.unwrap();
    })
    .await
    .unwrap();
}
