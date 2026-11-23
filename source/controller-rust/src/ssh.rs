use crate::{core::*, model::NodeSecret, realtime::AgentLink};
use base64::{Engine, engine::general_purpose::STANDARD};
use russh::{
    ChannelMsg, client,
    keys::{
        Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
    },
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};
use zeroize::Zeroizing;
pub type Client = client::Handle<Verifier>;
pub struct Verifier {
    pub expected: Option<Vec<u8>>,
    pub inspected: Arc<Mutex<Option<PublicKey>>>,
}
impl client::Handler for Verifier {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if key.certificate().is_some() {
            return Ok(false);
        }
        let public = key.public_key();
        let bytes = public.to_bytes().map_err(|_| russh::Error::UnknownKey)?;
        *self.inspected.lock().unwrap() = Some(public);
        Ok(self.expected.as_ref().is_none_or(|v| v == &bytes))
    }
}
fn algorithms(key: Option<&PublicKey>) -> Vec<Algorithm> {
    if let Some(k) = key {
        if matches!(k.algorithm(), Algorithm::Rsa { .. }) {
            vec![
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512),
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256),
                },
            ]
        } else {
            vec![k.algorithm()]
        }
    } else {
        vec![
            Algorithm::Ed25519,
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP256,
            },
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP384,
            },
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP521,
            },
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha512),
            },
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha256),
            },
        ]
    }
}
pub async fn connect<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: T,
    pin: Option<&str>,
    inspected: Arc<Mutex<Option<PublicKey>>>,
) -> Result<Client, &'static str> {
    let raw = pin
        .map(|p| STANDARD.decode(p).map_err(|_| "host key invalid"))
        .transpose()?;
    let key = raw
        .as_ref()
        .map(|v| PublicKey::from_bytes(v).map_err(|_| "host key invalid"))
        .transpose()?;
    let mut cfg = client::Config {
        window_size: 256 * 1024,
        maximum_packet_size: 32 * 1024,
        channel_buffer_size: 16,
        // Liveness is enforced by SSH keepalives and the authenticated terminal owner.
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        key_exchange_timeout: Some(Duration::from_secs(60)),
        nodelay: true,
        ..Default::default()
    };
    cfg.preferred.key = algorithms(key.as_ref()).into();
    timeout(
        Duration::from_secs(12),
        client::connect_stream(
            Arc::new(cfg),
            stream,
            Verifier {
                expected: raw,
                inspected,
            },
        ),
    )
    .await
    .map_err(|_| "SSH connection timeout")?
    .map_err(|_| "SSH handshake failed")
}
pub async fn agent_client(
    app: &App,
    link: Arc<AgentLink>,
    node_id: &str,
    user: &str,
    secret: &NodeSecret,
) -> Result<Arc<Client>, &'static str> {
    let raw = app.unseal(&format!("{node_id}:ssh"), &secret.ssh_key)?;
    let key = PrivateKey::from_openssh(raw.as_slice()).map_err(|_| "SSH identity invalid")?;
    // Managed identities have always been Ed25519. RSA support is used only to verify public host signatures.
    // No RSA private-key signing or decryption can be reached through this application.
    if key.algorithm() != Algorithm::Ed25519 {
        return Err("unsupported managed SSH identity");
    }
    let stream = link.open().await?;
    let mut client = connect(
        stream,
        (!secret.host_key.is_empty()).then_some(secret.host_key.as_str()),
        Arc::new(Mutex::new(None)),
    )
    .await?;
    let ok = timeout(
        Duration::from_secs(10),
        client.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None)),
    )
    .await
    .map_err(|_| "SSH authentication timeout")?
    .map_err(|_| "SSH authentication failed")?;
    if !ok.success() {
        return Err("SSH authentication failed");
    }
    Ok(Arc::new(client))
}
pub async fn password_client(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> Result<(Arc<Client>, Vec<u8>), &'static str> {
    let stream = timeout(
        Duration::from_secs(12),
        crate::ssh_address::connect(host, port),
    )
    .await
    .map_err(|_| "SSH connection timeout")?
    .map_err(|_| "SSH connection failed")?;
    let observed = Arc::new(Mutex::new(None));
    let mut client = connect(stream, None, observed.clone()).await?;
    let ok = timeout(
        Duration::from_secs(10),
        client.authenticate_password(user, password),
    )
    .await
    .map_err(|_| "SSH authentication timeout")?
    .map_err(|_| "SSH authentication failed")?;
    if !ok.success() {
        return Err("SSH authentication failed");
    }
    Ok((Arc::new(client), observed_bytes(&observed)?))
}
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub async fn exec(
    client: &Client,
    command: &str,
    input: &[u8],
    limit: usize,
) -> Result<String, &'static str> {
    let channel = client
        .channel_open_session()
        .await
        .map_err(|_| "SSH channel unavailable")?;
    channel
        .exec(true, command)
        .await
        .map_err(|_| "SSH command unavailable")?;
    let (mut reader, writer) = channel.split();
    let feed = async {
        if !input.is_empty() {
            writer.data(input).await.map_err(|_| "SSH input failed")?;
        }
        writer.eof().await.map_err(|_| "SSH input failed")
    };
    let receive = async {
        let mut out = Vec::new();
        let mut status = None;
        let mut accepted = false;
        while let Some(m) = reader.wait().await {
            match m {
                ChannelMsg::Success => accepted = true,
                ChannelMsg::Failure => return Err("SSH command rejected"),
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    if out.len() + data.len() > limit {
                        return Err("SSH output limit reached");
                    }
                    out.extend(data.as_ref());
                }
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        if status != Some(0) || !accepted {
            return Err("remote command failed");
        }
        String::from_utf8(out).map_err(|_| "remote output invalid")
    };
    let result = tokio::try_join!(feed, receive).map(|(_, output)| output);
    let _ = writer.close().await;
    result
}
pub async fn remote(
    client: &Client,
    user: &str,
    password: &str,
    command: &str,
    input: &[u8],
) -> Result<String, &'static str> {
    if user == "root" {
        return exec(client, command, input, 32768).await;
    }
    let marker = format!("vistart-{}", token());
    let wrapper = format!(
        "IFS= read -r vistart_line || exit 97; if [ \"$vistart_line\" != {} ]; then IFS= read -r vistart_line || exit 97; fi; [ \"$vistart_line\" = {} ] || exit 97; unset vistart_line; {command}",
        quote(&marker),
        quote(&marker)
    );
    let command = format!("sudo -k -S -p '' sh -c {}", quote(&wrapper));
    let mut payload = Zeroizing::new(format!("{password}\n{marker}\n").into_bytes());
    payload.extend_from_slice(input);
    exec(client, &command, &payload, 32768).await
}
pub fn create_key(comment: &str) -> Result<(Zeroizing<Vec<u8>>, String), &'static str> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut()).map_err(|_| "random unavailable")?;
    let pair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let key = PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(pair),
        comment,
    )
    .map_err(|_| "SSH key generation failed")?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|_| "SSH key serialization failed")?;
    let private = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|_| "SSH key serialization failed")?;
    Ok((Zeroizing::new(private.as_bytes().to_vec()), public))
}

fn observed_bytes(observed: &Mutex<Option<PublicKey>>) -> Result<Vec<u8>, &'static str> {
    observed
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("SSH key unavailable")?
        .to_bytes()
        .map_err(|_| "SSH key invalid")
}

// Only the SSH client's recovery identity is persisted. Deriving it from the negotiated
// server key prevents unattended recovery from disclosing an Agent token to a reassigned IP.
fn recovery_identity(seed: &[u8], host: &[u8]) -> Result<PrivateKey, &'static str> {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(seed).map_err(|_| "recovery seed invalid")?;
    mac.update(b"YJmonitor recovery SSH identity v1\0");
    mac.update(host);
    let secret = Zeroizing::new(<[u8; 32]>::from(mac.finalize().into_bytes()));
    Ok(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&secret).into())
}
pub fn bind_recovery(
    app: &App,
    id: &str,
    secret: &mut NodeSecret,
    host: &[u8],
) -> Result<(), &'static str> {
    let seed = app.unseal(&format!("{id}:recovery"), &secret.recovery_key)?;
    let key = recovery_identity(&seed, host)?;
    secret.recovery_public = key
        .public_key()
        .to_openssh()
        .map_err(|_| "invalid recovery identity")?;
    secret.derived_recovery = true;
    secret.host_key.clear();
    Ok(())
}

trait RecoveryStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RecoveryStream for T {}
pub async fn recovery_client(
    app: &App,
    node: &crate::model::Node,
    secret: &NodeSecret,
) -> Result<(Arc<Client>, Vec<u8>), &'static str> {
    let raw = app.unseal(
        &format!("{}:recovery", node.public.id),
        &secret.recovery_key,
    )?;
    let host = &node.ip;
    let port = u16::try_from(node.port).map_err(|_| "node port invalid")?;
    let stream: Box<dyn RecoveryStream> = match timeout(
        Duration::from_secs(8),
        crate::ssh_address::connect(host, port),
    )
    .await
    {
        Ok(Ok(s)) => Box::new(s),
        _ => {
            let link = app
                .lock()
                .agents
                .get(&node.public.id)
                .cloned()
                .ok_or("SSH unreachable")?;
            Box::new(link.open().await?)
        }
    };
    let observed = Arc::new(Mutex::new(None));
    let pin = (!secret.host_key.is_empty()).then_some(secret.host_key.as_str());
    if pin.is_none() && !secret.derived_recovery {
        return Err("recovery identity unavailable");
    }
    let mut client = connect(stream, pin, observed.clone()).await?;
    let host_key = observed_bytes(&observed)?;
    let key = if secret.derived_recovery {
        let key = recovery_identity(&raw, &host_key)?;
        if key
            .public_key()
            .to_openssh()
            .map_err(|_| "invalid recovery identity")?
            != secret.recovery_public
        {
            return Err("recovery identity changed");
        }
        key
    } else {
        PrivateKey::from_openssh(raw.as_slice()).map_err(|_| "recovery identity invalid")?
    };
    if key.algorithm() != Algorithm::Ed25519 {
        return Err("unsupported recovery identity");
    }
    let ok = timeout(
        Duration::from_secs(10),
        client.authenticate_publickey(
            &node.username,
            PrivateKeyWithHashAlg::new(Arc::new(key), None),
        ),
    )
    .await
    .map_err(|_| "recovery authentication timeout")?
    .map_err(|_| "recovery authentication failed")?;
    if !ok.success() {
        return Err("recovery key rejected");
    }
    Ok((Arc::new(client), host_key))
}

#[cfg(test)]
mod host_key_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct PasswordObserver(Arc<AtomicUsize>);
    impl russh::server::Handler for PasswordObserver {
        type Error = russh::Error;
        async fn auth_password(
            &mut self,
            _: &str,
            _: &str,
        ) -> Result<russh::server::Auth, Self::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(russh::server::Auth::Accept)
        }
    }
    #[test]
    fn unattended_recovery_credentials_are_specific_to_the_enrolled_server() {
        let first = recovery_identity(b"private node seed", b"server one").unwrap();
        let same = recovery_identity(b"private node seed", b"server one").unwrap();
        let other = recovery_identity(b"private node seed", b"server two").unwrap();
        assert_eq!(first.public_key(), same.public_key());
        assert_ne!(first.public_key(), other.public_key());
    }
    #[tokio::test]
    async fn password_install_accepts_first_and_rotated_hosts_and_legacy_recovery_checks_its_key() {
        timeout(Duration::from_secs(8), async {
            let (raw, _) = create_key("rotated-host-fixture").unwrap();
            let host = PrivateKey::from_openssh(raw.as_slice()).unwrap();
            let new_pin = STANDARD.encode(host.public_key().to_bytes().unwrap());
            let (raw, _) = create_key("old-host-fixture").unwrap();
            let old = PrivateKey::from_openssh(raw.as_slice()).unwrap();
            let old_pin = STANDARD.encode(old.public_key().to_bytes().unwrap());
            for pin in [None, Some(old_pin.as_str()), Some(new_pin.as_str())] {
                let attempts = Arc::new(AtomicUsize::new(0));
                let observer = PasswordObserver(attempts.clone());
                let (local, remote) = tokio::io::duplex(65536);
                let config = Arc::new(russh::server::Config {
                    keys: vec![host.clone()],
                    ..Default::default()
                });
                let server = tokio::spawn(async move {
                    if let Ok(s) = russh::server::run_stream(config, remote, observer).await {
                        let _ = s.await;
                    }
                });
                let inspected = Arc::new(Mutex::new(None));
                let result = connect(local, pin, inspected.clone()).await;
                assert_eq!(
                    inspected.lock().unwrap().as_ref().unwrap().key_data(),
                    host.public_key().key_data()
                );
                if pin.is_none() || pin == Some(new_pin.as_str()) {
                    let mut client = result.unwrap();
                    assert!(
                        client
                            .authenticate_password("fixture", "not-a-real-password")
                            .await
                            .unwrap()
                            .success()
                    );
                    assert_eq!(attempts.load(Ordering::SeqCst), 1);
                } else {
                    assert!(result.is_err());
                    assert_eq!(attempts.load(Ordering::SeqCst), 0);
                }
                server.abort();
            }
        })
        .await
        .unwrap();
    }
}
