//! Wire-level AEAD and CBC primitives and constant-time comparison shared by host and client.
use aes::{
    Aes128,
    cipher::{BlockDecryptMut, BlockEncryptMut, InnerIvInit, block_padding::Pkcs7},
};
use aes_gcm::{
    Aes128Gcm, AesGcm, Nonce,
    aead::{AeadInPlace, KeyInit, consts::U16},
};
use anyhow::{Result, bail};
use subtle::ConstantTimeEq;

pub fn equal(a: &[u8], b: &[u8]) -> bool {
    bool::from(a.ct_eq(b))
}

pub fn gcm_seal(key: &[u8; 16], nonce: &[u8], data: &[u8]) -> Result<([u8; 16], Vec<u8>)> {
    let mut b = data.to_vec();
    let tag = gcm_seal_in_place(key, nonce, &mut b)?;
    Ok((tag, b))
}

/// [`gcm_seal`] without the plaintext copy: `data` is encrypted in place.
pub fn gcm_seal_in_place(key: &[u8; 16], nonce: &[u8], data: &mut [u8]) -> Result<[u8; 16]> {
    GcmKey::new(key, nonce.len())?.seal_in_place(nonce, data)
}

/// An AES-128-GCM key for one nonce size, expanded once for every message
/// sealed or opened with it.
pub enum GcmKey {
    Nonce12(Aes128Gcm),
    Nonce16(AesGcm<Aes128, U16>),
}

impl GcmKey {
    pub fn new(key: &[u8; 16], nonce_len: usize) -> Result<Self> {
        Ok(match nonce_len {
            12 => Self::sized(key, false),
            16 => Self::sized(key, true),
            _ => bail!("unsupported AES-GCM nonce size"),
        })
    }

    /// For 16-byte nonces when `long`, else for 12-byte ones.
    pub fn sized(key: &[u8; 16], long: bool) -> Self {
        if long {
            Self::Nonce16(AesGcm::new(key.into()))
        } else {
            Self::Nonce12(Aes128Gcm::new(key.into()))
        }
    }

    pub fn seal_in_place(&self, nonce: &[u8], data: &mut [u8]) -> Result<[u8; 16]> {
        let tag = match (self, nonce.len()) {
            (Self::Nonce12(gcm), 12) => {
                gcm.encrypt_in_place_detached(Nonce::from_slice(nonce), &[], data)
            }
            (Self::Nonce16(gcm), 16) => {
                gcm.encrypt_in_place_detached(Nonce::<U16>::from_slice(nonce), &[], data)
            }
            _ => bail!("unsupported AES-GCM nonce size"),
        }
        .map_err(|_| anyhow::anyhow!("AES-GCM encryption failed"))?;
        Ok(tag.into())
    }

    pub fn open(&self, nonce: &[u8], tag: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let mut b = Vec::with_capacity(data.len());
        self.open_into(nonce, tag, data, &mut b)?;
        Ok(b)
    }

    /// [`Self::open`] decrypting into `out`, whose room the caller reuses across packets.
    pub fn open_into<'a>(
        &self,
        nonce: &[u8],
        tag: &[u8],
        data: &[u8],
        out: &'a mut Vec<u8>,
    ) -> Result<&'a [u8]> {
        if tag.len() != 16 {
            out.clear();
            bail!("invalid authentication tag");
        }
        out.clear();
        out.extend_from_slice(data);
        let tag = aes_gcm::Tag::from_slice(tag);
        let decrypted = match (self, nonce.len()) {
            (Self::Nonce12(gcm), 12) => {
                gcm.decrypt_in_place_detached(Nonce::from_slice(nonce), &[], out, tag)
            }
            (Self::Nonce16(gcm), 16) => {
                gcm.decrypt_in_place_detached(Nonce::<U16>::from_slice(nonce), &[], out, tag)
            }
            _ => {
                out.clear();
                bail!("unsupported AES-GCM nonce size");
            }
        };
        if decrypted.is_err() {
            out.clear();
            bail!("authentication failed");
        }
        Ok(out.as_slice())
    }
}

/// An AES-128-CBC key, expanded once for every packet sealed or opened with it.
#[derive(Clone)]
pub struct CbcKey(Aes128);

impl CbcKey {
    pub fn new(key: &[u8; 16]) -> Self {
        Self(Aes128::new(key.into()))
    }

    /// PKCS#7-padded AES-128-CBC into `out` (overwritten), keeping its allocation
    /// across packets and skipping zero-fill when `out` is already large enough.
    pub fn seal_into(&self, iv: &[u8; 16], data: &[u8], out: &mut Vec<u8>) {
        // Whole 16-byte blocks, one more for a length that is already a multiple:
        // every byte in `0..padded_len` is overwritten by ciphertext below.
        let padded_len = (data.len() | 15) + 1;
        if out.len() < padded_len {
            out.resize(padded_len, 0);
        } else {
            out.truncate(padded_len);
        }
        let written = cbc::Encryptor::<&Aes128>::inner_iv_init(&self.0, iv.into())
            .encrypt_padded_b2b_mut::<Pkcs7>(data, out)
            .expect("the buffer holds the padded data");
        let len = written.len();
        out.truncate(len);
    }

    /// [`Self::seal_into`] appended after `out`'s existing prefix (such as an RTP header).
    pub fn seal_append(&self, iv: &[u8; 16], data: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        let padded_len = (data.len() | 15) + 1;
        out.resize(start + padded_len, 0);
        let written = cbc::Encryptor::<&Aes128>::inner_iv_init(&self.0, iv.into())
            .encrypt_padded_b2b_mut::<Pkcs7>(data, &mut out[start..])
            .expect("the buffer holds the padded data");
        let len = written.len();
        out.truncate(start + len);
    }

    /// PKCS#7-unpadded AES-128-CBC into `out`, reusing its allocation across packets.
    pub fn open_into<'a>(
        &self,
        iv: &[u8; 16],
        data: &[u8],
        out: &'a mut Vec<u8>,
    ) -> Result<&'a [u8]> {
        if data.is_empty() || !data.len().is_multiple_of(16) {
            out.clear();
            bail!("invalid CBC payload");
        }
        if out.len() < data.len() {
            out.resize(data.len(), 0);
        } else {
            out.truncate(data.len());
        }
        let plain_len = match cbc::Decryptor::<&Aes128>::inner_iv_init(&self.0, iv.into())
            .decrypt_padded_b2b_mut::<Pkcs7>(data, out)
        {
            Ok(plain) => plain.len(),
            Err(_) => {
                out.clear();
                bail!("invalid CBC payload");
            }
        };
        out.truncate(plain_len);
        Ok(out.as_slice())
    }
}

pub fn gcm_open(key: &[u8; 16], nonce: &[u8], tag: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if tag.len() != 16 {
        bail!("invalid authentication tag");
    }
    GcmKey::new(key, nonce.len())?.open(nonce, tag, data)
}

pub fn cbc_seal(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    cbc_seal_into(key, iv, data, &mut out);
    out
}

/// [`cbc_seal`] into `out`, overwritten, whose room the caller keeps between packets.
pub fn cbc_seal_into(key: &[u8; 16], iv: &[u8; 16], data: &[u8], out: &mut Vec<u8>) {
    CbcKey::new(key).seal_into(iv, data, out);
}

pub fn cbc_open(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    CbcKey::new(key).open_into(iv, data, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nist_aes_gcm_vector() {
        let (tag, b) = gcm_seal(&[0; 16], &[0; 12], &[0; 16]).unwrap();
        assert_eq!(
            b,
            [
                0x03, 0x88, 0xda, 0xce, 0x60, 0xb6, 0xa3, 0x92, 0xf3, 0x28, 0xc2, 0xb9, 0x71, 0xb2,
                0xfe, 0x78
            ]
        );
        assert_eq!(
            tag,
            [
                0xab, 0x6e, 0x47, 0xd4, 0x2c, 0xec, 0x13, 0xbd, 0xf5, 0x3a, 0x67, 0xb2, 0x12, 0x57,
                0xbd, 0xdf
            ]
        );
    }

    #[test]
    fn reject_forged_tag_before_parsing() {
        let (mut t, b) = gcm_seal(&[3; 16], &[4; 12], b"authenticated input").unwrap();
        t[0] ^= 1;
        assert!(gcm_open(&[3; 16], &[4; 12], &t, &b).is_err());
    }

    #[test]
    fn legacy_nonce_and_audio_round_trip() {
        let k = [4; 16];
        let (t, b) = gcm_seal(&k, &[6; 16], b"old client").unwrap();
        assert_eq!(gcm_open(&k, &[6; 16], &t, &b).unwrap(), b"old client");
        let b = cbc_seal(&k, &[1; 16], b"opus");
        assert_eq!(cbc_open(&k, &[1; 16], &b).unwrap(), b"opus");
    }

    #[test]
    fn pre_expanded_cbc_and_gcm_open_into_reuse_buffers_without_leaking_stale_bytes() {
        let k = std::array::from_fn(|i| (i * 13 + 5) as u8);
        let iv = [7u8; 16];
        let cbc = CbcKey::new(&k);
        let mut sealed = vec![0xEE; 128];
        let mut opened = vec![0xDD; 128];
        let mut prefixed = Vec::new();
        for len in [0usize, 1, 15, 16, 17, 31, 32, 63, 64] {
            let plain: Vec<u8> = (0..len).map(|i| (i * 19 + 1) as u8).collect();
            let expected = cbc_seal(&k, &iv, &plain);
            cbc.seal_into(&iv, &plain, &mut sealed);
            assert_eq!(sealed, expected, "seal_into len {len}");
            prefixed.clear();
            prefixed.extend_from_slice(&[0xAA; 12]);
            cbc.seal_append(&iv, &plain, &mut prefixed);
            assert_eq!(&prefixed[..12], &[0xAA; 12]);
            assert_eq!(
                &prefixed[12..],
                expected.as_slice(),
                "seal_append len {len}"
            );
            let got = cbc.open_into(&iv, &sealed, &mut opened).unwrap();
            assert_eq!(got, plain.as_slice(), "open_into len {len}");
        }
        // Invalid padding (corrupting the final block's CBC chain byte) clears the scratch buffer.
        let mut bad = sealed.clone();
        let last = bad.len();
        bad[last - 17] ^= 0xFF;
        assert!(cbc.open_into(&iv, &bad, &mut opened).is_err());
        assert!(opened.is_empty());

        let gcm = GcmKey::sized(&k, false);
        let (tag, ct) = gcm_seal(&k, &[9; 12], b"control payload").unwrap();
        let mut gcm_out = vec![0xCC; 64];
        assert_eq!(
            gcm.open_into(&[9; 12], &tag, &ct, &mut gcm_out).unwrap(),
            b"control payload"
        );
        let mut bad_tag = tag;
        bad_tag[0] ^= 1;
        assert!(
            gcm.open_into(&[9; 12], &bad_tag, &ct, &mut gcm_out)
                .is_err()
        );
        assert!(gcm_out.is_empty());
    }
}
