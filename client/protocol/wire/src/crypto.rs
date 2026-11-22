//! Wire-level AEAD and CBC primitives and constant-time comparison shared by host and client.
use aes::{
    Aes128,
    cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7},
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
        if tag.len() != 16 {
            bail!("invalid authentication tag");
        }
        let mut b = data.to_vec();
        let tag = aes_gcm::Tag::from_slice(tag);
        match (self, nonce.len()) {
            (Self::Nonce12(gcm), 12) => {
                gcm.decrypt_in_place_detached(Nonce::from_slice(nonce), &[], &mut b, tag)
            }
            (Self::Nonce16(gcm), 16) => {
                gcm.decrypt_in_place_detached(Nonce::<U16>::from_slice(nonce), &[], &mut b, tag)
            }
            _ => bail!("unsupported AES-GCM nonce size"),
        }
        .map_err(|_| anyhow::anyhow!("authentication failed"))?;
        Ok(b)
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
    // Whole 16-byte blocks, one more for a length that is already a multiple:
    // the size encrypt_padded_vec_mut allocates for the same bytes.
    out.clear();
    out.resize((data.len() | 15) + 1, 0);
    let written = cbc::Encryptor::<Aes128>::new(key.into(), iv.into())
        .encrypt_padded_b2b_mut::<Pkcs7>(data, out)
        .expect("the buffer holds the padded data");
    let len = written.len();
    out.truncate(len);
}

pub fn cbc_open(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>> {
    cbc::Decryptor::<Aes128>::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(data)
        .map_err(|_| anyhow::anyhow!("invalid CBC payload"))
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
}
