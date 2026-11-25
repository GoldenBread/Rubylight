//! The Moonlight pairing handshake: PIN-derived key, challenge and response,
//! and the in-progress pairings keyed by client.
use crate::crypto;
use anyhow::{Result, bail};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use zeroize::Zeroize;

/// Device names come from the client (Android sends its model, such as
/// "moto g(60)") or from the console. They are shown escaped and reach
/// commands only through `command_device_name`, so ordinary punctuation is
/// fine; refuse what could still mislead in a log line or markup.
pub fn validate_device_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.chars().count() > 128
        || !name.chars().all(device_name_character)
    {
        bail!(
            "device name must be 1–128 characters without control or text direction characters or any of < > \" ` % $ \\ | ; ^"
        );
    }
    Ok(())
}
fn device_name_character(c: char) -> bool {
    !c.is_control()
        && !is_noncharacter(c)
        && !is_direction_control(c)
        && !matches!(
            c,
            '<' | '>' | '"' | '`' | '%' | '$' | '\\' | '|' | ';' | '^'
        )
}
/// Marks, embeddings, overrides and isolates turn the text around them, so a
/// name could pass for an address beside it; a line separator starts a line of
/// its own. A zero-width joiner is none of these: emoji are made with it.
fn is_direction_control(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}
/// `name` without those characters. A device paired before they were refused
/// may still carry them, and keeps its name; a notice shows it without them.
pub fn without_direction_controls(name: &str) -> String {
    name.chars().filter(|c| !is_direction_control(*c)).collect()
}
fn is_noncharacter(c: char) -> bool {
    let c = c as u32;
    (0xfdd0..=0xfdef).contains(&c) || c & 0xfffe == 0xfffe
}
/// The characters a device name keeps in a command's environment; any
/// other becomes an underscore, so names cannot expand into shell syntax.
fn command_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '\'')
}
pub fn command_device_name(name: &str) -> String {
    name.chars()
        .map(|c| if command_character(c) { c } else { '_' })
        .collect()
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum Phase {
    Cert,
    Challenge,
    Response,
    Complete,
}
pub struct Pairing {
    pub unique_id: String,
    pub name: String,
    pub certificate: String,
    pub created: Instant,
    /// The address that asked for the certificate; the later steps must come
    /// from it, so another device cannot end or advance this pairing.
    pub peer: Option<std::net::IpAddr>,
    /// The permissions the owner chose for this device
    /// (`state::pairing_access`); `None` keeps the default.
    pub permission: Option<u32>,
    /// The paired device that typed the PIN, by uuid; `None` for the console.
    /// Lives here so the cap can't be dropped with anything but the session.
    pub approver: Option<String>,
    /// The invite (`invites`) whose PIN this pairing answered: finishing it spends the
    /// invite and pairs the device as an invited guest.
    pub invite: Option<u32>,
    phase: Phase,
    key: [u8; 16],
    server_secret: [u8; 16],
    server_challenge: [u8; 16],
    client_hash: Vec<u8>,
}
impl Drop for Pairing {
    fn drop(&mut self) {
        self.key.zeroize();
        self.server_secret.zeroize();
    }
}
impl Pairing {
    pub fn new(
        unique_id: String,
        name: String,
        certificate: String,
        salt: &[u8],
        pin: &str,
    ) -> Result<Self> {
        validate_device_name(&name)?;
        if unique_id.is_empty()
            || unique_id.len() > 256
            || !unique_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            bail!("invalid client identity");
        }
        if pin.len() != 4 || !pin.bytes().all(|c| c.is_ascii_digit()) {
            bail!("PIN must contain four digits");
        }
        Self::with_pin(unique_id, name, certificate, salt, pin)
    }
    /// A pairing answering an invite (`invites`): its PIN is 32 hex digits derived from
    /// the invite's secret, never typed, so a recorded pairing gives nobody a PIN to guess.
    pub fn for_invite(
        unique_id: String,
        name: String,
        certificate: String,
        salt: &[u8],
        pin: &str,
        invite: u32,
    ) -> Result<Self> {
        validate_device_name(&name)?;
        if pin.len() != 32 || !pin.bytes().all(|c| c.is_ascii_hexdigit()) {
            bail!("an invite's PIN is 32 hex digits");
        }
        let mut pairing = Self::with_pin(unique_id, name, certificate, salt, pin)?;
        pairing.invite = Some(invite);
        Ok(pairing)
    }
    fn with_pin(
        unique_id: String,
        name: String,
        certificate: String,
        salt: &[u8],
        pin: &str,
    ) -> Result<Self> {
        if unique_id.is_empty()
            || unique_id.len() > 256
            || !unique_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            bail!("invalid client identity");
        }
        if salt.len() < 16 {
            bail!("salt is too short");
        }
        crypto::verify_pairing_certificate(&certificate)?;
        Ok(Self {
            unique_id,
            name,
            certificate,
            created: Instant::now(),
            peer: None,
            permission: None,
            approver: None,
            invite: None,
            phase: Phase::Cert,
            key: crypto::pin_key(salt[..16].try_into().unwrap(), pin),
            server_secret: crypto::random(),
            server_challenge: crypto::random(),
            client_hash: Vec::new(),
        })
    }
    fn require(&mut self, phase: Phase) -> Result<()> {
        if self.phase != phase || self.created.elapsed() > Duration::from_secs(300) {
            self.phase = Phase::Complete;
            bail!("pairing request is expired or out of order");
        }
        Ok(())
    }
    pub fn client_challenge(&mut self, id: &crypto::Identity, challenge: &[u8]) -> Result<Vec<u8>> {
        self.require(Phase::Cert)?;
        if challenge.len() != 16 {
            self.phase = Phase::Complete;
            bail!("invalid challenge length");
        }
        let mut data = crypto::ecb(&self.key, challenge, false)?;
        data.extend_from_slice(&id.signature);
        data.extend_from_slice(&self.server_secret);
        let mut response = crypto::hash(&data).to_vec();
        response.extend_from_slice(&self.server_challenge);
        self.phase = Phase::Challenge;
        crypto::ecb(&self.key, &response, true)
    }
    pub fn server_response(&mut self, id: &crypto::Identity, response: &[u8]) -> Result<Vec<u8>> {
        self.require(Phase::Challenge)?;
        if response.len() != 32 {
            self.phase = Phase::Complete;
            bail!("invalid challenge response length");
        }
        self.client_hash = crypto::ecb(&self.key, response, false)?;
        let mut secret = self.server_secret.to_vec();
        secret.extend_from_slice(&id.sign(&self.server_secret));
        self.phase = Phase::Response;
        Ok(secret)
    }
    pub fn finish(&mut self, secret: &[u8]) -> Result<()> {
        self.require(Phase::Response)?;
        self.phase = Phase::Complete;
        if secret.len() != 16 + 256 {
            bail!("invalid RSA pairing secret length");
        }
        crypto::verify_pairing_certificate(&self.certificate)?;
        let mut data = self.server_challenge.to_vec();
        data.extend_from_slice(&crypto::certificate_signature(&self.certificate)?);
        data.extend_from_slice(&secret[..16]);
        if !crypto::equal(&crypto::hash(&data), &self.client_hash)
            || !crypto::verify(&self.certificate, &secret[..16], &secret[16..])?
        {
            bail!("client proof or PIN verification failed");
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct Pairings {
    pub sessions: HashMap<String, Pairing>,
}
impl Pairings {
    pub fn expire(&mut self) {
        self.sessions
            .retain(|_, s| s.created.elapsed() < Duration::from_secs(300));
    }
    pub fn insert(&mut self, s: Pairing) -> Result<()> {
        self.expire();
        if self.sessions.contains_key(&s.unique_id) {
            bail!("pairing already in progress");
        }
        if self.sessions.len() >= 32 {
            bail!("too many pending pairing sessions");
        }
        self.sessions.insert(s.unique_id.clone(), s);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_names_cannot_expand_into_shell_syntax() {
        for name in ["Living room", "Çağrı's Phone", "日本語", "Phone-2.1_Pro"] {
            assert!(validate_device_name(name).is_ok(), "{name}");
        }
        // Since 2.2.2 these were refused, so pairing failed for Android
        // models with parentheses and for names typed on iOS.
        for name in [
            "moto g(60)",
            "Galaxy S24+",
            "Ramazan’s iPhone",
            "Steam Deck (OLED)",
            "AT&T phone, #2",
        ] {
            assert!(validate_device_name(name).is_ok(), "{name}");
        }
        assert_eq!(command_device_name("moto g(60)"), "moto g_60_");
        assert_eq!(command_device_name("AT&T phone"), "AT_T phone");
        for name in [
            "",
            " ",
            "Phone\" & whoami",
            "%COMSPEC%",
            "$(PATH)",
            "Phone\nentry",
            "Phone\u{fdd1}<form>",
        ] {
            assert!(validate_device_name(name).is_err(), "{name}");
        }
        assert!(validate_device_name(&"a".repeat(129)).is_err());
    }
    #[test]
    fn a_name_cannot_turn_the_text_beside_it() {
        for control in [
            '\u{61c}', '\u{200e}', '\u{200f}', '\u{2028}', '\u{2029}', '\u{202a}', '\u{202b}',
            '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            let name = format!("TV{control}611.4.861.291");
            assert!(validate_device_name(&name).is_err(), "{control:?}");
            assert_eq!(without_direction_controls(&name), "TV611.4.861.291");
        }
        // Moonlight's own names, right-to-left names and emoji still pair.
        for name in [
            "roth",
            "Moonlight Client",
            "טלוויזיה",
            "التلفزيون",
            "👩\u{200d}💻 TV",
            "می\u{200c}خواهم",
        ] {
            assert!(validate_device_name(name).is_ok(), "{name}");
            assert_eq!(without_direction_controls(name), name);
        }
    }
    #[test]
    fn an_invites_pin_is_long_and_never_a_typed_one() {
        let client = crypto::Identity::generate().unwrap();
        let pin = "0123456789abcdef0123456789ABCDEF";
        let made = |pin: &str| {
            Pairing::for_invite(
                "client-1".into(),
                "Friend".into(),
                client.certificate.clone(),
                &[5; 16],
                pin,
                7,
            )
        };
        let pairing = made(pin).unwrap();
        assert_eq!(pairing.invite, Some(7));
        assert_eq!(pairing.key, crypto::pin_key(&[5; 16], pin));
        for wrong in ["1234", &pin[..31], &format!("{}g", &pin[..31])] {
            assert!(made(wrong).is_err(), "{wrong}");
        }
        // A typed pairing takes four digits only, and is no invite's.
        let typed = Pairing::new(
            "client-1".into(),
            "Phone".into(),
            client.certificate.clone(),
            &[5; 16],
            "1234",
        )
        .unwrap();
        assert_eq!(typed.invite, None);
        assert!(
            Pairing::new(
                "client-1".into(),
                "Phone".into(),
                client.certificate.clone(),
                &[5; 16],
                pin
            )
            .is_err()
        );
    }
    #[test]
    fn pair_with_real_rsa_certificates_and_existing_wire_format() {
        let server = crypto::Identity::generate().unwrap();
        let client = crypto::Identity::generate().unwrap();
        let salt = [5; 16];
        let key = crypto::pin_key(&salt, "1234");
        let mut p = Pairing::new(
            "client-1".into(),
            "Moonlight".into(),
            client.certificate.clone(),
            &salt,
            "1234",
        )
        .unwrap();
        let challenge = [7; 16];
        let response = p
            .client_challenge(&server, &crypto::ecb(&key, &challenge, true).unwrap())
            .unwrap();
        let response = crypto::ecb(&key, &response, false).unwrap();
        let client_secret = [9; 16];
        let mut expected = challenge.to_vec();
        expected.extend_from_slice(&server.signature);
        expected.extend_from_slice(&p.server_secret);
        assert_eq!(&response[..32], &crypto::hash(&expected));
        let mut proof = response[32..].to_vec();
        proof.extend_from_slice(&client.signature);
        proof.extend_from_slice(&client_secret);
        let reply = p
            .server_response(
                &server,
                &crypto::ecb(&key, &crypto::hash(&proof), true).unwrap(),
            )
            .unwrap();
        assert!(crypto::verify(&server.certificate, &reply[..16], &reply[16..]).unwrap());
        let mut secret = client_secret.to_vec();
        secret.extend_from_slice(&client.sign(&client_secret));
        p.finish(&secret).unwrap();
        assert!(p.finish(&secret).is_err());
    }

    fn der_len(len: usize) -> Vec<u8> {
        if len < 0x80 {
            vec![len as u8]
        } else {
            let bytes = len.to_be_bytes();
            let skip = bytes.iter().take_while(|&&b| b == 0).count();
            let mut out = vec![0x80 | (bytes.len() - skip) as u8];
            out.extend_from_slice(&bytes[skip..]);
            out
        }
    }
    fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend_from_slice(&der_len(content.len()));
        out.extend_from_slice(content);
        out
    }
    fn der_seq(parts: &[&[u8]]) -> Vec<u8> {
        der_tlv(0x30, &parts.concat())
    }
    fn der_bit_string(bytes: &[u8]) -> Vec<u8> {
        let mut content = Vec::with_capacity(1 + bytes.len());
        content.push(0);
        content.extend_from_slice(bytes);
        der_tlv(0x03, &content)
    }
    fn der_cn(cn: &str) -> Vec<u8> {
        let oid_cn = [0x06, 0x03, 0x55, 0x04, 0x03];
        let value = der_tlv(0x0c, cn.as_bytes());
        let atv = der_seq(&[&oid_cn, &value]);
        let rdn = der_tlv(0x31, &atv);
        der_seq(&[&rdn])
    }
    fn build_self_signed_der(
        key: &rsa::RsaPrivateKey,
        cn: &str,
        version: Option<u8>,
        sig_alg_oid: &[u8],
    ) -> Vec<u8> {
        use rsa::pkcs8::EncodePublicKey;
        use rsa::signature::{RandomizedSigner, SignatureEncoding};
        let alg_id = der_seq(&[sig_alg_oid, &[0x05, 0x00]]);
        let name = der_cn(cn);
        let not_before = der_tlv(0x17, b"250101000000Z");
        let not_after = der_tlv(0x17, b"450101000000Z");
        let validity = der_seq(&[&not_before, &not_after]);
        let spki = rsa::RsaPublicKey::from(key)
            .to_public_key_der()
            .unwrap()
            .into_vec();
        let serial = [0x02, 0x01, 0x01];
        let mut tbs_parts: Vec<Vec<u8>> = Vec::new();
        if let Some(v) = version {
            tbs_parts.push(der_tlv(0xa0, &[0x02, 0x01, v]));
        }
        tbs_parts.push(serial.to_vec());
        tbs_parts.push(alg_id.clone());
        tbs_parts.push(name.clone());
        tbs_parts.push(validity);
        tbs_parts.push(name);
        tbs_parts.push(spki);
        let tbs_refs: Vec<&[u8]> = tbs_parts.iter().map(Vec::as_slice).collect();
        let tbs = der_seq(&tbs_refs);
        let sig_bytes = match sig_alg_oid {
            [
                0x06,
                0x09,
                0x2a,
                0x86,
                0x48,
                0x86,
                0xf7,
                0x0d,
                0x01,
                0x01,
                0x0b,
            ] => rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(key.clone())
                .sign_with_rng(&mut rand::rngs::OsRng, &tbs)
                .to_vec(),
            [
                0x06,
                0x09,
                0x2a,
                0x86,
                0x48,
                0x86,
                0xf7,
                0x0d,
                0x01,
                0x01,
                0x0c,
            ] => rsa::pkcs1v15::SigningKey::<sha2::Sha384>::new(key.clone())
                .sign_with_rng(&mut rand::rngs::OsRng, &tbs)
                .to_vec(),
            [
                0x06,
                0x09,
                0x2a,
                0x86,
                0x48,
                0x86,
                0xf7,
                0x0d,
                0x01,
                0x01,
                0x0d,
            ] => rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new(key.clone())
                .sign_with_rng(&mut rand::rngs::OsRng, &tbs)
                .to_vec(),
            _ => unreachable!(),
        };
        let sig = der_bit_string(&sig_bytes);
        der_seq(&[&tbs, &alg_id, &sig])
    }
    fn to_cert_pem(der: &[u8]) -> String {
        pem::encode(&pem::Pem::new("CERTIFICATE", der.to_vec()))
    }
    fn run_full_handshake(client: &crypto::Identity) -> Result<()> {
        let server = crypto::Identity::generate()?;
        let salt = [5; 16];
        let key = crypto::pin_key(&salt, "1234");
        let mut p = Pairing::new(
            "client-1".into(),
            "Client".into(),
            client.certificate.clone(),
            &salt,
            "1234",
        )?;
        let challenge = [7; 16];
        let response = p.client_challenge(&server, &crypto::ecb(&key, &challenge, true)?)?;
        let response = crypto::ecb(&key, &response, false)?;
        let client_secret = [9; 16];
        let mut proof = response[32..].to_vec();
        proof.extend_from_slice(&client.signature);
        proof.extend_from_slice(&client_secret);
        let _reply =
            p.server_response(&server, &crypto::ecb(&key, &crypto::hash(&proof), true)?)?;
        let mut secret = client_secret.to_vec();
        secret.extend_from_slice(&client.sign(&client_secret));
        p.finish(&secret)
    }
    const OID_SHA256_WITH_RSA: [u8; 11] = [
        0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b,
    ];
    const OID_SHA384_WITH_RSA: [u8; 11] = [
        0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c,
    ];
    const OID_SHA512_WITH_RSA: [u8; 11] = [
        0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d,
    ];

    #[test]
    fn real_rsa_client_and_host_certificates_verify_and_pair() {
        use rsa::pkcs8::EncodePrivateKey;
        // webOS v2 (moonlight-tv X.509 v2 certificate).
        let webos = crypto::Identity::from_pem(
            include_str!("../testdata/moonlight-tv-client.pem").into(),
            include_str!("../testdata/moonlight-tv-client.key").into(),
        )
        .unwrap();
        crypto::verify_pairing_certificate(&webos.certificate).unwrap();
        run_full_handshake(&webos).unwrap();

        // Sunshine / Rubylight (X.509 v3, CN=Sunshine).
        let sunshine = crypto::Identity::generate().unwrap();
        crypto::verify_pairing_certificate(&sunshine.certificate).unwrap();
        run_full_handshake(&sunshine).unwrap();

        // Moonlight (X.509 v3, CN=NVIDIA GameStream Client) & GFE (X.509 v1, no version tag).
        let rsa_key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let priv_pem = rsa_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        for (version, alg) in [
            (None, &OID_SHA256_WITH_RSA),
            (Some(2), &OID_SHA256_WITH_RSA),
            (Some(2), &OID_SHA384_WITH_RSA),
            (Some(2), &OID_SHA512_WITH_RSA),
        ] {
            let der = build_self_signed_der(&rsa_key, "NVIDIA GameStream Client", version, alg);
            let id = crypto::Identity::from_pem(to_cert_pem(&der), priv_pem.clone()).unwrap();
            crypto::verify_pairing_certificate(&id.certificate).unwrap();
            run_full_handshake(&id).unwrap();
        }
    }

    #[test]
    fn rejects_substituted_certificate_signature_and_tampered_fields() {
        use rsa::pkcs8::EncodePrivateKey;
        let victim = crypto::Identity::generate().unwrap();
        let attacker_key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let attacker_priv = attacker_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        let valid_der = build_self_signed_der(
            &attacker_key,
            "NVIDIA GameStream Client",
            Some(2),
            &OID_SHA256_WITH_RSA,
        );
        let (_, parsed) = x509_parser::parse_x509_certificate(&valid_der).unwrap();
        let tbs = parsed.tbs_certificate.as_ref();
        let alg = der_seq(&[&OID_SHA256_WITH_RSA, &[0x05, 0x00]]);
        let forged_der = der_seq(&[tbs, &alg, &der_bit_string(&victim.signature)]);
        let forged_pem = to_cert_pem(&forged_der);
        let forged = crypto::Identity::from_pem(forged_pem.clone(), attacker_priv).unwrap();
        assert_eq!(forged.signature, victim.signature);

        assert!(crypto::verify_pairing_certificate(&forged_pem).is_err());
        assert!(
            Pairing::new(
                "client-1".into(),
                "Attacker".into(),
                forged_pem.clone(),
                &[5; 16],
                "1234",
            )
            .is_err()
        );

        // Even if checked directly during finish(), forged self-signature fails.
        let client_secret = [9; 16];
        let server_challenge = [7; 16];
        let mut proof_data = server_challenge.to_vec();
        proof_data.extend_from_slice(&victim.signature);
        proof_data.extend_from_slice(&client_secret);
        let mut pairing = Pairing {
            unique_id: "client-1".into(),
            name: "Attacker".into(),
            certificate: forged_pem,
            created: Instant::now(),
            peer: None,
            permission: None,
            approver: None,
            invite: None,
            phase: Phase::Response,
            key: [0; 16],
            server_secret: [0; 16],
            server_challenge,
            client_hash: crypto::hash(&proof_data).to_vec(),
        };
        let mut secret = client_secret.to_vec();
        secret.extend_from_slice(&forged.sign(&client_secret));
        assert!(pairing.finish(&secret).is_err());

        // Single-byte modification in tbsCertificate invalidates self-signature.
        let mut tampered_der = valid_der.clone();
        let pos = tampered_der
            .windows(6)
            .position(|w| w == b"NVIDIA")
            .unwrap();
        tampered_der[pos] = b'X';
        assert!(crypto::verify_pairing_certificate(&to_cert_pem(&tampered_der)).is_err());

        // Algorithm mismatch between outer signatureAlgorithm and inner tbs.signature fails.
        let outer_sha512 = der_seq(&[&OID_SHA512_WITH_RSA, &[0x05, 0x00]]);
        let mismatched = der_seq(&[
            tbs,
            &outer_sha512,
            &der_bit_string(parsed.signature_value.as_ref()),
        ]);
        assert!(crypto::verify_pairing_certificate(&to_cert_pem(&mismatched)).is_err());
    }

    #[test]
    fn rejects_non_65537_rsa_exponent_and_short_keys() {
        let e3_key = rsa::RsaPrivateKey::new_with_exp(
            &mut rand::rngs::OsRng,
            2048,
            &rsa::BigUint::from(3u32),
        )
        .unwrap();
        let e3_der =
            build_self_signed_der(&e3_key, "CustomExponent", Some(2), &OID_SHA256_WITH_RSA);
        let e3_pem = to_cert_pem(&e3_der);
        assert!(crypto::verify_pairing_certificate(&e3_pem).is_err());
        assert!(
            Pairing::new("client-1".into(), "Client".into(), e3_pem, &[5; 16], "1234",).is_err()
        );

        let short_key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).unwrap();
        let short_der =
            build_self_signed_der(&short_key, "ShortKey", Some(2), &OID_SHA256_WITH_RSA);
        assert!(crypto::verify_pairing_certificate(&to_cert_pem(&short_der)).is_err());
    }
}
